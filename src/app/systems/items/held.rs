use std::f32::consts::PI;

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{HeldItemVisual, ItemVisualAssets, MainCamera},
        state::{GatherInputState, LocalPlayerState, MenuState, Screen, ToolSwapState},
    },
    items::{HeldMesh, ItemModel, item_definition},
};

use super::swing_poses::{
    ToolSwingPose, bag_idle_pose, hatchet_swing_pose, pickaxe_swing_pose, smoothstep,
};

const HELD_ITEM_FORWARD_OFFSET: f32 = 0.62;
const HELD_ITEM_RIGHT_OFFSET: f32 = 0.28;
const HELD_ITEM_DOWN_OFFSET: f32 = 0.24;

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_held_item_visual_system(
    mut commands: Commands,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    assets: Res<ItemVisualAssets>,
    gather_input: Res<GatherInputState>,
    swap_state: Res<ToolSwapState>,
    time: Res<Time>,
    camera: Query<Entity, With<MainCamera>>,
    held: Query<(Entity, &HeldItemVisual)>,
) {
    // Dead players don't render a held item. The corpse drops its
    // loot into a bag at the death position; the camera fades to
    // black while the death splash is up, and a visible weapon on
    // screen during that fade reads as a UI bug.
    let local_dead = matches!(
        local_player.lifecycle,
        Some(crate::server::PlayerLifecycle::Dead { .. })
    );
    let active_item = (menu.screen == Screen::InGame && !menu.pause_open && !local_dead)
        .then(|| {
            local_player
                .private
                .as_ref()
                .and_then(|private| private.inventory.active_actionbar_stack())
                .and_then(|stack| {
                    item_definition(&stack.item_id)
                        .map(|definition| (stack.item_id.clone(), definition))
                })
        })
        .flatten();

    let Some((item_id, definition)) = active_item.filter(|(_, definition)| definition.equipable)
    else {
        for (entity, _) in &held {
            commands.entity(entity).despawn();
        }
        return;
    };

    let Ok(camera_entity) = camera.single() else {
        return;
    };
    let transform = apply_idle_sway(
        held_item_local_transform(
            definition.model,
            gather_input.swing_fraction(),
            swap_state.fraction(),
        ),
        time.elapsed_secs(),
    );
    // The held item renders as one or more material layers sharing a single
    // swing transform: most items are a single layer, iron tools are two (a
    // matte handle body + a shiny iron head). Each layer is its own
    // camera-child entity tagged with the active `item_id`.
    //
    // Steady state (layers already match the active item): just drive the
    // swing/swap transform onto each layer, the cheap per-frame path. The
    // hierarchy/mesh/material are only (re)built when the held item changes,
    // so we don't retrigger change-detection or hierarchy fix-ups every frame.
    let held_entities: Vec<Entity> = held.iter().map(|(entity, _)| entity).collect();
    let layers_match_item =
        !held_entities.is_empty() && held.iter().all(|(_, visual)| visual.item_id == item_id);

    if layers_match_item {
        for entity in held_entities {
            commands.entity(entity).insert(transform);
        }
        return;
    }

    // Held item changed: tear down the old layers and rebuild for the new one.
    for entity in held_entities {
        commands.entity(entity).despawn();
    }
    for (mesh, material) in held_item_layers(&assets, definition.held_mesh) {
        commands.spawn((
            Name::new("Held Item"),
            HeldItemVisual {
                item_id: item_id.clone(),
            },
            ChildOf(camera_entity),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
            Visibility::Visible,
            // Held items sit right in front of the camera; their shadow would
            // slash across the floor like a phantom player and dominate the
            // frame. Skip the shadow pass.
            NotShadowCaster,
        ));
    }
}

/// Mesh + material layers that make up the in-hand visual for `held_mesh`.
/// One entry for single-material items; two for the authored tool glbs (stone
/// and iron), whose matte haft body and worked head need different materials
/// (Bevy binds one material per mesh). Layers share the mesh-local frame so they
/// overlay exactly under the same swing transform.
fn held_item_layers(
    assets: &ItemVisualAssets,
    held_mesh: HeldMesh,
) -> Vec<(Handle<Mesh>, Handle<StandardMaterial>)> {
    match held_mesh {
        // The bag silhouette covers raw materials and deployables-in-hand,
        // the structure mesh is what gets dropped into the world on
        // placement, not what's held.
        HeldMesh::Bag => vec![(
            assets.held_bag_mesh.clone(),
            assets.held_bag_material.clone(),
        )],
        // Every tool tier is an authored glb carrying its own two materials
        // (matte haft + worked head), drawn as a body layer plus a head layer.
        HeldMesh::StoneHatchet => vec![
            (
                assets.held_stone_hatchet_body_mesh.clone(),
                assets.held_stone_hatchet_body_material.clone(),
            ),
            (
                assets.held_stone_hatchet_head_mesh.clone(),
                assets.held_stone_hatchet_head_material.clone(),
            ),
        ],
        HeldMesh::StonePickaxe => vec![
            (
                assets.held_stone_pickaxe_body_mesh.clone(),
                assets.held_stone_pickaxe_body_material.clone(),
            ),
            (
                assets.held_stone_pickaxe_head_mesh.clone(),
                assets.held_stone_pickaxe_head_material.clone(),
            ),
        ],
        HeldMesh::IronHatchet => vec![
            (
                assets.held_iron_hatchet_body_mesh.clone(),
                assets.held_iron_hatchet_body_material.clone(),
            ),
            (
                assets.held_iron_hatchet_head_mesh.clone(),
                assets.held_iron_hatchet_head_material.clone(),
            ),
        ],
        HeldMesh::IronPickaxe => vec![
            (
                assets.held_iron_pickaxe_body_mesh.clone(),
                assets.held_iron_pickaxe_body_material.clone(),
            ),
            (
                assets.held_iron_pickaxe_head_mesh.clone(),
                assets.held_iron_pickaxe_head_material.clone(),
            ),
        ],
    }
}

/// Very subtle passive sway layered on top of the rest/swing pose so the
/// viewmodel breathes instead of sitting perfectly still. Two slow sine waves
/// at incommensurate frequencies (a gentle Lissajous) avoid a robotic
/// back-and-forth, and the amplitudes are deliberately tiny, a few millimetres
/// of drift and well under a degree of tilt, so it reads as "alive" without
/// ever competing with the swing or the aim. Always applied; during a swing
/// the much larger swing motion swamps it.
fn apply_idle_sway(transform: Transform, t: f32) -> Transform {
    let drift = Vec3::new(
        (t * 0.9).sin() * 0.0055,
        (t * 1.3 + 0.7).sin() * 0.0040,
        0.0,
    );
    let tilt = Quat::from_euler(
        EulerRot::XYZ,
        (t * 1.1).sin() * 0.0090,
        (t * 0.8 + 1.3).sin() * 0.0110,
        (t * 1.0 + 0.4).sin() * 0.0070,
    );
    Transform {
        translation: transform.translation + drift,
        rotation: tilt * transform.rotation,
        scale: transform.scale,
    }
}

fn held_item_local_transform(
    model: ItemModel,
    swing_fraction: f32,
    swap_fraction: f32,
) -> Transform {
    let phase = swing_fraction.clamp(0.0, 1.0);
    let model_down_offset = match model {
        ItemModel::Bag | ItemModel::Deployable => HELD_ITEM_DOWN_OFFSET,
        ItemModel::Hatchet | ItemModel::Pickaxe => HELD_ITEM_DOWN_OFFSET - 0.03,
    };

    let (pose, model_rotation): (ToolSwingPose, Quat) = match model {
        ItemModel::Bag | ItemModel::Deployable => (bag_idle_pose(phase), Quat::IDENTITY),
        ItemModel::Hatchet => (hatchet_swing_pose(phase), Quat::from_rotation_y(PI * 0.5)),
        ItemModel::Pickaxe => (pickaxe_swing_pose(phase), Quat::from_rotation_y(PI * 0.5)),
    };

    let swing_translation = Vec3::NEG_Z * pose.forward + Vec3::X * pose.right + Vec3::Y * pose.up;
    let base_rotation = Quat::from_euler(EulerRot::XYZ, pose.pitch, pose.yaw, pose.roll);
    let base_translation = Vec3::NEG_Z * HELD_ITEM_FORWARD_OFFSET
        + Vec3::X * HELD_ITEM_RIGHT_OFFSET
        - Vec3::Y * model_down_offset
        + swing_translation;
    let base_quat = base_rotation * model_rotation;

    // Entry animation: the tool is "picked off the player's back", it
    // starts below the rest pose and slightly tilted forward, then eases up
    // into place. Heavier items (pickaxe) drop further and tilt more so the
    // lift reads as weightier without being noticeably slower.
    let swap = swap_fraction.clamp(0.0, 1.0);
    let lag = 1.0 - smoothstep(swap);
    if lag <= f32::EPSILON {
        return Transform::from_translation(base_translation).with_rotation(base_quat);
    }

    let (drop, back, pitch_lag) = match model {
        ItemModel::Bag | ItemModel::Deployable => (0.40, 0.04, -0.30),
        ItemModel::Hatchet => (0.50, 0.05, -0.40),
        ItemModel::Pickaxe => (0.68, 0.06, -0.55),
    };

    let enter_offset = Vec3::new(0.0, -drop * lag, back * lag);
    let enter_tilt = Quat::from_rotation_x(pitch_lag * lag);
    Transform::from_translation(base_translation + enter_offset)
        .with_rotation(enter_tilt * base_quat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_swapped_in_tool_sits_at_its_rest_pose() {
        // swap_fraction == 1.0 means the tool has finished lifting into
        // view, so no enter-offset is applied, the transform is the
        // canonical rest pose for the model.
        let rest = held_item_local_transform(ItemModel::Hatchet, 0.0, 1.0);

        // The base rest translation sits forward (-Z), right (+X) and down.
        assert!(rest.translation.z < 0.0, "held item is in front of camera");
        assert!(rest.translation.x > 0.0, "held item offset to the right");
        assert!(rest.translation.y < 0.0, "held item offset downward");
    }

    #[test]
    fn entry_animation_drops_and_tilts_the_item_below_its_rest_pose() {
        // At swap_fraction == 0.0 the tool is freshly "picked off the
        // back", it starts lower than the rest pose.
        let entering = held_item_local_transform(ItemModel::Pickaxe, 0.0, 0.0);
        let rest = held_item_local_transform(ItemModel::Pickaxe, 0.0, 1.0);
        assert!(
            entering.translation.y < rest.translation.y,
            "entering item starts below rest"
        );
        // And it's tilted relative to rest.
        assert!(entering.rotation.angle_between(rest.rotation) > 0.05);
    }

    #[test]
    fn heavier_pickaxe_drops_further_on_entry_than_the_bag() {
        let pickaxe = held_item_local_transform(ItemModel::Pickaxe, 0.0, 0.0);
        let bag = held_item_local_transform(ItemModel::Bag, 0.0, 0.0);
        // The pickaxe's entry drop is the largest of the three models, so at
        // the start of the swap it sits lower than the bag.
        assert!(pickaxe.translation.y < bag.translation.y);
    }

    #[test]
    fn swing_phase_moves_the_held_item_relative_to_idle() {
        // A mid-swing phase displaces the hatchet from its idle (phase 0)
        // pose, the swing animation actually drives the transform.
        let idle = held_item_local_transform(ItemModel::Hatchet, 0.0, 1.0);
        let mid = held_item_local_transform(ItemModel::Hatchet, 0.5, 1.0);
        assert!(idle.translation.distance(mid.translation) > 0.01);
    }
}
