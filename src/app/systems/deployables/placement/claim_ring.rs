//! Claim-boundary ring VFX: while the building plan is held, every nearby
//! Tool Cupboard claim is traced with a translucent gizmo ring, green where
//! the local player is authorized to build and red where they are not. Pure
//! presentation: the boundary it draws is the same shared claim-footprint
//! geometry the server gates placement on.

use bevy::prelude::*;

use crate::{
    app::state::{BuildingPlanState, ClientRuntime, CurrentUser, LocalPlayerState, MenuState},
    building::{
        ClaimPlatform, FOUNDATION_SIZE_M, claim_cell_of, claim_footprint_cells, platform_top_offset,
    },
    game_balance::BUILDING_PRIVILEGE_MARGIN_CELLS,
    items::DeployableKind,
    server::{Deployable, DeployableAuth, DeployableTransform},
};

use super::{GhostIntent, current_ghost_intent};

/// Height of the territory boundary fade, in metres. Short enough to frame
/// a base without dominating the view.
const CLAIM_RING_HEIGHT_M: f32 = 2.5;
/// How often the boundary is recomputed (it only moves when the base or
/// authorization changes, not as the player walks); redrawn every frame
/// from the cached segments in between.
const CLAIM_RING_RECOMPUTE_SECS: f32 = 0.25;
/// Only ring cupboards within this distance of the player, so a view full
/// of bases doesn't flood the screen.
const CLAIM_RING_MAX_DISTANCE_M: f32 = 60.0;
/// Stacked line segments approximating the vertical alpha gradient
/// (matches the chunk dev overlay's fade).
const CLAIM_RING_FADE_SEGMENTS: u32 = 12;
/// Peak alpha at the floor; the fade tapers to fully transparent at the top.
const CLAIM_RING_BASE_ALPHA: f32 = 0.5;
/// Y of the bright floor line, just above ground to dodge z-fighting.
const CLAIM_RING_FLOOR_Y: f32 = 0.03;

/// One drawn boundary segment: a 3 m floor span on a claim cell edge plus
/// whether the local player is authorized at that claim (green) or not
/// (red).
pub(crate) struct ClaimBoundarySegment {
    floor_start: Vec3,
    floor_end: Vec3,
    authorized: bool,
}

/// Draw a translucent boundary ring around each nearby Tool Cupboard claim
/// while the building plan is held: green where the local player may build
/// (authorized), red where they may not. The ring traces the actual
/// foundation-projected claim cells (the same shared geometry the server
/// gates on), and each segment fades from solid at the floor to fully
/// transparent at the top, like the chunk dev overlay. Drawn with gizmos
/// (no entities): recomputed on a throttle, redrawn from the cache every
/// frame.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn update_claim_boundary_system(
    mut gizmos: Gizmos,
    time: Res<Time>,
    plan: Res<BuildingPlanState>,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    runtime: Res<ClientRuntime>,
    claim_query: Query<(&Deployable, &DeployableTransform, &DeployableAuth)>,
    user: Option<Res<CurrentUser>>,
    mut cached: Local<Vec<ClaimBoundarySegment>>,
    mut since_recompute: Local<f32>,
) {
    let holding_plan = matches!(
        current_ghost_intent(&local_player, &menu, &plan),
        Some(GhostIntent::Building(_))
    );
    if !holding_plan {
        cached.clear();
        // Force an immediate recompute the next time the plan comes out.
        *since_recompute = CLAIM_RING_RECOMPUTE_SECS;
        return;
    }

    *since_recompute += time.delta_secs().max(0.0);
    if *since_recompute >= CLAIM_RING_RECOMPUTE_SECS {
        *since_recompute = 0.0;
        let account = user.as_ref().map(|user| user.0.account_id);
        *cached = compute_claim_boundary_segments(&claim_query, &runtime, account);
    }

    for segment in cached.iter() {
        draw_claim_boundary_segment(&mut gizmos, segment);
    }
}

/// Build the boundary segments for every nearby claim: trace each
/// cupboard's footprint cell edges, tagged green/red by the local player's
/// authorization at that cupboard.
fn compute_claim_boundary_segments(
    claim_query: &Query<(&Deployable, &DeployableTransform, &DeployableAuth)>,
    runtime: &ClientRuntime,
    account: Option<crate::protocol::AccountId>,
) -> Vec<ClaimBoundarySegment> {
    let mut segments = Vec::new();
    let Some(player) = runtime.local_view() else {
        return segments;
    };
    let (px, pz) = (player.position.x, player.position.z);

    let platforms: Vec<ClaimPlatform> = claim_query
        .iter()
        .filter_map(|(meta, transform, _)| {
            let DeployableKind::Building { piece, .. } = meta.kind else {
                return None;
            };
            let top = platform_top_offset(piece)?;
            Some(ClaimPlatform {
                position: transform.position,
                top: transform.position.y + top,
            })
        })
        .collect();

    const HALF: f32 = FOUNDATION_SIZE_M / 2.0;
    for (meta, transform, auth) in claim_query {
        if !matches!(meta.kind, DeployableKind::ToolCupboard) {
            continue;
        }
        let dx = transform.position.x - px;
        let dz = transform.position.z - pz;
        if dx * dx + dz * dz > CLAIM_RING_MAX_DISTANCE_M * CLAIM_RING_MAX_DISTANCE_M {
            continue;
        }
        let authorized = account.is_some_and(|account| auth.0.contains(&account));

        // Footprint cells keyed by grid cell (exact neighbour lookups)
        // with the real XZ centre for positioning.
        let mut cells: std::collections::HashMap<(i32, i32), (f32, f32)> =
            std::collections::HashMap::new();
        for (cx, cz) in claim_footprint_cells(
            &platforms,
            transform.position,
            BUILDING_PRIVILEGE_MARGIN_CELLS,
        ) {
            cells.insert(claim_cell_of(cx, cz), (cx, cz));
        }

        for (cell, (rx, rz)) in &cells {
            for (sdx, sdz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                if cells.contains_key(&(cell.0 + sdx, cell.1 + sdz)) {
                    continue;
                }
                let edge_x = rx + sdx as f32 * HALF;
                let edge_z = rz + sdz as f32 * HALF;
                // The edge runs perpendicular to its outward normal.
                let (perp_x, perp_z) = (sdz as f32, sdx as f32);
                segments.push(ClaimBoundarySegment {
                    floor_start: Vec3::new(
                        edge_x - perp_x * HALF,
                        CLAIM_RING_FLOOR_Y,
                        edge_z - perp_z * HALF,
                    ),
                    floor_end: Vec3::new(
                        edge_x + perp_x * HALF,
                        CLAIM_RING_FLOOR_Y,
                        edge_z + perp_z * HALF,
                    ),
                    authorized,
                });
            }
        }
    }
    segments
}

/// Draw one boundary segment: a bright floor line plus vertical fades at
/// its ends and midpoint that taper to transparent at the top.
fn draw_claim_boundary_segment(gizmos: &mut Gizmos, segment: &ClaimBoundarySegment) {
    let floor_alpha = (CLAIM_RING_BASE_ALPHA + 0.15).min(1.0);
    gizmos.line(
        segment.floor_start,
        segment.floor_end,
        claim_ring_color(segment.authorized, floor_alpha),
    );
    let mid = segment.floor_start.lerp(segment.floor_end, 0.5);
    for base in [segment.floor_start, mid, segment.floor_end] {
        draw_claim_ring_fade(gizmos, base, segment.authorized);
    }
}

/// Stack fading vertical line segments from `base` upward with a quadratic
/// falloff, so it reads as solid at the ground fading to nothing at the top.
fn draw_claim_ring_fade(gizmos: &mut Gizmos, base: Vec3, authorized: bool) {
    let seg_h = CLAIM_RING_HEIGHT_M / CLAIM_RING_FADE_SEGMENTS as f32;
    for seg in 0..CLAIM_RING_FADE_SEGMENTS {
        let t = seg as f32 / CLAIM_RING_FADE_SEGMENTS as f32;
        let alpha = (1.0 - t).powi(2) * CLAIM_RING_BASE_ALPHA;
        let y0 = base.y + seg as f32 * seg_h;
        let y1 = base.y + (seg + 1) as f32 * seg_h;
        gizmos.line(
            Vec3::new(base.x, y0, base.z),
            Vec3::new(base.x, y1, base.z),
            claim_ring_color(authorized, alpha),
        );
    }
}

fn claim_ring_color(authorized: bool, alpha: f32) -> Color {
    let (r, g, b) = if authorized {
        (0.30, 0.90, 0.42)
    } else {
        (0.96, 0.32, 0.32)
    };
    Color::srgba(r, g, b, alpha.clamp(0.0, 1.0))
}
