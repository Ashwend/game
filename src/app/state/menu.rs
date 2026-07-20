use bevy::prelude::*;

use crate::{
    auth::AuthenticatedUser,
    save::{CorruptedWorld, WorldStore},
};

use super::{
    ConfirmationDialog, CreateWorldDialog, DirectConnectDialog, EditWorldDialog, LoadingSplash,
    NoticeDialog, WorldStartAttempt,
};

pub(crate) const DEFAULT_MULTIPLAYER_ADDR: &str = "46.224.101.205:7777";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    MainMenu,
    Options,
    // The singleplayer world picker. Its only constructor is the Singleplayer
    // main-menu button, which is `#[cfg(debug_assertions)]`-gated (dev/test
    // only; see `src/app/ui/menu.rs - main_menu_ui`), plus the dev-only headless
    // control socket. In a shipped release nothing constructs it, so allow the
    // resulting dead-code lint there; the variant and `worlds_ui` stay compiled.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    Worlds,
    Multiplayer,
    InGame,
}

impl Screen {
    pub(crate) fn uses_menu_backdrop(self) -> bool {
        self != Self::InGame
    }
}

#[derive(Resource)]
pub(crate) struct SaveStore(pub(crate) WorldStore);

#[derive(Resource)]
pub(crate) struct CurrentUser(pub(crate) AuthenticatedUser);

#[derive(Resource)]
pub(crate) struct MenuState {
    pub(crate) screen: Screen,
    pub(crate) worlds: Vec<crate::save::WorldSummary>,
    /// Saves that were present on disk but could not be parsed (truncated,
    /// bad header, mismatched format version, …). Rendered above the worlds
    /// table as a separate "couldn't load" group so the player knows there
    /// are files that need attention rather than seeing them silently
    /// dropped from the list.
    pub(crate) corrupted_worlds: Vec<CorruptedWorld>,
    pub(crate) create_world: Option<CreateWorldDialog>,
    pub(crate) edit_world: Option<EditWorldDialog>,
    pub(crate) direct_connect: Option<DirectConnectDialog>,
    pub(crate) world_start: Option<WorldStartAttempt>,
    /// Loading overlay shown on top of every screen. Set on app launch
    /// (with the `Startup` kind) and again whenever the player commits
    /// to entering a world. Lives in `MenuState` (not `ClientRuntime`)
    /// because it's a UI-only artifact: gameplay state advances normally
    /// underneath.
    pub(crate) loading_splash: Option<LoadingSplash>,
    pub(crate) multiplayer_addr: String,
    pub(crate) status: Option<String>,
    pub(crate) pause_open: bool,
    pub(crate) pause_options_open: bool,
    pub(crate) inventory_open: bool,
    /// Whether the dedicated crafting screen is up. Like
    /// `inventory_open`, this only frees the cursor and gates input, it
    /// does not pause gameplay or the network tick. The server keeps
    /// progressing the queue regardless of this flag.
    pub(crate) crafting_open: bool,
    /// Mirrors `local_player.open_furnace.is_some()` from the snapshot.
    /// Used by the input gating helpers to suppress movement/look while
    /// the furnace modal is up, without those helpers having to reach
    /// into the snapshot themselves. Synced by
    /// [`crate::app::systems::sync_furnace_open_flag_system`].
    pub(crate) furnace_open: bool,
    /// Mirrors `local_player.open_loot_bag.is_some()` from
    /// replication. Same purpose as `furnace_open`, input gating
    /// without reaching into the replicated state from every
    /// helper.
    pub(crate) loot_bag_open: bool,
    /// Mirrors `local_player.open_workbench.is_some()` from replication. Same
    /// purpose as `furnace_open`: input gating without reaching into the
    /// replicated state from every helper. Synced by
    /// [`crate::app::systems::sync_workbench_open_flag_system`].
    pub(crate) workbench_open: bool,
    /// True while the world-map overlay is toggled open. A translucent overlay
    /// (server-rastered terrain + the player's own markers + a grid and facing
    /// arrow) is shown; the cursor is freed for marker interaction and
    /// look/swing freeze, but movement stays live and, per the "gameplay never
    /// pauses" invariant, simulation keeps ticking. The map key (or Escape)
    /// closes it. Managed by `world_map_input_system`.
    pub(crate) world_map_open: bool,
    pub(crate) chat_open: bool,
    pub(crate) chat_focus_pending: bool,
    pub(crate) chat_input: String,
    pub(crate) confirmation: Option<ConfirmationDialog>,
    /// Set by a confirmed `DeleteWorldMapMarker` confirmation; drained by
    /// `world_map_input_system`, which sends the server the delete command (the
    /// generic confirmation handler has no network access). Lives here, not on
    /// `WorldMapUiState`, so it survives even if the map is closed before the
    /// next in-game frame drains it.
    pub(crate) world_map_delete_pending: Option<u32>,
    pub(crate) notice: Option<NoticeDialog>,
    /// In-game single-field text dialog (door codes, sleeping-bag
    /// rename). One slot: opening a new prompt replaces any current one.
    /// Gates controls through `gameplay_accepts_controls` like every
    /// other overlay.
    pub(crate) text_prompt: Option<TextPrompt>,
    /// Set when the server tells the local client they died. Drives
    /// the "You died, Killed by …, Respawn" splash. Cleared when the
    /// respawn lands (server pushes a `Correction` and the runtime
    /// flips back to alive) so the splash auto-dismisses.
    pub(crate) death_splash: Option<DeathSplash>,
    /// Live cinematic playback state, mirroring the server's
    /// `ServerMessage::Cinematic` cues. While set, the camera detaches onto
    /// the shot paths, controls are blocked (simulation keeps ticking, per
    /// the invariant), the HUD hides, and the countdown slate draws. Cleared
    /// by the `Stopped` cue.
    pub(crate) cinematic: Option<CinematicOverlay>,
    pub(crate) quit_requested: bool,
    /// Set by the title-screen "Sign out" link; consumed by
    /// `drive_auth_flow_system` (token store cleared + back to the login splash).
    pub(crate) sign_out_requested: bool,
    /// Set by the login splash's Cancel button / Escape while a sign-in (or
    /// startup restore) is in flight; consumed by `drive_auth_flow_system`,
    /// which tells the worker to stop waiting on the browser and drops back to
    /// the login splash with no error.
    pub(crate) cancel_auth_requested: bool,
    /// Set when a join is abandoned because the stored session can't be renewed
    /// (no refresh token, or it's no longer valid). Consumed by
    /// `drive_auth_flow_system`, which clears `CurrentUser` and drops back to
    /// the login splash showing this reason, so the player understands why they
    /// were signed out instead of just being bounced.
    pub(crate) force_sign_out: Option<String>,
}

/// Client-side cinematic playback state, driven by the server's
/// `ServerMessage::Cinematic` phase cues plus a local elapsed clock (the
/// countdown display, camera-path time, and intermission chip all derive
/// from `elapsed` against the shared script; only phase edges ride the
/// wire). Stored on `MenuState` because it gates input exactly like the
/// other overlays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CinematicOverlay {
    pub(crate) phase: CinematicOverlayPhase,
    /// Seconds since the current phase's cue arrived (client-local clock,
    /// advanced by `tick_cinematic_overlay_system`).
    pub(crate) elapsed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CinematicOverlayPhase {
    /// Server init phase: world cleanup + stage spawn. Camera parks on shot
    /// 0's opening frame under a "preparing" slate.
    Preparing,
    /// Countdown slate before `shot_index`; camera parked on its opening
    /// frame so the operator sees the framing before action.
    Countdown { shot_index: usize, seconds: f32 },
    /// The shot is live: clean frame, camera flying the authored path.
    Playing { shot_index: usize },
    /// Post-shot idle: camera holds `prev_shot_index`'s final frame for a
    /// clean cut; a small chip shows what's next (or that it finished).
    Intermission {
        prev_shot_index: usize,
        next_shot_index: Option<usize>,
        seconds: f32,
    },
}

impl CinematicOverlay {
    pub(crate) fn new(phase: CinematicOverlayPhase) -> Self {
        Self {
            phase,
            elapsed: 0.0,
        }
    }

    /// Which shot the detached camera should show, and at what path time.
    /// `Playing` advances with the local clock; every other phase parks on
    /// a still frame (start of the upcoming shot, or end of the previous).
    pub(crate) fn camera_target(&self) -> (usize, f32) {
        match self.phase {
            CinematicOverlayPhase::Preparing => (0, 0.0),
            CinematicOverlayPhase::Countdown { shot_index, .. } => (shot_index, 0.0),
            CinematicOverlayPhase::Playing { shot_index } => (shot_index, self.elapsed),
            CinematicOverlayPhase::Intermission {
                prev_shot_index, ..
            } => (prev_shot_index, f32::MAX),
        }
    }
}

/// Snapshot of "I just died" UI state. Stored on `MenuState` because
/// the splash sits in the same UI layer as the pause/inventory
/// overlays and shares their input gating.
#[derive(Debug, Clone)]
pub(crate) struct DeathSplash {
    /// Display name of the killer, resolved server-side. `None` for
    /// environmental death (future) where there's no attacker to
    /// credit.
    pub(crate) killer_name: Option<String>,
    /// Seconds since the splash opened. Drives the slow fade from
    /// "the player still sees the world dying" into the fully-black
    /// "YOU DIED" screen.
    pub(crate) elapsed: f32,
    /// Seconds spent in the closing-fade animation. `None` until the
    /// respawn `Correction` lands, at that point the splash keeps
    /// rendering but its backdrop alpha eases from "fully black" back
    /// to "fully transparent" over `CLOSE_FADE_SECS`. The splash
    /// clears itself when the fade completes, so the new HUD doesn't
    /// pop in for a frame underneath a still-black screen.
    pub(crate) closing_elapsed: Option<f32>,
    /// True once the player pressed Escape on the full-screen splash.
    /// The blackout collapses into a compact respawn pill so chat and
    /// the pause menu become reachable while dead; respawning is still
    /// one click away on the pill.
    pub(crate) minimized: bool,
    /// The dying player's placed sleeping bags, straight from
    /// `PlayerKilled`. Rendered as one "spawn here" button per bag next
    /// to the random respawn.
    pub(crate) respawn_bags: Vec<crate::protocol::RespawnBagOption>,
}

impl DeathSplash {
    pub(crate) fn new(
        killer_name: Option<String>,
        respawn_bags: Vec<crate::protocol::RespawnBagOption>,
    ) -> Self {
        Self {
            killer_name,
            elapsed: 0.0,
            closing_elapsed: None,
            minimized: false,
            respawn_bags,
        }
    }

    /// Start the close-fade. Idempotent, once started, the same
    /// timer keeps ticking; a second call is a no-op so racing
    /// `Correction` messages can't reset the curve and leave the
    /// player staring at black.
    pub(crate) fn begin_closing(&mut self) {
        if self.closing_elapsed.is_none() {
            self.closing_elapsed = Some(0.0);
        }
    }
}

/// What the in-game text prompt is for, and where its submission goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextPromptKind {
    /// Choose the lock code while hanging a door. The placement command
    /// only fires once this confirms, cancelling places nothing. `variant`
    /// is the door (wood vs iron) the player is hanging.
    DoorSetCode {
        doorway_id: crate::protocol::DeployedEntityId,
        variant: crate::items::DoorVariant,
        flip: bool,
    },
    /// The server prompted for the code of door `door_id`.
    DoorEnterCode {
        door_id: crate::protocol::DeployedEntityId,
    },
    /// Rotate the code of a door the player is authorized on.
    DoorChangeCode {
        door_id: crate::protocol::DeployedEntityId,
    },
    /// Rename an owned sleeping bag.
    RenameBag {
        bag_id: crate::protocol::DeployedEntityId,
    },
    /// Name (or rename) one of the player's own world-map markers.
    NameWorldMapMarker { id: u32 },
}

/// One single-field in-game dialog. Numeric for door codes, free text
/// for bag names; the UI reads the kind to pick labels and validation.
#[derive(Debug, Clone)]
pub(crate) struct TextPrompt {
    pub(crate) kind: TextPromptKind,
    pub(crate) input: String,
    pub(crate) autofocus_pending: bool,
}

impl TextPrompt {
    pub(crate) fn new(kind: TextPromptKind) -> Self {
        // The rename prompt opens from the sleeping-bag wheel, whose
        // trigger is a *held* E; autofocusing the free-text field would
        // let the still-held key type itself into the name ("e"). The
        // door prompts keep autofocus: their field is digits-only, so a
        // held letter key can't leak in.
        let autofocus_pending = !matches!(kind, TextPromptKind::RenameBag { .. });
        Self {
            kind,
            input: String::new(),
            autofocus_pending,
        }
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            screen: Screen::MainMenu,
            worlds: Vec::new(),
            corrupted_worlds: Vec::new(),
            create_world: None,
            edit_world: None,
            direct_connect: None,
            world_start: None,
            loading_splash: Some(LoadingSplash::startup()),
            multiplayer_addr: DEFAULT_MULTIPLAYER_ADDR.to_owned(),
            status: None,
            pause_open: false,
            pause_options_open: false,
            inventory_open: false,
            crafting_open: false,
            furnace_open: false,
            loot_bag_open: false,
            workbench_open: false,
            world_map_open: false,
            chat_open: false,
            chat_focus_pending: false,
            chat_input: String::new(),
            confirmation: None,
            world_map_delete_pending: None,
            notice: None,
            text_prompt: None,
            death_splash: None,
            cinematic: None,
            quit_requested: false,
            sign_out_requested: false,
            cancel_auth_requested: false,
            force_sign_out: None,
        }
    }
}

impl MenuState {
    /// True while a blocking dialog (single-field text prompt, confirmation, or
    /// notice) owns the screen. Gameplay hotkeys, toggle inventory/crafting,
    /// open chat, must stay inert while one is up so a keystroke meant for the
    /// dialog (typing a marker name, pressing Enter to confirm) doesn't also
    /// fire a shortcut behind it. Centralised so the keybind and UI open-paths
    /// can't drift out of sync.
    pub(crate) fn dialog_modal_open(&self) -> bool {
        self.text_prompt.is_some() || self.confirmation.is_some() || self.notice.is_some()
    }

    /// True while a centred panel overlay (inventory, crafting, furnace, loot
    /// bag, workbench, or the world map) covers the screen. The first-person
    /// held item hides while one is up: the viewmodel camera composites AFTER
    /// the UI pass (its whole job is drawing the tool over the finished frame),
    /// so a visible held item would render on top of the panel instead of
    /// behind it. Chat and toasts are corner overlays and deliberately don't
    /// count; the pause menu is handled separately by the held-item gate.
    pub(crate) fn panel_overlay_open(&self) -> bool {
        self.inventory_open
            || self.crafting_open
            || self.furnace_open
            || self.loot_bag_open
            || self.workbench_open
            || self.world_map_open
    }

    /// True while a world-entry loading splash (`EnteringWorld` /
    /// `JoiningServer`) is on screen. The screen is already `InGame`
    /// underneath it (so simulation runs, per the gameplay-never-pauses
    /// invariant), but the player can't see the world yet: input gating
    /// hides the held viewmodel and freezes controls, and the entity
    /// reconcilers switch to their aggressive loading spawn budgets (frame
    /// hitches behind an opaque overlay are invisible).
    pub(crate) fn world_entry_splash_active(&self) -> bool {
        self.loading_splash.as_ref().is_some_and(|splash| {
            matches!(
                splash.kind,
                crate::app::state::LoadingSplashKind::EnteringWorld
                    | crate::app::state::LoadingSplashKind::JoiningServer
            )
        })
    }

    /// Hand off to the in-game screen after a successful singleplayer or
    /// multiplayer session start. Resets the modal/chat overlay state. Both
    /// session-start paths (loopback singleplayer and direct multiplayer)
    /// funnel through here so the two flows can't drift in what they clear.
    ///
    /// This flips the *screen* (so the scene starts building underneath the
    /// splash) but deliberately does NOT mark the loading splash ready to
    /// fade, that's gated on the world actually being ready to interact with
    /// (Welcome applied, scene spawned, local player replicated) via
    /// `LoadingSplash::note_world_ready`, driven from the UI each frame. The
    /// session object existing only means the handshake started, not that the
    /// world has arrived or rendered.
    pub(crate) fn enter_in_game(&mut self) {
        self.screen = Screen::InGame;
        self.pause_open = false;
        self.pause_options_open = false;
        self.crafting_open = false;
        self.furnace_open = false;
        self.loot_bag_open = false;
        self.workbench_open = false;
        self.chat_open = false;
        self.chat_focus_pending = false;
        self.text_prompt = None;
        self.status = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn death_splash_new_has_no_killer_and_no_closing_timer() {
        let splash = DeathSplash::new(None, Vec::new());
        assert!(splash.killer_name.is_none());
        assert_eq!(splash.elapsed, 0.0);
        assert!(
            splash.closing_elapsed.is_none(),
            "the closing fade only starts after the respawn lands"
        );
    }

    #[test]
    fn death_splash_remembers_killer_name() {
        let splash = DeathSplash::new(Some("Murderer".into()), Vec::new());
        assert_eq!(splash.killer_name.as_deref(), Some("Murderer"));
    }

    #[test]
    fn death_splash_carries_respawn_bags() {
        let splash = DeathSplash::new(
            None,
            vec![crate::protocol::RespawnBagOption {
                id: crate::protocol::DeployedEntityId(7),
                name: "home".to_owned(),
                cooldown_seconds: 0,
            }],
        );
        assert_eq!(splash.respawn_bags.len(), 1);
        assert_eq!(splash.respawn_bags[0].name, "home");
    }

    #[test]
    fn begin_closing_starts_the_fade() {
        let mut splash = DeathSplash::new(None, Vec::new());
        splash.begin_closing();
        assert_eq!(
            splash.closing_elapsed,
            Some(0.0),
            "first call should start the closing timer at 0"
        );
    }

    #[test]
    fn begin_closing_is_idempotent() {
        // A second `Correction` that lands while the fade is already
        // running must not reset the curve, that would freeze the
        // player on a black screen for an extra fade window.
        let mut splash = DeathSplash::new(None, Vec::new());
        splash.begin_closing();
        splash.closing_elapsed = Some(0.25);
        splash.begin_closing();
        assert_eq!(splash.closing_elapsed, Some(0.25));
    }

    #[test]
    fn panel_overlays_count_but_chat_and_dialogs_do_not() {
        // The held item hides behind every centred panel overlay; chat is a
        // corner overlay and dialogs are handled by their own gate, so neither
        // counts here.
        let mut menu = MenuState::default();
        assert!(!menu.panel_overlay_open());

        menu.chat_open = true;
        assert!(!menu.panel_overlay_open(), "chat is not a panel overlay");
        menu.chat_open = false;

        for set in [
            |m: &mut MenuState| m.inventory_open = true,
            |m: &mut MenuState| m.crafting_open = true,
            |m: &mut MenuState| m.furnace_open = true,
            |m: &mut MenuState| m.loot_bag_open = true,
            |m: &mut MenuState| m.workbench_open = true,
            |m: &mut MenuState| m.world_map_open = true,
        ] {
            let mut menu = MenuState::default();
            set(&mut menu);
            assert!(menu.panel_overlay_open());
        }
    }
}
