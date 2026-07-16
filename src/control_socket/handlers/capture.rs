//! Observation handlers: the screenshot capture and the `dump_state` JSON
//! snapshot an agent asserts against.

use std::path::PathBuf;

use anyhow::Result;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use super::HandlerContext;
use crate::app::state::{ClientRuntime, LocalPlayerState, MenuState};
use crate::control_socket::wire::{ClientStateDump, DeployableDump, PlayerDump};

pub(super) fn screenshot(ctx: &mut HandlerContext, path: PathBuf) -> Result<String> {
    // In headless-capture mode the primary camera renders to an
    // off-screen image (the window is hidden), so screenshot that image;
    // otherwise read the live window framebuffer as before.
    let screenshot = match ctx.capture {
        Some(capture) => Screenshot::image(capture.image.clone()),
        None => Screenshot::primary_window(),
    };
    ctx.commands
        .spawn(screenshot)
        .observe(save_to_disk(path.clone()));
    Ok(format!(
        "screenshot queued to {} (lands within a frame or two)",
        path.display()
    ))
}

pub(super) fn dump_state(ctx: &mut HandlerContext) -> Result<String> {
    let dump = build_dump(
        ctx.runtime,
        ctx.menu,
        ctx.local_player,
        ctx.placement,
        ctx.deployables,
    );
    Ok(serde_json::to_string(&dump)?)
}

fn build_dump(
    runtime: &ClientRuntime,
    menu: &MenuState,
    local_player: &LocalPlayerState,
    placement: &crate::app::state::DeployablePlacementState,
    deployables: &[DeployableDump],
) -> ClientStateDump {
    let view = runtime.local_view();
    ClientStateDump {
        ghost_position: placement.world_position.map(|p| [p.x, p.y, p.z]),
        ghost_valid: placement.valid,
        client_id: runtime.client_id,
        is_admin: runtime.is_admin,
        world_loaded: runtime.world.is_some(),
        world_version: runtime.world_version,
        in_world: runtime.client_id.is_some()
            && runtime.world.is_some()
            && local_player.entity.is_some(),
        private_present: local_player.private.is_some(),
        screen: format!("{:?}", menu.screen),
        inventory_open: menu.inventory_open,
        crafting_open: menu.crafting_open,
        furnace_open: menu.furnace_open,
        loot_bag_open: menu.loot_bag_open,
        pause_open: menu.pause_open,
        chat_open: menu.chat_open,
        death_splash: menu.death_splash.is_some(),
        position: view.map(|v| [v.position.x, v.position.y, v.position.z]),
        yaw: view.map(|v| v.yaw),
        pitch: view.map(|v| v.pitch),
        health: view.map(|v| v.health),
        local_ping_ms: runtime.local_ping_ms,
        players: runtime
            .players
            .iter()
            .map(|p| PlayerDump {
                client_id: p.client_id,
                name: p.name.clone(),
                ping_ms: p.ping_ms,
            })
            .collect(),
        deployables: deployables.to_vec(),
        meteor_world: tracked_meteor_state(runtime)
            .map(|state| [state.position.x, state.position.y, state.position.z]),
        meteor_velocity: tracked_meteor_state(runtime)
            .map(|state| [state.velocity.x, state.velocity.y, state.velocity.z]),
        meteor_shower_impact: tracked_meteor(runtime).map(|event| {
            [
                event.impact_position.x,
                event.impact_position.y,
                event.impact_position.z,
            ]
        }),
    }
}

/// The shower meteor the dump tracks: the next-to-impact live one, preferring
/// meteors that have NOT yet landed (so the camera target moves on to the next
/// strike instead of staying pinned to the first crater for its whole despawn
/// window), falling back to the earliest crater when everything has landed.
fn tracked_meteor(runtime: &ClientRuntime) -> Option<&crate::app::state::MeteorShowerEvent> {
    let now = runtime.server_tick();
    runtime
        .meteor_showers
        .iter()
        .min_by_key(|event| (event.has_impacted(now), event.impact_tick))
}

/// The tracked meteor's in-flight world state, preferring the most imminent
/// meteor that is actually inside its flight window this frame.
fn tracked_meteor_state(runtime: &ClientRuntime) -> Option<crate::world::MeteorWorldState> {
    let now = runtime.server_tick_precise();
    let mut in_flight: Vec<(
        &crate::app::state::MeteorShowerEvent,
        crate::world::MeteorWorldState,
    )> = runtime
        .meteor_showers
        .iter()
        .filter_map(|event| {
            crate::world::meteor_world_state(
                bevy::math::Vec2::new(event.impact_position.x, event.impact_position.z),
                event.impact_tick,
                event.trajectory_seed,
                now,
            )
            .map(|state| (event, state))
        })
        .collect();
    in_flight.sort_by_key(|(event, _)| event.impact_tick);
    in_flight.into_iter().next().map(|(_, state)| state)
}
