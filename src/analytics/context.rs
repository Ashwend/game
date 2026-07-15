//! Super properties, the hardware/OS/version snapshot that PostHog joins
//! onto every event. Captured once at startup, then read-only.
//!
//! Two-stage fill:
//! 1. [`SuperProps::initial`] runs before the worker thread is spawned and
//!    populates everything we can probe synchronously (OS, CPU, RAM, app
//!    version, build profile, locale, environment).
//! 2. [`fill_render_props_system`] runs as a Bevy `Update` system. The
//!    first time it observes [`RenderAdapterInfo`] and the primary
//!    [`Window`], it overlays the GPU/display keys and inserts the
//!    [`RenderPropsFilled`] sentinel so subsequent frames early-return.
//!
//! The same keys are also surfaced as PostHog Person properties via a
//! `$set` envelope in [`person_set`], the worker attaches that
//! to every event so hardware/OS info lives on the user's profile in
//! PostHog, not just on each event row.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::window::PrimaryWindow;
use serde_json::{Map, Value, json};

use super::config::Environment;

/// Shared, mutable super-property map. The worker reads it on flush; Bevy
/// startup systems write to it during early frames. Keep the critical
/// sections tiny.
pub(crate) type SharedProps = Arc<Mutex<Map<String, Value>>>;

const BEVY_VERSION: &str = "0.18.1";

pub(crate) struct SuperProps;

impl SuperProps {
    /// Synchronous probe done before plugin/worker startup. Cheap enough to
    /// run on the main thread, `os_info` and `sysinfo` together take a few
    /// ms and never panic.
    pub(crate) fn initial(environment: Environment) -> Map<String, Value> {
        let mut props = Map::new();

        let info = os_info::get();
        props.insert("$os".to_owned(), json!(info.os_type().to_string()));
        props.insert("$os_version".to_owned(), json!(info.version().to_string()));
        props.insert("$device_type".to_owned(), json!("Desktop"));
        props.insert("os_arch".to_owned(), json!(std::env::consts::ARCH));

        // `new_all` forces full refresh of CPU + memory in one call. The
        // earlier `new() + refresh_cpu_all() + refresh_memory()` form could
        // come back empty on some platforms, `new_all` is the documented
        // sysinfo way to get a complete snapshot.
        let system = sysinfo::System::new_all();
        let cpus = system.cpus();
        if let Some(cpu) = cpus.first() {
            let brand = cpu.brand().trim();
            if !brand.is_empty() {
                props.insert("cpu_brand".to_owned(), json!(brand));
            }
        }
        if !cpus.is_empty() {
            props.insert("cpu_cores".to_owned(), json!(cpus.len()));
        }
        let ram_mb = system.total_memory() / 1_048_576;
        if ram_mb > 0 {
            props.insert("ram_total_mb".to_owned(), json!(ram_mb));
        }

        props.insert("app_version".to_owned(), json!(env!("CARGO_PKG_VERSION")));
        props.insert("bevy_version".to_owned(), json!(BEVY_VERSION));
        props.insert(
            "build_profile".to_owned(),
            json!(if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }),
        );
        props.insert("environment".to_owned(), json!(environment.as_str()));

        if let Some(locale) = sys_locale::get_locale() {
            props.insert("locale".to_owned(), json!(locale));
        }

        props
    }
}

/// Keys that should also be promoted to PostHog Person properties via `$set`
/// so they're visible on the user's profile, not just per-event. Anything
/// not in this list stays event-only (e.g. environment, build_profile,
/// those legitimately differ between sessions and shouldn't overwrite the
/// Person row).
const PERSON_PROPERTY_KEYS: &[&str] = &[
    "$os",
    "$os_version",
    "$device_type",
    "os_arch",
    "cpu_brand",
    "cpu_cores",
    "ram_total_mb",
    "gpu_name",
    "gpu_backend",
    "display_resolution",
    "app_version",
    "bevy_version",
    "locale",
];

/// Builds the `$set` JSON object that promotes hardware/OS keys to the
/// PostHog Person profile. Called by the worker for every outgoing event so
/// the user's profile in PostHog always reflects their current hardware /
/// OS / app build, not just whatever was on the first event PostHog saw.
pub(crate) fn person_set(super_props: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for key in PERSON_PROPERTY_KEYS {
        if let Some(value) = super_props.get(*key) {
            out.insert((*key).to_owned(), value.clone());
        }
    }
    out
}

/// One-shot startup system: write GPU/display keys into the shared map the
/// first time the relevant Bevy resources exist, then mark itself done via
/// the [`RenderPropsFilled`] sentinel resource.
pub(crate) fn fill_render_props_system(
    mut commands: Commands,
    filled: Option<Res<RenderPropsFilled>>,
    shared: Option<Res<SuperPropsHandle>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    if filled.is_some() {
        return;
    }
    let Some(shared) = shared else {
        return;
    };

    // Only attempt the fill when at least one of the runtime sources is
    // ready. If neither has appeared yet, leave the system to fire again
    // on the next frame.
    if adapter.is_none() && primary_window.iter().next().is_none() {
        return;
    }

    let Ok(mut map) = shared.0.lock() else {
        return;
    };

    if let Some(adapter) = adapter.as_ref() {
        let name = adapter.name.trim();
        if !name.is_empty() {
            map.insert("gpu_name".to_owned(), json!(name));
        }
        map.insert(
            "gpu_backend".to_owned(),
            json!(format!("{:?}", adapter.backend)),
        );
    }

    if let Ok(window) = primary_window.single() {
        let res = window.physical_size();
        if res.x > 0 && res.y > 0 {
            map.insert(
                "display_resolution".to_owned(),
                json!(format!("{}x{}", res.x, res.y)),
            );
        }
    }

    drop(map);
    commands.insert_resource(RenderPropsFilled);
}

/// Marker that [`fill_render_props_system`] has run. Inserted on completion
/// so the system early-returns on subsequent frames.
#[derive(Resource)]
pub(crate) struct RenderPropsFilled;

/// Bevy resource handle around the shared super-property map. Cheap to
/// clone (it's an `Arc`).
#[derive(Resource, Clone)]
pub(crate) struct SuperPropsHandle(pub(crate) SharedProps);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_props_include_required_fields() {
        let props = SuperProps::initial(Environment::Dev);
        assert!(props.contains_key("$os"));
        assert!(props.contains_key("$device_type"));
        assert!(props.contains_key("app_version"));
        assert_eq!(props["environment"], json!("dev"));
        assert!(matches!(
            props["build_profile"].as_str(),
            Some("debug" | "release")
        ));
    }
}
