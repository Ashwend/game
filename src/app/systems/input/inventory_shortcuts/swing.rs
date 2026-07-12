use bevy::prelude::*;

use crate::{
    app::{
        audio::surface::{SurfaceMaterial, surface_for_resource_model},
        state::{
            ClientRuntime, ImpactEffectKind, PendingAudioCue, PendingImpactEffect,
            PickupTargetState, SwingImpact, SwingTarget,
        },
    },
    items::{HANDS_TOOL, ToolKind, ToolProfile, item_definition},
    protocol::{
        AttackPlayerCommand, ClientMessage, DamageDeployableCommand, ResourceGatherCommand,
    },
    resources::resource_node_definition,
};

use super::GameplayInventoryShortcutsParams;
use super::predict::predict_gather;
use super::send::send_gameplay_message;

/// The swing archetype ([`ItemModel`]) backing a left-click swing: its
/// duration/contact fraction, pose, camera kick, audio pool, and the impact
/// identity carried on the wire (`SwingStart`/`PlayerImpact`). Both a real gather
/// tool and a dedicated weapon produce a swing:
///
/// - A tool maps its [`ToolKind`] to a swing archetype (Hatchet/Pickaxe).
/// - A weapon animates and reads as its own registry `model`
///   (Club/Spear/Sword/Mace).
///
/// This is exactly the server's [`crate::items::ItemDefinition::swing_model`]
/// derivation, so the client's local swing and the server's stamped peer-visible
/// swing can never disagree. Bare hands and non-combat, non-tool items (ores,
/// wood, deployables-in-hand) return `None`, so the swing never starts.
pub(super) fn equipped_swing(
    local_player: &crate::app::state::LocalPlayerState,
) -> Option<crate::items::ItemModel> {
    let definition = local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))?;
    if definition.tool.is_some() || definition.weapon.is_some() {
        return Some(definition.swing_model());
    }
    None
}

/// Resolve the active actionbar item to a tool profile, falling back to
/// the synthesized [`HANDS_TOOL`] when no tool is held. The server runs
/// the same fallback in `apply_gather_command`, so the client's hit
/// check and the server's payout decision stay aligned for crude
/// (hand-harvestable) nodes. Used only by the harvest-check path; the
/// swing-start path goes through [`equipped_swing`] which treats the
/// empty-hand case as "no swing".
pub(super) fn equipped_tool_profile(
    local_player: &crate::app::state::LocalPlayerState,
) -> ToolProfile {
    local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|definition| definition.tool)
        .unwrap_or(HANDS_TOOL)
}

// Single decision point for the swing: a miss queues only the whoosh, a hit
// queues the spatial impact sound, visual chips, camera kick, and the gather
// command. The swing state guarantees at most one impact event per swing, so
// hit and miss audio can never both play.
pub(super) fn dispatch_swing_impact(
    params: &mut GameplayInventoryShortcutsParams,
    impact: SwingImpact,
) {
    match impact.target {
        Some(SwingTarget::ResourceNode(id)) => dispatch_resource_swing(params, impact, id),
        Some(SwingTarget::Deployable(id)) => dispatch_deployable_swing(params, impact, id),
        Some(SwingTarget::Player(id)) => dispatch_player_swing(params, impact, id),
        None => params.gather_input.set_pending_miss_audio(),
    }
}

fn dispatch_resource_swing(
    params: &mut GameplayInventoryShortcutsParams,
    impact: SwingImpact,
    node_id: u64,
) {
    // Target was harvestable when the swing tick read it, but the resource
    // node's anchor / kind metadata could still be missing if the entity
    // was despawned this same frame. Treat that as a miss, better a
    // whoosh than a silent swing.
    let Some(anchor) = resource_target_anchor(&params.pickup_target, node_id) else {
        params.gather_input.set_pending_miss_audio();
        return;
    };
    let Some(surface) = resource_target_surface(&params.pickup_target) else {
        params.gather_input.set_pending_miss_audio();
        return;
    };
    // Pick the visual kind from the node model directly so crude
    // materials (branches, surface stones, grass) get their dedicated
    // smaller bursts instead of the heavy tree/ore palette that
    // `for_surface` would resolve to.
    let kind = resource_target_model(&params.pickup_target)
        .map(ImpactEffectKind::for_resource_model)
        .unwrap_or_else(|| ImpactEffectKind::for_surface(surface));

    let spray_direction = swing_spray_direction(&params.runtime, anchor);
    let seed = params.gather_input.current_swing_seed();
    params.gather_input.set_pending_impact(PendingImpactEffect {
        anchor,
        spray_direction,
        kind,
        seed,
    });
    params.gather_input.set_pending_audio_cue(PendingAudioCue {
        anchor,
        model: impact.model,
        surface,
        is_player_hit: false,
    });

    params.camera_kick.trigger(impact.model);
    params.combat_feedback.trigger_hit_marker(false);

    // Predict the payout landing in the bag instantly. The node's visual
    // shrink / death stays server-driven (we never predict depletion). A
    // rejected gather (range, cooldown, full bag) reverts when the server
    // advances `applied_action_seq`. `seq == 0` means "not predicted".
    let seq = predict_gather(
        &mut params.prediction,
        &params.local_player,
        node_id,
        &params.pickup_target,
    );
    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: node_id,
            seq,
            // Where our look ray hit the node (the same point we spawned our
            // local burst at), so peers spawn their burst there too.
            hit_point: crate::protocol::Vec3Net::new(anchor.x, anchor.y, anchor.z),
        }),
        "gather command",
    );
}

/// Damage swing on a placed structure: same camera kick + per-tool
/// surface cue + spark visual as a resource hit, but the network
/// payload is `DamageDeployable` (no inventory payout, just HP).
fn dispatch_deployable_swing(
    params: &mut GameplayInventoryShortcutsParams,
    impact: SwingImpact,
    deployable_id: u64,
) {
    let Some(anchor) = params
        .pickup_target
        .world_position
        .filter(|_| params.pickup_target.deployable_id == Some(deployable_id))
        .map(|pos| bevy::prelude::Vec3::new(pos.x, pos.y, pos.z))
    else {
        params.gather_input.set_pending_miss_audio();
        return;
    };

    // Surface picks the impact sound + chip palette. The structure's
    // material lives on `DeployableKind` so it stays aligned with the
    // server's damage multiplier path (`kind.material()`).
    let surface = match params
        .pickup_target
        .deployable_kind
        .map(|kind| kind.material())
    {
        Some(
            crate::items::DestructibleMaterial::Wood
            | crate::items::DestructibleMaterial::Sticks
            | crate::items::DestructibleMaterial::WoodBuilding
            | crate::items::DestructibleMaterial::Cloth,
        ) => SurfaceMaterial::Wood,
        Some(
            crate::items::DestructibleMaterial::Stone
            | crate::items::DestructibleMaterial::StoneBuilding
            // Iron doors read as a hard surface; the stone chip/clang is the
            // closest cue we have until a dedicated metal impact exists.
            | crate::items::DestructibleMaterial::MetalBuilding,
        )
        | None => SurfaceMaterial::Stone,
    };
    let visual_kind = ImpactEffectKind::for_surface(surface);

    let spray_direction = swing_spray_direction(&params.runtime, anchor);
    let seed = params.gather_input.current_swing_seed();
    params.gather_input.set_pending_impact(PendingImpactEffect {
        anchor,
        spray_direction,
        kind: visual_kind,
        seed,
    });
    params.gather_input.set_pending_audio_cue(PendingAudioCue {
        anchor,
        model: impact.model,
        surface,
        is_player_hit: false,
    });

    params.camera_kick.trigger(impact.model);
    params.combat_feedback.trigger_hit_marker(false);

    // A hammer swing on any placed structure is a repair tap, not
    // damage; the server applies the material cost + HP restore
    // (tier materials for building blocks and doors, the crafting
    // recipe's primary material for crafted deployables). The hammer
    // never damages anything. The hammer and the hatchet share a swing
    // archetype (Hatchet), so we re-resolve the held tool here rather than
    // inferring intent from the swing model.
    let is_hammer =
        equipped_tool_profile(&params.local_player).kind == crate::items::ToolKind::Hammer;
    let message = if is_hammer {
        ClientMessage::Building(crate::protocol::BuildingCommand::Repair { id: deployable_id })
    } else {
        ClientMessage::DamageDeployable(DamageDeployableCommand { id: deployable_id })
    };
    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        message,
        "damage command",
    );
}

/// PvP swing dispatch, mirrors `dispatch_deployable_swing` but the
/// network payload is `AttackPlayer` (no inventory payout) and the
/// impact visual uses the dedicated `FleshHit` palette: a red blood spray
/// (see `ImpactEffectAssets::blood_material` + the `FleshHit` arm in
/// `app::systems::effects`).
fn dispatch_player_swing(
    params: &mut GameplayInventoryShortcutsParams,
    impact: SwingImpact,
    target_player_id: crate::protocol::ClientId,
) {
    let Some(anchor) = params
        .pickup_target
        .world_position
        .filter(|_| params.pickup_target.player_id == Some(target_player_id))
        .map(|pos| bevy::prelude::Vec3::new(pos.x, pos.y, pos.z))
    else {
        // Target moved out of view between scan and impact, treat as
        // a miss so the swing still produces a whoosh.
        params.gather_input.set_pending_miss_audio();
        return;
    };

    // Local prediction: chip burst + camera kick + impact audio so the
    // attacker sees instant feedback. The server confirms with
    // `ServerMessage::PlayerImpact` to peers; a desync resolves on the
    // next replication tick. The `is_player_hit` flag steers the audio
    // dispatcher onto the dedicated `ImpactPlayerBlunt` pool.
    let surface = SurfaceMaterial::Wood; // audio fallback when pool routing is bypassed.
    let visual_kind = ImpactEffectKind::FleshHit;
    let spray_direction = swing_spray_direction(&params.runtime, anchor);
    let seed = params.gather_input.current_swing_seed();
    params.gather_input.set_pending_impact(PendingImpactEffect {
        anchor,
        spray_direction,
        kind: visual_kind,
        seed,
    });
    params.gather_input.set_pending_audio_cue(PendingAudioCue {
        anchor,
        model: impact.model,
        surface,
        is_player_hit: true,
    });

    // Predicted floating damage number, orange, since the local client is the
    // attacker. The server replies with `PlayerImpact { damage_dealt }` so a
    // desync would cost only the brief mismatch between this predicted value and
    // the armor-reduced server value. Resolve the held item's `AttackProfile` the
    // same way the server does (a weapon's own damage, or a gather tool's
    // per-tier damage) rather than going by tool alone, so a weapon's predicted
    // number is right too.
    if let Some(profile) = params
        .local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(crate::combat::resolve_attack_profile)
    {
        params
            .commands
            .spawn(crate::app::ui::floating_text::FloatingDamageText::new(
                anchor,
                profile.damage,
                crate::app::ui::floating_text::FloatingDamageRole::Dealt,
            ));
    }

    params.camera_kick.trigger(impact.model);
    params.combat_feedback.trigger_hit_marker(true);
    // Attacker-side hit-stop: a brief viewmodel freeze on this confirmed local
    // player hit (only here, never on a node/deployable hit or a whiff). Scaled
    // by the Dev `hit_stop_scale` slider (neutral 1.0, 0 disables), read the same
    // per-frame way as the swing/kick feel.
    params
        .gather_input
        .trigger_hit_stop(params.settings.dev.combat.hit_stop_scale);

    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        ClientMessage::AttackPlayer(AttackPlayerCommand { target_player_id }),
        "attack player command",
    );
}

pub(super) fn equipped_tool_can_harvest_target(
    local_player: &crate::app::state::LocalPlayerState,
    target: &PickupTargetState,
) -> bool {
    let profile = equipped_tool_profile(local_player);
    let Some(definition_id) = target.resource_definition_id.as_deref() else {
        return false;
    };
    let Some(definition) = resource_node_definition(definition_id) else {
        return false;
    };
    definition.required_tool.allows(profile)
}

/// Returns the resource node model the player is currently looking at,
/// resolved through the pickup target's cached definition id. Used to
/// drive per-model swing visuals (e.g. small grass burst vs heavy
/// tree-chip burst).
pub(super) fn resource_target_model(
    target: &PickupTargetState,
) -> Option<crate::resources::ResourceNodeModel> {
    let definition_id = target.resource_definition_id.as_deref()?;
    resource_node_definition(definition_id).map(|definition| definition.model)
}

/// Returns true when the looked-at resource node is hand-harvestable
/// (its `required_tool` is `Hands`). The E quick-pickup path is gated on
/// this client-side and re-checked server-side.
pub(super) fn resource_target_is_crude(target: &PickupTargetState) -> bool {
    let Some(definition_id) = target.resource_definition_id.as_deref() else {
        return false;
    };
    let Some(definition) = resource_node_definition(definition_id) else {
        return false;
    };
    definition.required_tool.kind == ToolKind::Hands
}

pub(super) fn resource_target_anchor(target: &PickupTargetState, node_id: u64) -> Option<Vec3> {
    let position = target.world_position?;
    if target.resource_node_id != Some(node_id) {
        return None;
    }
    Some(Vec3::new(position.x, position.y, position.z))
}

pub(super) fn resource_target_surface(target: &PickupTargetState) -> Option<SurfaceMaterial> {
    let definition_id = target.resource_definition_id.as_deref()?;
    let definition = resource_node_definition(definition_id)?;
    Some(surface_for_resource_model(definition.model))
}

pub(super) fn swing_spray_direction(runtime: &ClientRuntime, anchor: Vec3) -> Vec3 {
    let Some(player) = runtime.local_view() else {
        return Vec3::Y;
    };
    let eye = Vec3::from(player.position) + Vec3::Y * crate::app::EYE_HEIGHT;
    let to_player = (eye - anchor).normalize_or_zero();
    if to_player.length_squared() < f32::EPSILON {
        Vec3::Y
    } else {
        to_player
    }
}
