//! PostHog analytics plugin.
//!
//! - **Client-only.** Added in `app::run_app`. Dedicated server and admin
//!   CLI do not load this module.
//! - **Opt-in.** Disabled unless `analytics.local.toml` exists in the repo
//!   root or `POSTHOG_*` env vars are set (see [`config`]). Disabled mode
//!   short-circuits in [`Analytics::track`] before any work is done.
//! - **Non-blocking.** Events are pushed into a bounded mpsc channel and
//!   flushed by a background thread (see [`client`]). The game thread
//!   never touches the network.
//! - **Privacy.** EU endpoints by default, `$ip = null` when
//!   `disable_geoip` is set, no chat text / player names / save paths in
//!   properties. Anonymous UUID — not account ID — as `distinct_id`.

mod client;
pub(crate) mod config;
mod context;
mod distinct_id;
pub(crate) mod events;

use std::{
    env,
    sync::{Arc, Mutex},
    time::Instant,
};

use bevy::{app::AppExit, prelude::*};
use uuid::Uuid;

pub(crate) use events::{
    ConnectFailReason, ErrorCategory, Event, ScreenKind, SessionEndReason, SessionMode,
};

use self::{
    client::{EventRecord, WorkerHandle, WorkerSender},
    config::AnalyticsConfig,
    context::{SuperPropsHandle, fill_render_props_system},
};

/// Bevy resource carrying the analytics sender (or a no-op when disabled).
///
/// Cheap to clone: holds an `Arc` to the worker handle plus a `Clone`
/// sender. Pass by `Res<Analytics>` at call sites and call
/// [`Analytics::track`].
#[derive(Resource, Clone)]
pub(crate) struct Analytics {
    inner: Option<AnalyticsInner>,
}

#[derive(Clone)]
struct AnalyticsInner {
    sender: WorkerSender,
    handle: Arc<WorkerHandle>,
}

impl Analytics {
    /// No-op analytics resource. Inserted when config is missing/disabled.
    pub(crate) fn disabled() -> Self {
        Self { inner: None }
    }

    /// Enqueue an event onto the background worker. Cheap when disabled —
    /// the call short-circuits before any property allocation.
    pub(crate) fn track(&self, event: Event) {
        let Some(inner) = &self.inner else {
            return;
        };
        let (name, properties) = event.name_and_props();
        inner.sender.try_send(EventRecord {
            name,
            properties,
            timestamp_ms: now_ms(),
        });
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Tracks the wall-clock app start so [`Event::AppQuit`] can include a
/// duration. Inserted alongside [`Analytics`] when the plugin runs.
#[derive(Resource)]
pub(crate) struct AnalyticsAppStart(pub(crate) Instant);

/// Bevy plugin. Add only on the client (see `app::run_app`).
pub(crate) struct AnalyticsPlugin;

impl Plugin for AnalyticsPlugin {
    fn build(&self, app: &mut App) {
        let repo_root = env::current_dir().unwrap_or_else(|_| ".".into());
        let cfg = AnalyticsConfig::load(&repo_root);
        let initial_props = context::SuperProps::initial(cfg.environment);
        let shared_props: context::SharedProps = Arc::new(Mutex::new(initial_props));

        let analytics = if cfg.enabled {
            spawn_worker(&cfg, Arc::clone(&shared_props))
        } else {
            Analytics::disabled()
        };

        app.insert_resource(analytics)
            .insert_resource(AnalyticsAppStart(Instant::now()))
            .insert_resource(SuperPropsHandle(shared_props))
            .add_systems(Startup, app_started_system)
            .add_systems(Update, fill_render_props_system)
            .add_systems(Update, app_quit_drain_system);
    }
}

fn spawn_worker(cfg: &AnalyticsConfig, shared_props: context::SharedProps) -> Analytics {
    let api_key = match cfg.api_key.clone() {
        Some(key) => key,
        None => return Analytics::disabled(),
    };
    let distinct_id = match resolve_distinct_id() {
        Ok(id) => id,
        Err(error) => {
            eprintln!("analytics: could not resolve distinct id, disabling: {error:#}");
            return Analytics::disabled();
        }
    };
    match WorkerHandle::spawn(
        api_key,
        cfg.host.clone(),
        distinct_id,
        cfg.disable_geoip,
        shared_props,
    ) {
        Ok(handle) => {
            let sender = handle.sender.clone();
            Analytics {
                inner: Some(AnalyticsInner {
                    sender,
                    handle: Arc::new(handle),
                }),
            }
        }
        Err(error) => {
            eprintln!("analytics: could not spawn worker thread, disabling: {error:#}");
            Analytics::disabled()
        }
    }
}

fn resolve_distinct_id() -> anyhow::Result<Uuid> {
    let path = distinct_id::platform_default_path()?;
    distinct_id::load_or_create(&path)
}

fn app_started_system(analytics: Res<Analytics>) {
    analytics.track(Event::AppStarted);
}

/// Catch [`AppExit`] and flush the worker before the process exits. Bounded
/// to a few hundred ms inside the worker so quit never hangs.
fn app_quit_drain_system(
    analytics: Res<Analytics>,
    started: Res<AnalyticsAppStart>,
    mut exit: MessageReader<AppExit>,
) {
    if exit.read().next().is_none() {
        return;
    }
    let duration_s = started.0.elapsed().as_secs_f64();
    analytics.track(Event::AppQuit { duration_s });
    if let Some(inner) = analytics.inner.as_ref() {
        inner.handle.flush_and_join();
    }
}
