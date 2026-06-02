//! Debug overlay: draws the 64 m world-chunk boundaries around the
//! player as a set of fading vertical "walls". Pure visual aid, gated
//! behind the **Chunk Overlay** hud setting. No simulation impact.
//!
//! Implementation uses Bevy's gizmo system, a per-frame line stream
//! with no allocated entities, so toggling it on/off is instant and
//! there's nothing to clean up between frames.

use bevy::prelude::*;

use crate::{
    app::state::{ClientRuntime, ClientSettings, MenuState, Screen},
    world::CHUNK_SIZE_M,
};

/// How many concentric grids around the player to draw boundaries for.
/// Picked to overshoot the player's view tier so the player can always
/// see where the next boundary lies, even at High view distance.
const OVERLAY_RADIUS_GRIDS: i32 = 4;

/// Height of the vertical wall, tall enough to read at a glance but
/// well below the canopy of a large pine so the visualization doesn't
/// dominate the screen.
const OVERLAY_WALL_HEIGHT_M: f32 = 6.0;

/// Number of stacked line segments used per wall to approximate the
/// vertical alpha gradient. More segments = smoother fade, but each is
/// a separate gizmo line; this value is the sweet spot.
const OVERLAY_FADE_SEGMENTS: u32 = 16;

/// Floor accent, a flat line at the very bottom of every boundary
/// makes it obvious where the chunk edges are even when the player is
/// looking straight down. Drawn at this Y so it sits just above the
/// world floor without z-fighting.
const OVERLAY_FLOOR_Y: f32 = 0.02;

/// Base RGB tint for boundary walls. Cyan reads as "debug overlay"
/// rather than something diegetic the world cares about.
const OVERLAY_RGB: (f32, f32, f32) = (0.45, 0.85, 1.0);

/// Peak alpha at the floor for the wall gradient. Anything above this
/// makes the overlay too noisy when standing inside it.
const OVERLAY_BASE_ALPHA: f32 = 0.55;

/// Peak alpha for the bright floor accent line.
const OVERLAY_FLOOR_ALPHA: f32 = 0.65;

pub(crate) fn chunk_overlay_system(
    mut gizmos: Gizmos,
    runtime: Res<ClientRuntime>,
    settings: Res<ClientSettings>,
    menu: Res<MenuState>,
) {
    if !settings.hud.show_chunk_overlay {
        return;
    }
    if menu.screen != Screen::InGame {
        return;
    }
    let Some(player) = runtime.local_view() else {
        return;
    };

    let player_x = player.position.x;
    let player_z = player.position.z;
    let player_gx = (player_x / CHUNK_SIZE_M).floor() as i32;
    let player_gz = (player_z / CHUNK_SIZE_M).floor() as i32;

    let radius = OVERLAY_RADIUS_GRIDS;

    // For each vertical boundary (constant-x line) in range, draw:
    //  - a bright floor line spanning every cell of length CHUNK_SIZE_M
    //  - vertical fading wall segments stacked from the floor to OVERLAY_WALL_HEIGHT_M
    for dx in -radius..=radius + 1 {
        let x = (player_gx + dx) as f32 * CHUNK_SIZE_M;
        // Floor accent along this boundary, cell by cell, for the full
        // visible span.
        for dz in -radius..=radius {
            let z0 = (player_gz + dz) as f32 * CHUNK_SIZE_M;
            let z1 = z0 + CHUNK_SIZE_M;
            draw_floor_line(
                &mut gizmos,
                Vec3::new(x, OVERLAY_FLOOR_Y, z0),
                Vec3::new(x, OVERLAY_FLOOR_Y, z1),
            );
            // A wall segment in the middle of each cell edge gives the
            // overlay a sense of depth without filling the whole
            // boundary with vertical lines.
            let z_mid = z0 + CHUNK_SIZE_M * 0.5;
            draw_vertical_fade(&mut gizmos, Vec3::new(x, 0.0, z_mid));
        }
        // Plus a tall "post" at each chunk corner along this boundary so
        // the corners of the chunk are obvious.
        for dz in -radius..=radius + 1 {
            let z = (player_gz + dz) as f32 * CHUNK_SIZE_M;
            draw_vertical_fade(&mut gizmos, Vec3::new(x, 0.0, z));
        }
    }

    // Same dance for the perpendicular set of boundaries (constant-z
    // lines). Floor accents only, the corner posts were already drawn
    // by the loop above.
    for dz in -radius..=radius + 1 {
        let z = (player_gz + dz) as f32 * CHUNK_SIZE_M;
        for dx in -radius..=radius {
            let x0 = (player_gx + dx) as f32 * CHUNK_SIZE_M;
            let x1 = x0 + CHUNK_SIZE_M;
            draw_floor_line(
                &mut gizmos,
                Vec3::new(x0, OVERLAY_FLOOR_Y, z),
                Vec3::new(x1, OVERLAY_FLOOR_Y, z),
            );
            let x_mid = x0 + CHUNK_SIZE_M * 0.5;
            draw_vertical_fade(&mut gizmos, Vec3::new(x_mid, 0.0, z));
        }
    }
}

fn draw_floor_line(gizmos: &mut Gizmos, start: Vec3, end: Vec3) {
    gizmos.line(start, end, overlay_color(OVERLAY_FLOOR_ALPHA));
}

/// Stack `OVERLAY_FADE_SEGMENTS` line segments from `base` upward, each
/// with a slightly lower alpha than the previous so the wall reads as
/// "solid at the ground, fading into nothing" at the top.
fn draw_vertical_fade(gizmos: &mut Gizmos, base: Vec3) {
    let seg_h = OVERLAY_WALL_HEIGHT_M / OVERLAY_FADE_SEGMENTS as f32;
    for seg in 0..OVERLAY_FADE_SEGMENTS {
        let t = seg as f32 / OVERLAY_FADE_SEGMENTS as f32;
        // Quadratic falloff, more weight near the ground, faster fade
        // as we approach the top. Reads as a smooth gradient instead of
        // a flat ramp.
        let alpha = (1.0 - t).powi(2) * OVERLAY_BASE_ALPHA;
        let y0 = seg as f32 * seg_h;
        let y1 = (seg + 1) as f32 * seg_h;
        gizmos.line(
            Vec3::new(base.x, y0, base.z),
            Vec3::new(base.x, y1, base.z),
            overlay_color(alpha),
        );
    }
}

fn overlay_color(alpha: f32) -> Color {
    let (r, g, b) = OVERLAY_RGB;
    Color::srgba(r, g, b, alpha.clamp(0.0, 1.0))
}
