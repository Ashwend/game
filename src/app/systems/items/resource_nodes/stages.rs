//! Visual mining stages for ore/vein nodes.
//!
//! Ore and stone-vein nodes step through `ORE_NODE_STAGE_COUNT` meshes
//! as their replicated `ResourceNodeStorage` drains: untouched, worn
//! down, nearly mined out. The stage swap is purely cosmetic, gather
//! rules, colliders, and targeting are unchanged, but it makes a worked
//! vein readable at a glance and gives each threshold crossing a chunky
//! rock-crumble burst so progress lands as feedback, not just numbers.
//!
//! Event-driven like the rest of the reconcile pipeline: the system only
//! looks at entities whose `ResourceNodeStorage` changed this frame
//! (gathers in the AoI ring, a handful at most), never the full
//! replicated set. Stage state lives in `ResourceNodeEntities::stages`
//! and is compared by value, so a storage diff that doesn't cross a
//! threshold (or a spurious Lightyear change tick) is a no-op.

use bevy::prelude::*;

use crate::{
    app::{
        audio::{PlaySound, SoundId},
        scene::{ImpactEffectAssets, ResourceVisualAssets},
        systems::effects::spawn_ore_shatter_burst,
    },
    protocol::ItemStack,
    resource_nodes::{ResourceNodeDefinition, resource_node_definition},
    server::{ResourceNode, ResourceNodeStorage},
};

use super::{ResourceNodeEntities, spawn::ore_stage_mesh};

/// Remaining-storage fraction above which a node still shows its full,
/// untouched mesh. With a 72-ore node and 6 ore per swing, the first
/// stage swap lands on swing 4 of 12, early enough that mining feels
/// responsive from the start.
const ORE_STAGE_WORN_BELOW: f32 = 0.70;
/// Fraction below which the node shows the nearly-mined-out mesh. Same
/// 72-ore node: swing 8 of 12. The final swing despawns the node through
/// the existing shatter death effect, so there is no empty-mesh stage.
const ORE_STAGE_GUTTED_BELOW: f32 = 0.35;

/// Height above the node base where the stage-crossing crumble burst
/// spawns. Between the worn mound's peak and the gutted core, so the
/// debris reads as breaking off the rock at either transition.
const STAGE_BURST_HEIGHT: f32 = 0.28;
/// Shatter-burst magnitude for a stage crossing (1.0 is the depletion
/// shatter). The shatter's heavy gravity and near-ground spray keep the
/// chunks tumbling at the mound's base, the rock slumping down a size,
/// instead of the wide chip splash an impact burst gives.
const STAGE_BURST_MAGNITUDE: f32 = 0.5;

/// Map remaining storage to a visual stage index (0 = full). Pure
/// fraction math over the definition's spawn quantities, so it works the
/// same for every ore size.
pub(super) fn ore_depletion_stage(
    definition: &ResourceNodeDefinition,
    storage: &[ItemStack],
) -> u8 {
    let total: u32 = definition
        .storage
        .iter()
        .map(|material| material.quantity as u32)
        .sum();
    if total == 0 {
        return 0;
    }
    let remaining: u32 = storage.iter().map(|stack| stack.quantity as u32).sum();
    let fraction = remaining as f32 / total as f32;
    if fraction >= ORE_STAGE_WORN_BELOW {
        0
    } else if fraction >= ORE_STAGE_GUTTED_BELOW {
        1
    } else {
        2
    }
}

/// Stage for a node that is about to be enqueued for spawn: 0 unless it
/// is an ore/vein arriving with partially drained storage (a vein someone
/// else worked, or persisted partial storage loaded from a save).
pub(super) fn initial_node_stage(definition_id: &str, storage: Option<&ResourceNodeStorage>) -> u8 {
    let Some(definition) = resource_node_definition(definition_id) else {
        return 0;
    };
    if !stage_capable(definition) {
        return 0;
    }
    storage
        .map(|storage| ore_depletion_stage(definition, &storage.0))
        .unwrap_or(0)
}

fn stage_capable(definition: &ResourceNodeDefinition) -> bool {
    definition.model.is_ore()
        || definition.model == crate::resource_nodes::ResourceNodeModel::StoneVein
}

/// Swap ore/vein mirror meshes when the replicated storage crosses a
/// stage threshold, firing a rock-crumble burst on the way down. Runs
/// after `apply_resource_nodes_system` so a node spawned this frame is
/// already tracked (its spawn picked the right stage mesh, making this
/// system's first sight of it a no-op).
pub(crate) fn apply_resource_node_stage_system(
    mut commands: Commands,
    assets: Res<ResourceVisualAssets>,
    impact_assets: Res<ImpactEffectAssets>,
    mut play: MessageWriter<PlaySound>,
    mut entities: ResMut<ResourceNodeEntities>,
    changed: Query<(&ResourceNode, &ResourceNodeStorage), Changed<ResourceNodeStorage>>,
) {
    for (node, storage) in &changed {
        let Some(definition) = resource_node_definition(&node.definition_id) else {
            continue;
        };
        if !stage_capable(definition) {
            continue;
        }
        let stage = ore_depletion_stage(definition, &storage.0);

        #[cfg(feature = "replication-trace")]
        info!(
            target: "replication_trace",
            "client: OreStage           EVAL id={} stage={stage}",
            node.id
        );

        // Still waiting on the spawn budget: refresh the queued stage so
        // the mirror spawns current instead of one threshold stale.
        if let Some(pending) = entities
            .pending_spawns
            .iter_mut()
            .find(|pending| pending.id == node.id)
        {
            pending.stage = stage;
            continue;
        }

        let Some(mirror) = entities.entities.get(&node.id).copied() else {
            #[cfg(feature = "replication-trace")]
            info!(
                target: "replication_trace",
                "client: OreStage           SKIP id={} (no mirror)",
                node.id
            );
            continue;
        };
        let previous = entities.stages.insert(node.id, stage).unwrap_or(0);
        if stage == previous {
            continue;
        }
        let Some(mesh) = ore_stage_mesh(&assets, definition.model, stage) else {
            continue;
        };
        #[cfg(feature = "replication-trace")]
        info!(
            target: "replication_trace",
            "client: OreStage           SWAP id={} {previous} -> {stage} mirror={mirror:?}",
            node.id
        );
        commands.entity(mirror).insert(Mesh3d(mesh));

        // Crumble feedback only when mining *down* through a threshold.
        // (Stage decreases shouldn't happen, regrow spawns a fresh node,
        // but if one ever does the mesh still corrects silently.)
        if stage > previous {
            let anchor = Vec3::from(node.position) + Vec3::Y * STAGE_BURST_HEIGHT;
            let seed = (node.id.0 as u32)
                .wrapping_mul(0x85EB_CA77)
                .wrapping_add(stage as u32);
            spawn_ore_shatter_burst(
                &mut commands,
                &impact_assets,
                anchor,
                seed,
                STAGE_BURST_MAGNITUDE,
            );
            play.write(PlaySound::at(SoundId::OreStageCrumble, anchor));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::COAL_ID,
        resource_nodes::{COAL_NODE_ID, PINE_TREE_NODE_ID},
    };

    fn coal_definition() -> &'static ResourceNodeDefinition {
        resource_node_definition(COAL_NODE_ID).expect("coal definition")
    }

    #[test]
    fn stage_steps_down_through_the_thresholds() {
        let definition = coal_definition();
        let total: u16 = definition
            .storage
            .iter()
            .map(|material| material.quantity)
            .sum();

        // Untouched and just-above-worn stay full.
        assert_eq!(
            ore_depletion_stage(definition, &[ItemStack::new(COAL_ID, total)]),
            0
        );
        let above_worn = ((total as f32) * 0.75) as u16;
        assert_eq!(
            ore_depletion_stage(definition, &[ItemStack::new(COAL_ID, above_worn)]),
            0
        );

        // Between the thresholds: worn.
        let mid = ((total as f32) * 0.5) as u16;
        assert_eq!(
            ore_depletion_stage(definition, &[ItemStack::new(COAL_ID, mid)]),
            1
        );

        // Below the gutted threshold (including empty, despawn races the
        // final diff): gutted.
        let low = ((total as f32) * 0.2) as u16;
        assert_eq!(
            ore_depletion_stage(definition, &[ItemStack::new(COAL_ID, low)]),
            2
        );
        assert_eq!(ore_depletion_stage(definition, &[]), 2);
    }

    #[test]
    fn meteorite_steps_through_mining_stages_like_ore() {
        use crate::items::METEORITE_ALLOY_ID;
        use crate::resource_nodes::METEORITE_NODE_ID;
        let definition = resource_node_definition(METEORITE_NODE_ID).expect("meteorite definition");
        // It is stage-capable (its slag-mound glbs have 3 stages).
        assert!(
            stage_capable(definition),
            "meteorite must step through depletion stages"
        );
        let total: u16 = definition.storage.iter().map(|m| m.quantity).sum();
        // Full -> stage 0; mid -> 1; nearly gone / empty -> 2.
        assert_eq!(
            ore_depletion_stage(definition, &[ItemStack::new(METEORITE_ALLOY_ID, total)]),
            0
        );
        let mid = ((total as f32) * 0.5) as u16;
        assert_eq!(
            ore_depletion_stage(definition, &[ItemStack::new(METEORITE_ALLOY_ID, mid)]),
            1
        );
        assert_eq!(ore_depletion_stage(definition, &[]), 2);
        // The initial-stage helper agrees for a part-drained meteorite node.
        let drained = ResourceNodeStorage(vec![ItemStack::new(METEORITE_ALLOY_ID, total / 5)]);
        assert_eq!(initial_node_stage(METEORITE_NODE_ID, Some(&drained)), 2);
    }

    #[test]
    fn initial_stage_is_zero_for_trees_and_full_ores() {
        // Trees never stage, regardless of storage.
        let half_tree = ResourceNodeStorage(vec![ItemStack::new(crate::items::WOOD_ID, 10)]);
        assert_eq!(initial_node_stage(PINE_TREE_NODE_ID, Some(&half_tree)), 0);

        // Full ore spawns at stage 0; a part-mined one spawns staged.
        let definition = coal_definition();
        let total: u16 = definition
            .storage
            .iter()
            .map(|material| material.quantity)
            .sum();
        let full = ResourceNodeStorage(vec![ItemStack::new(COAL_ID, total)]);
        assert_eq!(initial_node_stage(COAL_NODE_ID, Some(&full)), 0);
        let drained = ResourceNodeStorage(vec![ItemStack::new(COAL_ID, total / 4)]);
        assert_eq!(initial_node_stage(COAL_NODE_ID, Some(&drained)), 2);

        // Missing storage component (shouldn't happen, both replicate in
        // the same group) degrades to the full mesh.
        assert_eq!(initial_node_stage(COAL_NODE_ID, None), 0);
    }
}
