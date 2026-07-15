//! First-person sword slash "wind" trail.
//!
//! While the slash whips down-across the frame, a translucent wind ribbon
//! TRACES the blade: each frame it is rebuilt through the blade's actual
//! positions over the last stretch of the swing arc, so it always trails
//! behind the sword on the side the blade is moving away from, the cartoony
//! anime cut. The ribbon is anchored to the same pose math the sword renders
//! with ([`super::held::held_item_local_transform`] sampled at trailing swing
//! phases), so it can never desync from the blade, and it is pushed slightly
//! deeper than the sword so the opaque blade always occludes it (the wind
//! reads BEHIND the sword, never painted over it). Pure client feel: one
//! lazily-spawned camera-child entity on the viewmodel layer whose small mesh
//! is rewritten in place each frame. The window/fade lives in
//! [`slash_ribbon`], a pure function so the shape is unit-testable without
//! Bevy.

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{NoFrustumCulling, RenderLayers},
    light::NotShadowCaster,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::{
    app::{
        scene::{MainCamera, VIEWMODEL_RENDER_LAYER},
        state::{GatherInputState, MenuState, Screen},
    },
    items::{HeldMesh, ItemModel, item_definition},
};

use super::held::{RangedPoseInputs, held_item_local_transform};

/// The slash phase where the cut begins (just past `sword_swing_pose`'s
/// drawn-up apex key at 0.16): the ribbon's tail never reaches back into the
/// raise, so the draw-up leaves no wind.
const STRIKE_START_PHASE: f32 = 0.18;
/// The phase where the ribbon's head stops following the blade: right at the
/// spline's off-frame exit key (0.46), so the whole cross-screen cut feeds the
/// trail but the draw-back never drags the wind backward again.
const HEAD_FREEZE_PHASE: f32 = 0.48;
/// The phase by which the flash has fully left the viewport (and faded as a
/// backstop).
const FADE_END_PHASE: f32 = 0.66;
/// How much swing phase the ribbon spans behind the blade: the wind is a
/// short comet tail chasing the edge, not the whole arc at once.
const TAIL_SPAN_PHASE: f32 = 0.18;

/// Peak opacity at the ribbon's leading edge. Bright enough to flash for the
/// few frames of the cut, still translucent so the scene reads through it.
const TRAIL_PEAK_ALPHA: f32 = 0.55;

/// Ribbon stations sampled along the arc between tail and head each frame.
const RIBBON_STATIONS: usize = 24;

/// Blade-local anchor points (the sword glb's frame, blade along +Y). The
/// ribbon's inner edge rides mid-blade; the outer edge flares a little PAST
/// the authored tip (+0.35) so the wind licks beyond the edge.
const BLADE_ROOT_LOCAL: Vec3 = Vec3::new(0.0, 0.02, 0.0);
const BLADE_TIP_LOCAL: Vec3 = Vec3::new(0.0, 0.44, 0.0);

/// How far the ribbon is pushed away from the camera relative to the blade
/// path it samples, in view space. Keeps the alpha-blended wind strictly
/// deeper than the opaque sword, so the blade always draws in front of it.
const BEHIND_BLADE_OFFSET: f32 = 0.05;

/// Where the flash flies once the cut is done: after the blade exits, the
/// whole ribbon keeps travelling along the cut's direction (left and slightly
/// down, in camera space) until it is fully OUT of the viewport, instead of
/// fading in place mid-screen (owner spec: the flash must slash across and
/// out of the camera view). Far enough that even the trailing end clears the
/// frame edge at the ribbon's depth.
const EXIT_TRAVEL: Vec3 = Vec3::new(-2.8, -0.55, 0.0);

/// The ribbon's phase window at a slash `phase`: `None` outside the window,
/// otherwise the `(tail, head, envelope, exit)` to build the ribbon from.
/// `tail` and `head` are swing phases (the ribbon spans the blade's path
/// between them, head = where the blade is / was last); `envelope` is the
/// whole-ribbon opacity multiplier; `exit` ramps 0 -> 1 after the head
/// freezes and drives the flash's fly-off along [`EXIT_TRAVEL`], so the wind
/// finishes its slash clean out of the viewport rather than dissolving
/// mid-screen.
pub(crate) fn slash_ribbon(phase: f32) -> Option<(f32, f32, f32, f32)> {
    if phase <= STRIKE_START_PHASE || phase >= FADE_END_PHASE {
        return None;
    }
    let head = phase.min(HEAD_FREEZE_PHASE);
    let tail = (head - TAIL_SPAN_PHASE).max(STRIKE_START_PHASE);
    if head - tail < 1e-4 {
        return None;
    }
    let exit = ((phase - HEAD_FREEZE_PHASE) / (FADE_END_PHASE - HEAD_FREEZE_PHASE)).clamp(0.0, 1.0);
    // The exit motion is what removes the flash; the envelope only softens it
    // near the very end as a backstop against any sliver still in frame.
    let envelope = 1.0 - exit * exit * 0.6;
    Some((tail, head, envelope, exit))
}

/// The camera-local blade root and tip at a given swing `phase`: the same
/// whole-item transform the sword renders with (minus the tiny idle sway),
/// pushed [`BEHIND_BLADE_OFFSET`] deeper so the ribbon depth-tests behind the
/// blade.
fn blade_points(held_mesh: HeldMesh, phase: f32) -> (Vec3, Vec3) {
    let transform = held_item_local_transform(
        ItemModel::Sword,
        held_mesh,
        phase,
        1.0,
        RangedPoseInputs::default(),
    );
    let push = Vec3::NEG_Z * BEHIND_BLADE_OFFSET;
    (
        transform.transform_point(BLADE_ROOT_LOCAL) + push,
        transform.transform_point(BLADE_TIP_LOCAL) + push,
    )
}

/// Build the ribbon's vertex positions and colors for the current frame: one
/// station per sampled phase between `tail` and `head`, two vertices per
/// station (blade root edge, blade tip edge). Alpha ramps from 0 at the tail
/// to `envelope * TRAIL_PEAK_ALPHA` at the head, and the root edge is fainter
/// than the tip edge (the tip travels fastest, so the wind is strongest out at
/// the edge). Returned as flat attribute vectors ready to write into the mesh.
fn ribbon_attributes(
    held_mesh: HeldMesh,
    tail: f32,
    head: f32,
    envelope: f32,
) -> (Vec<[f32; 3]>, Vec<[f32; 4]>) {
    let mut positions = Vec::with_capacity(RIBBON_STATIONS * 2);
    let mut colors = Vec::with_capacity(RIBBON_STATIONS * 2);
    for i in 0..RIBBON_STATIONS {
        let t = i as f32 / (RIBBON_STATIONS - 1) as f32;
        let phase = tail + (head - tail) * t;
        let (root, tip) = blade_points(held_mesh, phase);
        positions.push(root.to_array());
        positions.push(tip.to_array());
        // Comet fade toward the tail; the head edge carries the peak.
        let alpha = envelope * TRAIL_PEAK_ALPHA * t * t;
        colors.push([1.0, 1.0, 1.0, alpha * 0.55]);
        colors.push([1.0, 1.0, 1.0, alpha]);
    }
    (positions, colors)
}

/// The initial (hidden) ribbon mesh: correct topology, degenerate geometry.
/// The system rewrites its position/color attributes in place every visible
/// frame.
fn ribbon_mesh() -> Mesh {
    let positions = vec![[0.0f32, 0.0, 0.0]; RIBBON_STATIONS * 2];
    let normals = vec![[0.0f32, 0.0, 1.0]; RIBBON_STATIONS * 2];
    let colors = vec![[1.0f32, 1.0, 1.0, 0.0]; RIBBON_STATIONS * 2];
    let mut indices = Vec::with_capacity((RIBBON_STATIONS - 1) * 6);
    for i in 0..(RIBBON_STATIONS as u32 - 1) {
        let a = i * 2;
        indices.extend_from_slice(&[a, a + 1, a + 2, a + 2, a + 1, a + 3]);
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U32(indices))
}

/// The lazily-created trail entity plus its dedicated mesh instance (rewritten
/// in place each visible frame, so it must not share a handle with anything).
pub(crate) struct SlashTrailHandles {
    entity: Entity,
    mesh: Handle<Mesh>,
}

/// Drive the sword's slash trail each frame: rebuilt through the blade's real
/// path while a slash is mid-window, hidden otherwise. The entity is spawned
/// lazily on first use as a child of the main camera (the viewmodel layer
/// draws over the finished frame, exactly like the held item; the ribbon's
/// vertices are authored directly in camera space).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sword_slash_trail_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    menu: Res<MenuState>,
    local_player: Res<crate::app::state::LocalPlayerState>,
    gather_input: Res<GatherInputState>,
    camera: Query<Entity, With<MainCamera>>,
    mut trail: Local<Option<SlashTrailHandles>>,
    mut trail_entity: Query<(&mut Visibility, &mut Transform)>,
) {
    // The trail exists only while a sword slash is mid-window, with the same
    // screen/overlay gating as the held item itself.
    let held_sword =
        (menu.screen == Screen::InGame && !menu.pause_open && !menu.panel_overlay_open())
            .then(|| {
                local_player
                    .private
                    .as_ref()
                    .and_then(|private| private.inventory.active_actionbar_stack())
                    .and_then(|stack| item_definition(&stack.item_id))
                    .filter(|definition| definition.model == ItemModel::Sword)
                    .map(|definition| definition.held_mesh)
            })
            .flatten();
    let ribbon = held_sword
        .map(|held_mesh| (held_mesh, slash_ribbon(gather_input.swing_fraction())))
        .and_then(|(held_mesh, ribbon)| ribbon.map(|r| (held_mesh, r)));

    let Some((held_mesh, (tail, head, envelope, exit))) = ribbon else {
        // Out of the window (or no sword): hide the trail if it exists.
        if let Some(handles) = trail.as_ref()
            && let Ok((mut visibility, _)) = trail_entity.get_mut(handles.entity)
        {
            *visibility = Visibility::Hidden;
        }
        return;
    };

    let handles = match trail.as_mut() {
        Some(handles) => handles,
        None => {
            let Ok(camera_entity) = camera.single() else {
                return;
            };
            let mesh = meshes.add(ribbon_mesh());
            let material = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                ..default()
            });
            let entity = commands
                .spawn((
                    Name::new("Sword Slash Trail"),
                    ChildOf(camera_entity),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    RenderLayers::layer(VIEWMODEL_RENDER_LAYER),
                    NotShadowCaster,
                    // The ribbon's vertices are rewritten every frame, but the
                    // entity's Aabb is computed once from the initial
                    // (degenerate) mesh and never refreshed, so frustum
                    // culling would drop it permanently. It only exists for a
                    // few frames mid-slash, right in front of the camera:
                    // skip culling outright.
                    NoFrustumCulling,
                ))
                .id();
            *trail = Some(SlashTrailHandles { entity, mesh });
            trail.as_mut().expect("just set")
        }
    };

    if let Some(mut mesh) = meshes.get_mut(&handles.mesh) {
        let (positions, colors) = ribbon_attributes(held_mesh, tail, head, envelope);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
    if let Ok((mut visibility, mut transform)) = trail_entity.get_mut(handles.entity) {
        *visibility = Visibility::Visible;
        // Fly-off: once the cut is done the whole flash keeps travelling along
        // the cut direction, accelerating out of the viewport.
        *transform = Transform::from_translation(EXIT_TRAVEL * (exit * exit));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ribbon_is_hidden_outside_the_slash_window() {
        assert!(slash_ribbon(0.0).is_none(), "no wind at guard");
        assert!(slash_ribbon(0.15).is_none(), "no wind during the raise");
        assert!(
            slash_ribbon(0.70).is_none(),
            "no wind once the draw-back is underway"
        );
        assert!(slash_ribbon(1.0).is_none(), "no wind back at rest");
    }

    #[test]
    fn ribbon_head_follows_the_blade_then_flies_out_of_frame() {
        let (tail, head, envelope, exit) = slash_ribbon(0.30).expect("wind mid-cut");
        assert!(
            (head - 0.30).abs() < 1e-6,
            "mid-cut the head rides the blade's current phase"
        );
        assert!(
            tail >= STRIKE_START_PHASE && tail < head,
            "the tail trails behind the head, never into the wind-up"
        );
        assert!(
            (envelope - 1.0).abs() < 1e-6,
            "full strength through the cut"
        );
        assert_eq!(exit, 0.0, "no fly-off while the blade still leads the wind");

        let (_, frozen_head, _, early_exit) = slash_ribbon(0.52).expect("wind exiting");
        assert!(
            (frozen_head - HEAD_FREEZE_PHASE).abs() < 1e-6,
            "past contact the head freezes instead of tracing the draw-back"
        );
        assert!(
            early_exit > 0.0 && early_exit < 1.0,
            "the flash is mid fly-off, got {early_exit}"
        );
        let (_, _, _, late_exit) = slash_ribbon(0.64).expect("wind nearly out");
        assert!(
            late_exit > 0.85,
            "just before the window closes the flash has flown (almost) fully out, got {late_exit}"
        );
    }

    #[test]
    fn ribbon_traces_the_blade_toward_the_lower_left() {
        // The ribbon must follow the blade through the cut, and what matters
        // is where the blade READS on screen, so compare perspective-projected
        // positions (x/-z, y/-z): view-space X alone is misleading because the
        // strike also drives the blade deep into the scene. Early in the
        // strike the blade sits upper-right of where it ends up, so the traced
        // path travels LEFT and DOWN across the frame, the wind trailing on
        // the up-right side the blade moves away from.
        let held_mesh = item_definition("iron_sword")
            .expect("sword registered")
            .held_mesh;
        let project = |p: Vec3| Vec2::new(p.x / -p.z, p.y / -p.z);
        let (early_root, early_tip) = blade_points(held_mesh, 0.19);
        let (late_root, late_tip) = blade_points(held_mesh, 0.42);
        for (early, late, edge) in [
            (project(early_root), project(late_root), "root"),
            (project(early_tip), project(late_tip), "tip"),
        ] {
            assert!(
                early.x > late.x + 0.3,
                "the traced {edge} path sweeps right-to-left on screen: {} -> {}",
                early.x,
                late.x
            );
            assert!(
                early.y > late.y + 0.5,
                "the traced {edge} path drops toward the bottom on screen: {} -> {}",
                early.y,
                late.y
            );
        }
    }

    #[test]
    fn ribbon_alpha_peaks_at_the_head_and_dies_at_the_tail() {
        let held_mesh = item_definition("iron_sword")
            .expect("sword registered")
            .held_mesh;
        let (positions, colors) = ribbon_attributes(held_mesh, 0.20, 0.36, 1.0);
        assert_eq!(positions.len(), RIBBON_STATIONS * 2);
        assert_eq!(colors.len(), RIBBON_STATIONS * 2);
        let tail_alpha = colors[1][3];
        let head_alpha = colors[colors.len() - 1][3];
        assert!(tail_alpha < 0.01, "the tail end is transparent");
        assert!(
            (head_alpha - TRAIL_PEAK_ALPHA).abs() < 1e-4,
            "the head edge carries the peak alpha"
        );
        let head_root_alpha = colors[colors.len() - 2][3];
        assert!(
            head_root_alpha < head_alpha,
            "the root edge is fainter than the fast-moving tip edge"
        );
    }
}
