use bevy::{camera::visibility::VisibilityRange, light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{ImpactEffectAssets, NetworkResourceNode, ResourceVisualAssets, ToonMaterial},
        state::ImpactEffectKind,
        systems::effects::spawn_impact_burst,
    },
    protocol::{ResourceNodeId, Vec3Net},
    resources::ResourceNodeModel,
};

use super::{ResourceNodeEntities, ResourceNodePopIn, hay_sway::HayGrass};

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_resource_node_entity(
    commands: &mut Commands,
    assets: &ResourceVisualAssets,
    impact_assets: &ImpactEffectAssets,
    entities: &mut ResourceNodeEntities,
    id: ResourceNodeId,
    position: Vec3Net,
    model: ResourceNodeModel,
    dead: bool,
    stage: u8,
    target_transform: Transform,
    should_pop_in: bool,
) {
    // A tree standing where growth is poor renders as a bare dead snag instead of a
    // lush tree. `dead` is the server-authoritative, replicated flag (decided at
    // generation, see `resources::spawn_resource_node`); this is a pure visual swap,
    // the node is otherwise a normal tree. Snags are a single weathered vertex-
    // coloured mesh, no canopy and no LOD (they're already low-poly).
    let dead_tree = model.is_tree() && dead;
    let live_tree = model.is_tree() && !dead;

    let (mesh, material) = if dead_tree {
        (
            dead_tree_mesh(assets, model),
            ResourceNodeMaterial::Standard(assets.dead_bark_material.clone()),
        )
    } else {
        let (mesh, material) = resource_node_visual(assets, model, id);
        // Ore/vein nodes that arrive already part-mined (a vein someone else worked,
        // or persisted partial storage from a save) spawn straight at their
        // depletion-stage mesh instead of briefly flashing full. (Trees: this is the
        // bark trunk; the alpha-masked canopy is a child spawned below.)
        let mesh = ore_stage_mesh(assets, model, stage).unwrap_or(mesh);
        (mesh, material)
    };
    let mut spawn_command = commands.spawn((
        Name::new(format!("Resource Node {id}")),
        NetworkResourceNode { id, model, dead },
        Mesh3d(mesh),
        target_transform,
        Visibility::Visible,
    ));
    // Ore/vein nodes get the cel-shaded `ToonMaterial`; everything else its
    // `StandardMaterial` (distinct component types, so attached after the spawn).
    insert_resource_node_material(&mut spawn_command, material);
    // Crude clutter (branch piles, surface stones, hay grass) spawns densely and
    // casts only a negligible-size shadow under its own footprint, so skip the
    // shadow pass for it. Trees DO cast (trunk + the near canopy child below): a
    // forest keeps its readable near-tree shade, while the distant low-poly LOD
    // child opts out so distant trees stop flooding the outer cascades. The
    // player, buildings, and ore/stone veins still cast.
    if model.is_crude() {
        spawn_command.insert(NotShadowCaster);
    }
    // Hay grass leans in the wind on the CPU (it can't bend in a shader like the
    // cosmetic field, see `sway_hay_grass_system`); capture its resting rotation
    // so the per-frame lean is re-derived from a fixed base instead of drifting.
    if matches!(model, ResourceNodeModel::HayGrass) {
        spawn_command.insert(HayGrass::new(target_transform.rotation));
    }
    if should_pop_in {
        spawn_command.insert(ResourceNodePopIn {
            elapsed: 0.0,
            base_transform: target_transform,
        });
    }
    // Live trees get a distance LOD: this (full-detail) trunk + its canopy child
    // switch off past `TREE_LOD_DISTANCE`; a low-poly LOD child switches on at the
    // same distance. Bevy's `VisibilityRange` does this GPU-side off the existing
    // visibility pass, no per-frame CPU cost. Hard step, not a dither crossfade
    // (see `TREE_LOD_DISTANCE` for why).
    if live_tree {
        spawn_command.insert(tree_lod_high_range());
    }
    let entity = spawn_command.id();
    entities.entities.insert(id, entity);
    entities.stages.insert(id, stage);

    if live_tree {
        let foliage = tree_foliage_visual(assets, model);
        let lod_mesh = tree_lod_mesh(assets, model);
        commands.entity(entity).with_children(|parent| {
            // Alpha-masked needle/leaf canopy, drawn near (hidden with the trunk
            // past the LOD distance). Casts shadows like the trunk so a forest
            // floor stays shaded up close.
            if let Some((foliage_mesh, foliage_material)) = foliage {
                parent.spawn((
                    Name::new(format!("Resource Node {id} Canopy")),
                    Mesh3d(foliage_mesh),
                    MeshMaterial3d(foliage_material),
                    tree_lod_high_range(),
                    Transform::default(),
                    Visibility::Visible,
                ));
            }
            // Distant low-poly stand-in: one cheap vertex-coloured mesh that
            // switches in at `TREE_LOD_DISTANCE` and does not cast, so trees past
            // it stop re-rendering into the shadow cascades.
            if let Some(lod_mesh) = lod_mesh {
                parent.spawn((
                    Name::new(format!("Resource Node {id} LOD")),
                    Mesh3d(lod_mesh),
                    MeshMaterial3d(assets.vertex_material.clone()),
                    tree_lod_low_range(),
                    Transform::default(),
                    Visibility::Visible,
                    NotShadowCaster,
                ));
            }
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

/// The bare dead-snag mesh for a tree model, by size, species no longer matters
/// once the canopy is gone. Non-tree models never reach here (guarded by
/// `is_tree` at the call site); they fall through to the small snag.
fn dead_tree_mesh(assets: &ResourceVisualAssets, model: ResourceNodeModel) -> Handle<Mesh> {
    match model {
        ResourceNodeModel::PineTreeLarge | ResourceNodeModel::BirchTreeLarge => {
            assets.dead_tree_large_mesh.clone()
        }
        ResourceNodeModel::PineTreeMedium | ResourceNodeModel::BirchTreeMedium => {
            assets.dead_tree_medium_mesh.clone()
        }
        _ => assets.dead_tree_small_mesh.clone(),
    }
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
/// LOD is visible at any distance, no gap, no overlap.
fn tree_lod_low_range() -> VisibilityRange {
    VisibilityRange {
        start_margin: TREE_LOD_DISTANCE..TREE_LOD_DISTANCE,
        end_margin: 10_000.0..10_000.0,
        use_aabb: false,
    }
}

/// A short upward chip burst sells the "fresh from the ground" moment.
/// Trees throw wood chips, ores throw stone shards, crude nodes throw
/// their own small per-kind burst, same palette as gather impacts so
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

/// The depletion-stage mesh for an ore/vein model, or `None` for models
/// that don't step through mining stages (trees, crude clutter). `stage`
/// indexes the per-style array and is clamped to the last stage.
pub(crate) fn ore_stage_mesh(
    assets: &ResourceVisualAssets,
    model: ResourceNodeModel,
    stage: u8,
) -> Option<Handle<Mesh>> {
    let meshes = match model {
        ResourceNodeModel::CoalOre => &assets.coal_node_meshes,
        ResourceNodeModel::IronOre => &assets.iron_node_meshes,
        ResourceNodeModel::SulfurOre => &assets.sulfur_node_meshes,
        ResourceNodeModel::StoneVein => &assets.stone_vein_meshes,
        _ => return None,
    };
    let index = (stage as usize).min(meshes.len() - 1);
    Some(meshes[index].clone())
}

/// Which material kind a resource-node visual carries. Ore/vein nodes are
/// cel-shaded ([`ToonMaterial`]); every other model (trees, crude clutter)
/// stays on a `StandardMaterial`. The spawn sites attach whichever component
/// type this names, since `MeshMaterial3d<A>` and `MeshMaterial3d<B>` are
/// distinct components.
pub(crate) enum ResourceNodeMaterial {
    Standard(Handle<StandardMaterial>),
    Toon(Handle<ToonMaterial>),
}

pub(crate) fn resource_node_visual(
    assets: &ResourceVisualAssets,
    model: ResourceNodeModel,
    id: ResourceNodeId,
) -> (Handle<Mesh>, ResourceNodeMaterial) {
    let toon = || ResourceNodeMaterial::Toon(assets.ore_toon_material.clone());
    match model {
        ResourceNodeModel::CoalOre => (assets.coal_node_meshes[0].clone(), toon()),
        ResourceNodeModel::IronOre => (assets.iron_node_meshes[0].clone(), toon()),
        ResourceNodeModel::SulfurOre => (assets.sulfur_node_meshes[0].clone(), toon()),
        ResourceNodeModel::StoneVein => (assets.stone_vein_meshes[0].clone(), toon()),
        // Trees: the bark trunk mesh + shared bark material. The alpha-masked
        // canopy is a separate child (see `tree_foliage_visual` + the spawn path).
        ResourceNodeModel::PineTreeSmall => (
            assets.pine_tree_small_trunk_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.pine_bark_material.clone()),
        ),
        ResourceNodeModel::PineTreeMedium => (
            assets.pine_tree_medium_trunk_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.pine_bark_material.clone()),
        ),
        ResourceNodeModel::PineTreeLarge => (
            assets.pine_tree_large_trunk_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.pine_bark_material.clone()),
        ),
        ResourceNodeModel::BirchTreeSmall => (
            assets.birch_tree_small_trunk_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.birch_bark_material.clone()),
        ),
        ResourceNodeModel::BirchTreeMedium => (
            assets.birch_tree_medium_trunk_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.birch_bark_material.clone()),
        ),
        ResourceNodeModel::BirchTreeLarge => (
            assets.birch_tree_large_trunk_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.birch_bark_material.clone()),
        ),
        ResourceNodeModel::SurfaceStone => (
            assets.surface_stone_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.vertex_material.clone()),
        ),
        ResourceNodeModel::BranchPile => (
            assets.branch_pile_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.vertex_material.clone()),
        ),
        // Tall grass: pick one of the 3 toony seed-headed cards by `id % 3` so a
        // patch of hay isn't one repeated tuft.
        ResourceNodeModel::HayGrass => (
            assets.hay_grass_mesh.clone(),
            ResourceNodeMaterial::Standard(assets.hay_grass_materials[(id % 3) as usize].clone()),
        ),
    }
}

/// Attach the right material component for a resource-node visual: a
/// `MeshMaterial3d<ToonMaterial>` for ore/vein nodes, otherwise the
/// `MeshMaterial3d<StandardMaterial>`. Centralises the enum match so both spawn
/// sites (network spawn + menu backdrop) stay in sync.
pub(crate) fn insert_resource_node_material(
    entity: &mut EntityCommands,
    material: ResourceNodeMaterial,
) {
    match material {
        ResourceNodeMaterial::Standard(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
        ResourceNodeMaterial::Toon(handle) => {
            entity.insert(MeshMaterial3d(handle));
        }
    }
}

/// The alpha-masked canopy (needle/leaf) mesh + shared foliage material for a live
/// tree model, spawned as a child of the bark trunk. `None` for non-tree models
/// (and dead snags, which have no canopy). Pine and birch each share one canopy
/// material across all sizes so the forest batches by mesh+material.
pub(crate) fn tree_foliage_visual(
    assets: &ResourceVisualAssets,
    model: ResourceNodeModel,
) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
    Some(match model {
        ResourceNodeModel::PineTreeSmall => (
            assets.pine_tree_small_foliage_mesh.clone(),
            assets.pine_foliage_material.clone(),
        ),
        ResourceNodeModel::PineTreeMedium => (
            assets.pine_tree_medium_foliage_mesh.clone(),
            assets.pine_foliage_material.clone(),
        ),
        ResourceNodeModel::PineTreeLarge => (
            assets.pine_tree_large_foliage_mesh.clone(),
            assets.pine_foliage_material.clone(),
        ),
        ResourceNodeModel::BirchTreeSmall => (
            assets.birch_tree_small_foliage_mesh.clone(),
            assets.birch_foliage_material.clone(),
        ),
        ResourceNodeModel::BirchTreeMedium => (
            assets.birch_tree_medium_foliage_mesh.clone(),
            assets.birch_foliage_material.clone(),
        ),
        ResourceNodeModel::BirchTreeLarge => (
            assets.birch_tree_large_foliage_mesh.clone(),
            assets.birch_foliage_material.clone(),
        ),
        _ => return None,
    })
}

// Dead-tree vitality is decided server-side now (see `resources::spawn_resource_node`
// + its tests); this module only renders the replicated `dead` flag.
