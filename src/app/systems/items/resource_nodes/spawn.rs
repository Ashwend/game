use bevy::{camera::visibility::VisibilityRange, light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{
            GrassMaterialHandle, ImpactEffectAssets, NetworkResourceNode, ResourceVisualAssets,
        },
        state::ImpactEffectKind,
        systems::effects::spawn_impact_burst,
    },
    protocol::{ResourceNodeId, Vec3Net},
    resources::ResourceNodeModel,
};

use super::{ResourceNodeEntities, ResourceNodePopIn};

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_resource_node_entity(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    grass_material: &GrassMaterialHandle,
    impact_assets: &ImpactEffectAssets,
    entities: &mut ResourceNodeEntities,
    id: ResourceNodeId,
    position: Vec3Net,
    model: ResourceNodeModel,
    target_transform: Transform,
    should_pop_in: bool,
) {
    let (mesh, material) = resource_node_visual(assets, model);
    let lod_mesh = tree_lod_mesh(assets, model);
    let mut spawn_command = commands.spawn((
        Name::new(format!("Resource Node {id}")),
        NetworkResourceNode { id, model },
        Mesh3d(mesh),
        target_transform,
        Visibility::Visible,
    ));
    // Hay grass renders with the swaying grass shader (the *same* material as
    // the cosmetic detail grass, so they move in unison); every other node uses
    // its matte `StandardMaterial`.
    if model == ResourceNodeModel::HayGrass {
        spawn_command.insert(MeshMaterial3d(grass_material.0.clone()));
    } else {
        spawn_command.insert(MeshMaterial3d(material.clone()));
    }
    // Crude clutter (branch piles, surface stones, hay grass) spawns
    // densely — Plains chunks alone carry ~28 grass tufts plus stones
    // and sticks — and each casts a negligible-size shadow under its
    // own footprint. Skipping the shadow pass for these gets us a
    // meaningful per-frame win in populated areas without losing
    // readable silhouettes (trees and veins still cast).
    if model.is_crude() {
        spawn_command.insert(NotShadowCaster);
    }
    if should_pop_in {
        spawn_command.insert(ResourceNodePopIn {
            elapsed: 0.0,
            base_transform: target_transform,
        });
    }
    // Trees get a distance LOD: this (full-detail) mesh switches off past
    // `TREE_LOD_DISTANCE`; a child carrying the low-poly mesh switches on at the
    // same distance. Bevy's `VisibilityRange` does this GPU-side off the
    // existing visibility pass — no per-frame CPU cost. It's a hard step, not a
    // dither crossfade (see `TREE_LOD_DISTANCE` for why). Cuts main-pass vertex
    // throughput across a forest of distant trees (shadow distance is bounded
    // separately by the Shadows graphics setting).
    if lod_mesh.is_some() {
        spawn_command.insert(tree_lod_high_range());
    }
    let entity = spawn_command.id();
    entities.entities.insert(id, entity);

    if let Some(lod_mesh) = lod_mesh {
        commands.entity(entity).with_children(|parent| {
            parent.spawn((
                Name::new(format!("Resource Node {id} LOD")),
                Mesh3d(lod_mesh),
                MeshMaterial3d(material),
                tree_lod_low_range(),
                Transform::default(),
                Visibility::Visible,
            ));
        });
    }

    if should_pop_in {
        spawn_pop_in_chip_burst(commands, impact_assets, id, position, model);
    }
}

/// Distance (m from camera) at which a tree switches from its full-detail mesh
/// to the low-poly LOD.
///
/// We deliberately use a **hard switch, not a dither crossfade**. Bevy's
/// `VisibilityRange` crossfade is a screen-space stochastic dither in the
/// fragment shader, and it misbehaves against this camera's post-process stack
/// (HDR + bloom + the fullscreen atmosphere pass + FXAA): inside the fade band
/// the dithered fragments get discarded, so a tree at ~LOD distance randomly
/// renders as nothing but its shadow. See the upstream reports
/// (<https://github.com/bevyengine/bevy/issues/17643>,
/// <https://github.com/bevyengine/bevy/pull/16286>). Zero-width margins below
/// disable the dither entirely; the trade-off is a small LOD pop at this
/// distance instead of a fade.
const TREE_LOD_DISTANCE: f32 = 80.0;

/// The low-poly LOD mesh for a tree model, or `None` for non-tree nodes.
fn tree_lod_mesh(assets: &ResourceVisualAssets, model: ResourceNodeModel) -> Option<Handle<Mesh>> {
    Some(match model {
        ResourceNodeModel::PineTreeSmall => assets.pine_tree_small_lod_mesh.clone(),
        ResourceNodeModel::PineTreeMedium => assets.pine_tree_medium_lod_mesh.clone(),
        ResourceNodeModel::PineTreeLarge => assets.pine_tree_large_lod_mesh.clone(),
        ResourceNodeModel::BirchTreeSmall => assets.birch_tree_small_lod_mesh.clone(),
        ResourceNodeModel::BirchTreeMedium => assets.birch_tree_medium_lod_mesh.clone(),
        ResourceNodeModel::BirchTreeLarge => assets.birch_tree_large_lod_mesh.clone(),
        _ => return None,
    })
}

/// `VisibilityRange` for the full-detail mesh: visible up close, hard cutoff at
/// `TREE_LOD_DISTANCE`. Zero-width margins = a step switch, no dither crossfade
/// (see [`TREE_LOD_DISTANCE`] for why we avoid the crossfade).
fn tree_lod_high_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: TREE_LOD_DISTANCE..TREE_LOD_DISTANCE,
        use_aabb: false,
    }
}

/// `VisibilityRange` for the low-poly mesh: hard switch-in at
/// `TREE_LOD_DISTANCE`, then visible out to the (well-beyond-far-plane) cutoff.
/// The switch-in distance is identical to the high mesh's cutoff so exactly one
/// LOD is visible at any distance — no gap, no overlap.
fn tree_lod_low_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: TREE_LOD_DISTANCE..TREE_LOD_DISTANCE,
        end_margin: 10_000.0..10_000.0,
        use_aabb: false,
    }
}

/// A short upward chip burst sells the "fresh from the ground" moment.
/// Trees throw wood chips, ores throw stone shards, crude nodes throw
/// their own small per-kind burst — same palette as gather impacts so
/// the visual language stays consistent.
fn spawn_pop_in_chip_burst(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    id: ResourceNodeId,
    position: Vec3Net,
    model: ResourceNodeModel,
) {
    let burst_anchor = Vec3::from(position) + Vec3::Y * 0.18;
    let kind = ImpactEffectKind::for_resource_model(model);
    let seed = (id as u32).wrapping_mul(0x9E37_79B1);
    spawn_impact_burst(
        commands,
        impact_assets,
        kind,
        burst_anchor,
        Vec3::Y,
        seed,
        0.65,
    );
}

pub(crate) fn resource_node_visual(
    assets: &ResourceVisualAssets,
    model: ResourceNodeModel,
) -> (Handle<Mesh>, Handle<StandardMaterial>) {
    match model {
        ResourceNodeModel::CoalOre => (assets.coal_node_mesh.clone(), assets.coal_material.clone()),
        ResourceNodeModel::IronOre => (assets.iron_node_mesh.clone(), assets.iron_material.clone()),
        ResourceNodeModel::SulfurOre => (
            assets.sulfur_node_mesh.clone(),
            assets.sulfur_material.clone(),
        ),
        ResourceNodeModel::StoneVein => (
            assets.stone_vein_mesh.clone(),
            assets.stone_vein_material.clone(),
        ),
        ResourceNodeModel::PineTreeSmall => (
            assets.pine_tree_small_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::PineTreeMedium => (
            assets.pine_tree_medium_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::PineTreeLarge => (
            assets.pine_tree_large_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeSmall => (
            assets.birch_tree_small_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeMedium => (
            assets.birch_tree_medium_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BirchTreeLarge => (
            assets.birch_tree_large_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::SurfaceStone => (
            assets.surface_stone_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::BranchPile => (
            assets.branch_pile_mesh.clone(),
            assets.vertex_material.clone(),
        ),
        ResourceNodeModel::HayGrass => (
            assets.hay_grass_mesh.clone(),
            assets.vertex_material.clone(),
        ),
    }
}
