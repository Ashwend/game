//! In-game update checker + self-updater (client-only).
//!
//! On boot a background thread (the app has no async runtime, so this mirrors
//! the [`crate::analytics`] worker: one OS thread, blocking `ureq`) asks GitHub
//! for the latest release. If it's newer than [`crate::protocol::GAME_VERSION`]
//! the player gets a modal with the changelog since their version and can
//! update in place or skip. Updating downloads the host's release archive,
//! verifies it, stages the new binary beside the running one, and hands off to
//! the sibling `ashwend-updater` process, which swaps the binary and relaunches
//! after this process exits.
//!
//! This module owns everything that does *not* touch the app's session/menu
//! state: the [`UpdateState`] resource, the worker, and the message pump. The
//! one system that must coordinate with the world save on quit lives in
//! `crate::app::systems` (where `ClientRuntime` et al. are in scope) and reads
//! [`UpdateState`] from here.

mod apply;
mod asset;
mod download;
mod github;
mod skipped;
mod version;

use std::{path::PathBuf, thread};

use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender, unbounded};

use self::{github::ReleaseAsset, version::Version};

pub(crate) use self::apply::spawn_updater;

/// Bevy plugin. Added on the client only (see `app::run_app`); the dedicated
/// server and admin CLI never load it.
pub(crate) struct UpdatePlugin;

impl Plugin for UpdatePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(UpdateState::spawn())
            .add_systems(Update, poll_update_messages_system);
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum UpdateStatus {
    /// Boot-time check in flight.
    Checking,
    /// Newest release is not newer than us (or the check failed, same UX:
    /// nothing to show).
    UpToDate,
    /// A newer release exists and is described by [`UpdateState::available`].
    Available,
    /// Downloading + extracting the new binary.
    Downloading { received: u64, total: Option<u64> },
    /// New binary staged and verified; ready to swap on restart.
    Ready,
    /// Player asked to restart & update; the apply system is taking over (it
    /// first saves any open world, then launches the updater and quits).
    Applying,
    /// Something went wrong; message is shown with a "download page" fallback.
    Failed(String),
}

/// Details of the newer release once the check finds one.
pub(crate) struct AvailableUpdate {
    pub(crate) version: String,
    pub(crate) changelog_md: String,
    /// The host's release asset, if this platform publishes one. `None` ⇒ the
    /// in-place path isn't available and we fall back to the download page.
    asset: Option<ReleaseAsset>,
    /// Set once the binary is downloaded, verified, and staged.
    staged_path: Option<PathBuf>,
}

#[derive(Resource)]
pub(crate) struct UpdateState {
    pub(crate) status: UpdateStatus,
    pub(crate) available: Option<AvailableUpdate>,
    /// Whether the changelog modal is showing. Auto-opens once on boot when an
    /// un-skipped update is found; the corner pill toggles it afterwards.
    pub(crate) modal_open: bool,
    /// Release notes for the build the player is *currently* running, fetched in
    /// the same boot-time check as the update probe. Drives the title-screen
    /// "what's new" modal opened from the version label. `None` until the check
    /// returns, when offline, or for a dev build with no matching release.
    current_changelog: Option<String>,
    /// Whether the title-screen "what's new" modal is showing.
    current_changelog_open: bool,
    skipped_version: Option<String>,
    channels: Option<Channels>,
}

struct Channels {
    cmd_tx: Sender<Cmd>,
    msg_rx: Receiver<Msg>,
}

enum Cmd {
    Download { asset: ReleaseAsset },
}

enum Msg {
    /// `available: None` ⇒ up to date / check failed. `current_changelog`
    /// carries the running build's own release notes regardless, for the
    /// title-screen "what's new" view.
    Checked {
        available: Option<AvailableUpdate>,
        current_changelog: Option<String>,
    },
    Progress {
        received: u64,
        total: Option<u64>,
    },
    Staged {
        path: PathBuf,
    },
    Failed(String),
}

impl UpdateState {
    fn spawn() -> Self {
        let skipped_version = skipped::load();
        let (cmd_tx, cmd_rx) = unbounded::<Cmd>();
        let (msg_tx, msg_rx) = unbounded::<Msg>();
        let spawned = thread::Builder::new()
            .name("ashwend-update-worker".to_owned())
            .spawn(move || run_worker(&cmd_rx, &msg_tx));
        match spawned {
            Ok(_join) => Self {
                status: UpdateStatus::Checking,
                available: None,
                modal_open: false,
                current_changelog: None,
                current_changelog_open: false,
                skipped_version,
                channels: Some(Channels { cmd_tx, msg_rx }),
            },
            Err(error) => {
                eprintln!("update: could not spawn checker thread, disabling: {error}");
                Self {
                    status: UpdateStatus::UpToDate,
                    available: None,
                    modal_open: false,
                    current_changelog: None,
                    current_changelog_open: false,
                    skipped_version,
                    channels: None,
                }
            }
        }
    }

    /// True while there is a newer release to act on (available, downloading,
    /// staged, applying, or a failure the player should see).
    pub(crate) fn has_update(&self) -> bool {
        self.available.is_some()
            && !matches!(self.status, UpdateStatus::Checking | UpdateStatus::UpToDate)
    }

    pub(crate) fn latest_version(&self) -> Option<&str> {
        self.available.as_ref().map(|a| a.version.as_str())
    }

    pub(crate) fn changelog(&self) -> &str {
        self.available
            .as_ref()
            .map(|a| a.changelog_md.as_str())
            .unwrap_or("")
    }

    pub(crate) fn open_modal(&mut self) {
        self.modal_open = true;
    }

    pub(crate) fn dismiss_modal(&mut self) {
        self.modal_open = false;
    }

    /// Release notes for the running build, if the boot check found a matching
    /// release. Drives the title-screen "what's new" modal.
    pub(crate) fn current_changelog(&self) -> Option<&str> {
        self.current_changelog.as_deref()
    }

    pub(crate) fn current_changelog_open(&self) -> bool {
        self.current_changelog_open
    }

    pub(crate) fn open_current_changelog(&mut self) {
        self.current_changelog_open = true;
    }

    pub(crate) fn dismiss_current_changelog(&mut self) {
        self.current_changelog_open = false;
    }

    /// Whether this install can update itself (supported host + the sibling
    /// updater binary is present). When false, the modal offers the download
    /// page instead of an in-place update.
    pub(crate) fn can_self_update(&self) -> bool {
        self.available.as_ref().is_some_and(|a| a.asset.is_some()) && apply::can_self_update()
    }

    /// Begin the in-place update: kick the worker off to download + stage. If
    /// in-place isn't possible, open the releases page and close the modal.
    pub(crate) fn begin_download(&mut self) {
        if !self.can_self_update() {
            apply::open_download_page();
            self.modal_open = false;
            return;
        }
        let asset = self
            .available
            .as_ref()
            .and_then(|a| a.asset.clone())
            .expect("can_self_update implies an asset");
        if let Some(channels) = &self.channels {
            let _ = channels.cmd_tx.send(Cmd::Download { asset });
            self.status = UpdateStatus::Downloading {
                received: 0,
                total: None,
            };
        }
    }

    /// Mark the update for application. The apply system in `crate::app` takes
    /// it from here (saving any open world before relaunching).
    pub(crate) fn request_apply(&mut self) {
        self.status = UpdateStatus::Applying;
    }

    /// Persist the available version as "skipped" so it won't auto-pop again,
    /// and close the modal. The corner pill stays.
    pub(crate) fn skip(&mut self) {
        if let Some(available) = &self.available {
            skipped::save(&available.version);
            self.skipped_version = Some(available.version.clone());
        }
        self.modal_open = false;
    }

    /// Open the releases page in the browser (used by the failure fallback).
    pub(crate) fn open_download_page(&self) {
        apply::open_download_page();
    }

    /// The staged binary path, once [`UpdateStatus::Ready`]. Read by the apply
    /// system.
    pub(crate) fn staged_path(&self) -> Option<PathBuf> {
        self.available.as_ref().and_then(|a| a.staged_path.clone())
    }

    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        self.status = UpdateStatus::Failed(message.into());
    }

    /// A disabled, no-worker state for tests that need to construct
    /// [`UpdateState`] without touching the network.
    #[cfg(test)]
    pub(crate) fn idle_for_test() -> Self {
        Self {
            status: UpdateStatus::UpToDate,
            available: None,
            modal_open: false,
            current_changelog: None,
            current_changelog_open: false,
            skipped_version: None,
            channels: None,
        }
    }
}

/// Drain worker messages into [`UpdateState`] each frame.
fn poll_update_messages_system(mut state: ResMut<UpdateState>) {
    let messages: Vec<Msg> = match &state.channels {
        Some(channels) => channels.msg_rx.try_iter().collect(),
        None => return,
    };
    for message in messages {
        match message {
            Msg::Checked {
                available,
                current_changelog,
            } => {
                state.current_changelog = current_changelog;
                match available {
                    Some(available) => {
                        let skipped =
                            state.skipped_version.as_deref() == Some(available.version.as_str());
                        state.available = Some(available);
                        state.status = UpdateStatus::Available;
                        // Auto-open on boot unless the player already skipped this
                        // exact version; the pill still appears either way.
                        state.modal_open = !skipped;
                    }
                    None => state.status = UpdateStatus::UpToDate,
                }
            }
            Msg::Progress { received, total } => {
                if matches!(state.status, UpdateStatus::Downloading { .. }) {
                    state.status = UpdateStatus::Downloading { received, total };
                }
            }
            Msg::Staged { path } => {
                if let Some(available) = &mut state.available {
                    available.staged_path = Some(path);
                }
                state.status = UpdateStatus::Ready;
            }
            Msg::Failed(error) => {
                eprintln!("update: {error}");
                state.status = UpdateStatus::Failed(error);
            }
        }
    }
}

fn run_worker(cmd_rx: &Receiver<Cmd>, msg_tx: &Sender<Msg>) {
    let agent = github::build_agent();

    // Boot-time check.
    let _ = msg_tx.send(check_latest(&agent));

    // Then serve download requests until the channel closes (process exit).
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Cmd::Download { asset } => {
                let progress = |received: u64, total: Option<u64>| {
                    let _ = msg_tx.send(Msg::Progress { received, total });
                };
                match download::download_and_stage(&agent, &asset, &progress) {
                    Ok(path) => {
                        let _ = msg_tx.send(Msg::Staged { path });
                    }
                    Err(error) => {
                        let _ = msg_tx.send(Msg::Failed(error));
                    }
                }
            }
        }
    }
}

fn check_latest(agent: &ureq::Agent) -> Msg {
    let releases = match github::fetch_releases(agent) {
        Ok(releases) => releases,
        Err(error) => {
            // Treat any check failure as "up to date", never block or nag on a
            // flaky network. Log once for diagnostics.
            eprintln!("update: check failed: {error}");
            return Msg::Checked {
                available: None,
                current_changelog: None,
            };
        }
    };

    // Always capture the running build's own notes for the "what's new" view,
    // independent of whether a newer release exists.
    let current = Version::current();
    let current_changelog = github::changelog_for(&releases, current);
    let available = newer_release(&releases, current);
    Msg::Checked {
        available,
        current_changelog,
    }
}

/// The newest stable release strictly newer than `current`, packaged for the
/// update modal. `None` when the player is already on the latest stable build
/// or the release list carries no usable tag.
fn newer_release(releases: &[github::Release], current: Version) -> Option<AvailableUpdate> {
    let latest = github::latest_stable(releases)?;
    let latest_version = Version::parse(&latest.tag_name)?;
    if latest_version <= current {
        return None;
    }
    Some(AvailableUpdate {
        version: latest_version.to_string(),
        changelog_md: github::changelog_since(releases, current),
        asset: latest.host_asset().cloned(),
        staged_path: None,
    })
}
