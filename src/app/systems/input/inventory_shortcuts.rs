use bevy::{
    ecs::system::SystemParam,
    input::mouse::MouseWheel,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::{
        audio::surface::{SurfaceMaterial, surface_for_resource_model},
        state::{
            ClientErrorToast, ClientRuntime, ClientSettings, ErrorToastSink, GatherInputState,
            ImpactEffectKind, InventoryUiState, KeyAction, MenuState, PendingAudioCue,
            PendingImpactEffect, PickupTargetState, SwingImpact, SwingTarget, ToolSwapState,
        },
    },
    items::{HANDS_TOOL, ToolKind, ToolProfile, item_definition},
    protocol::{
        ACTIONBAR_SLOT_COUNT, AttackPlayerCommand, ClientMessage, DamageDeployableCommand,
        InventoryCommand, ItemContainerSlot, LootBagCommand, ResourceGatherCommand,
    },
    resources::resource_node_definition,
};

use super::gating::{gameplay_accepts_controls, primary_window_focused};

#[derive(SystemParam)]
pub(crate) struct GameplayInventoryShortcutsParams<'w, 's> {
    commands: Commands<'w, 's>,
    time: Res<'w, Time>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    mouse_buttons: Res<'w, ButtonInput<MouseButton>>,
    mouse_wheel: MessageReader<'w, 's, MouseWheel>,
    runtime: ResMut<'w, ClientRuntime>,
    local_player: Res<'w, crate::app::state::LocalPlayerState>,
    gather_input: ResMut<'w, GatherInputState>,
    inventory_ui: ResMut<'w, InventoryUiState>,
    menu: ResMut<'w, MenuState>,
    crafting_ui: ResMut<'w, crate::app::state::CraftingUiState>,
    pickup_target: Res<'w, PickupTargetState>,
    swap_state: Res<'w, ToolSwapState>,
    settings: Res<'w, ClientSettings>,
    camera_kick: ResMut<'w, crate::app::systems::CameraImpactKick>,
    error_toasts: MessageWriter<'w, ClientErrorToast>,
    primary_window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

pub(crate) fn gameplay_inventory_shortcuts_system(mut params: GameplayInventoryShortcutsParams) {
    if !gameplay_accepts_controls(&params.menu, primary_window_focused(&params.primary_window)) {
        params.mouse_wheel.clear();
        params.gather_input.cancel();
        return;
    }

    for slot in 0..ACTIONBAR_SLOT_COUNT {
        if actionbar_key_pressed(&params.keys, &params.settings, slot) {
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::SelectActionbarSlot { slot },
            );
        }
    }

    let wheel_delta = params
        .mouse_wheel
        .read()
        .map(|event| event.y.signum() as i8)
        .sum::<i8>();
    if wheel_delta != 0 {
        send_inventory_command(
            &mut params.runtime,
            &mut params.error_toasts,
            InventoryCommand::SelectActionbarOffset {
                offset: -wheel_delta.signum(),
            },
        );
    }

    if params
        .settings
        .keybindings
        .just_pressed(KeyAction::DropItem, &params.keys)
    {
        let Some(active_actionbar_slot) = params
            .local_player
            .private
            .as_ref()
            .map(|private| private.inventory.active_actionbar_slot)
        else {
            return;
        };
        send_inventory_command(
            &mut params.runtime,
            &mut params.error_toasts,
            InventoryCommand::Drop {
                from: ItemContainerSlot::actionbar(active_actionbar_slot),
                quantity: Some(1),
            },
        );
    }

    if params
        .settings
        .keybindings
        .just_pressed(KeyAction::PickUp, &params.keys)
    {
        if let Some(dropped_item_id) = params.pickup_target.dropped_item_id {
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::PickUp { dropped_item_id },
            );
            params.inventory_ui.note_pickup_intent();
        } else if let Some(resource_node_id) = params.pickup_target.resource_node_id
            && resource_target_is_crude(&params.pickup_target)
        {
            // Crude nodes (branches, surface stones, grass tufts) can be
            // picked up with E. The server gates on the same crude check
            // and a view-ray ping, so a wrong target is silently dropped.
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::PickUpResourceNode { resource_node_id },
            );
            params.inventory_ui.note_pickup_intent();
        } else if let Some(id) = params.pickup_target.deployable_id {
            // Same key, different intent: opening a placed structure's
            // UI. Furnace opens its server-side interactive view;
            // workbench is a client-only convenience that opens the
            // crafting modal (the workbench is otherwise just a
            // proximity gate). Other deployable kinds no-op for now.
            use crate::items::DeployableKind;
            match params.pickup_target.deployable_kind {
                Some(DeployableKind::Furnace { .. }) => {
                    send_place_deployable_or_furnace_open(
                        &mut params.runtime,
                        &mut params.error_toasts,
                        id,
                    );
                }
                Some(DeployableKind::Workbench { .. }) => {
                    crate::app::systems::input::open_crafting_modal(
                        &mut params.menu,
                        &mut params.inventory_ui,
                        &mut params.crafting_ui,
                        &mut params.runtime,
                        &mut params.error_toasts,
                    );
                }
                None => {}
            }
        } else if let Some(id) = params.pickup_target.loot_bag_id {
            // Open the death loot bag. Server validates range +
            // membership and replies by populating
            // `PlayerPrivate.open_loot_bag` so the transfer UI
            // becomes visible on the next replication tick.
            send_gameplay_message(
                &mut params.runtime,
                &mut params.error_toasts,
                ClientMessage::LootBag(LootBagCommand::Open { id }),
                "loot bag open",
            );
        }
    }

    // Tool-swap entry locks out swings — the new tool is still being
    // lifted into view, so it can't be used yet. Death does the same:
    // a corpse can't swing.
    let local_dead = matches!(
        params.local_player.lifecycle,
        Some(crate::server::PlayerLifecycle::Dead { .. })
    );
    let equipped_tool = if params.swap_state.is_swapping() || local_dead {
        params.gather_input.cancel();
        None
    } else {
        equipped_tool_kind(&params.local_player)
    };
    // Pick the swing target. Priority:
    //  1. Another player inside attack range. Players win over
    //     resource nodes / deployables because at melee range the
    //     intent is unambiguous — if you're aiming at the avatar of
    //     someone running past a tree, that's the target you mean.
    //     Gated on a real tool being equipped (bare hands deal no PvP
    //     damage; the server rejects too).
    //  2. A resource node the held tool can actually harvest. Wrong-
    //     tool nodes turn into "no target" so the impact frame resolves
    //     to a clean miss instead of a hit the server would reject.
    //  3. A placed structure the player is aimed at. Reaching this
    //     branch already implies a real tool is equipped — bare hands
    //     and non-tool items return `None` from `equipped_tool_kind`,
    //     which short-circuits the swing before this check runs.
    let target =
        if let Some(player_id) = params.pickup_target.player_id
            && equipped_tool.is_some()
        {
            Some(SwingTarget::Player(player_id))
        } else if let Some(node_id) = params.pickup_target.resource_node_id.filter(|_| {
            equipped_tool_can_harvest_target(&params.local_player, &params.pickup_target)
        }) {
            Some(SwingTarget::ResourceNode(node_id))
        } else if let Some(deployable_id) = params.pickup_target.deployable_id
            && equipped_tool.is_some()
        {
            Some(SwingTarget::Deployable(deployable_id))
        } else {
            None
        };
    let impact = params.gather_input.update(
        params.time.delta_secs(),
        params.mouse_buttons.just_pressed(MouseButton::Left),
        params.mouse_buttons.pressed(MouseButton::Left),
        equipped_tool,
        target,
    );
    if let Some(impact) = impact {
        dispatch_swing_impact(&mut params, impact);
    }
}

/// Tool kind backing a left-click swing. Only items with a real
/// [`ToolProfile`] count — bare hands and non-tool items (ores, wood,
/// deployables-in-hand) return `None` so the swing never starts and no
/// impact-detection fires. Fists can't damage anything in this game
/// today, and an in-hand ore swinging at a tree shouldn't pretend to.
fn equipped_tool_kind(local_player: &crate::app::state::LocalPlayerState) -> Option<ToolKind> {
    local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|definition| definition.tool)
        .map(|profile| profile.kind)
}

/// Resolve the active actionbar item to a tool profile, falling back to
/// the synthesized [`HANDS_TOOL`] when no tool is held. The server runs
/// the same fallback in `apply_gather_command`, so the client's hit
/// check and the server's payout decision stay aligned for crude
/// (hand-harvestable) nodes. Used only by the harvest-check path; the
/// swing-start path goes through [`equipped_tool_kind`] which treats
/// the empty-hand case as "no tool".
fn equipped_tool_profile(local_player: &crate::app::state::LocalPlayerState) -> ToolProfile {
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
fn dispatch_swing_impact(params: &mut GameplayInventoryShortcutsParams, impact: SwingImpact) {
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
    // was despawned this same frame. Treat that as a miss — better a
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
        tool: impact.tool,
        surface,
        is_player_hit: false,
    });

    params.camera_kick.trigger(impact.tool);

    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: node_id,
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
        Some(crate::items::DeployableMaterial::Wood) => SurfaceMaterial::Wood,
        Some(crate::items::DeployableMaterial::Stone) | None => SurfaceMaterial::Stone,
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
        tool: impact.tool,
        surface,
        is_player_hit: false,
    });

    params.camera_kick.trigger(impact.tool);

    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        ClientMessage::DamageDeployable(DamageDeployableCommand { id: deployable_id }),
        "damage command",
    );
}

/// PvP swing dispatch — mirrors `dispatch_deployable_swing` but the
/// network payload is `AttackPlayer` (no inventory payout) and the
/// impact visual uses the dedicated `FleshHit` palette (Phase 4 will
/// flip the placeholder kind to `FleshHit`; today it uses the generic
/// stone-shard fallback so the swing still produces feedback).
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
        // Target moved out of view between scan and impact — treat as
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
        tool: impact.tool,
        surface,
        is_player_hit: true,
    });

    // Predicted floating damage number — orange, since the local
    // client is the attacker. The server replies with
    // `PlayerImpact { damage_dealt }` so a desync would cost only
    // the brief mismatch between this predicted value and the
    // armor-reduced server value. Today every player has armor 0
    // so the prediction is always exact.
    if let Some(damage) = crate::combat::tool_player_damage(impact.tool, 0) {
        params
            .commands
            .spawn(crate::app::ui::floating_text::FloatingDamageText::new(
                anchor,
                damage.raw,
                crate::app::ui::floating_text::FloatingDamageRole::Dealt,
            ));
    }

    params.camera_kick.trigger(impact.tool);

    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        ClientMessage::AttackPlayer(AttackPlayerCommand { target_player_id }),
        "attack player command",
    );
}

fn equipped_tool_can_harvest_target(
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
fn resource_target_model(
    target: &PickupTargetState,
) -> Option<crate::resources::ResourceNodeModel> {
    let definition_id = target.resource_definition_id.as_deref()?;
    resource_node_definition(definition_id).map(|definition| definition.model)
}

/// Returns true when the looked-at resource node is hand-harvestable
/// (its `required_tool` is `Hands`). The E quick-pickup path is gated on
/// this client-side and re-checked server-side.
fn resource_target_is_crude(target: &PickupTargetState) -> bool {
    let Some(definition_id) = target.resource_definition_id.as_deref() else {
        return false;
    };
    let Some(definition) = resource_node_definition(definition_id) else {
        return false;
    };
    definition.required_tool.kind == ToolKind::Hands
}

fn resource_target_anchor(target: &PickupTargetState, node_id: u64) -> Option<Vec3> {
    let position = target.world_position?;
    if target.resource_node_id != Some(node_id) {
        return None;
    }
    Some(Vec3::new(position.x, position.y, position.z))
}

fn resource_target_surface(target: &PickupTargetState) -> Option<SurfaceMaterial> {
    let definition_id = target.resource_definition_id.as_deref()?;
    let definition = resource_node_definition(definition_id)?;
    Some(surface_for_resource_model(definition.model))
}

fn swing_spray_direction(runtime: &ClientRuntime, anchor: Vec3) -> Vec3 {
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

/// Direct slot → keybinding map. Looks the action up by slot index so the
/// table stays in lockstep with `ACTIONBAR_SLOT_COUNT` and the bindings the
/// player can rebind through the options panel.
const ACTIONBAR_ACTIONS: [KeyAction; ACTIONBAR_SLOT_COUNT] = [
    KeyAction::ActionbarSlot1,
    KeyAction::ActionbarSlot2,
    KeyAction::ActionbarSlot3,
    KeyAction::ActionbarSlot4,
    KeyAction::ActionbarSlot5,
    KeyAction::ActionbarSlot6,
    KeyAction::ActionbarSlot7,
    KeyAction::ActionbarSlot8,
    KeyAction::ActionbarSlot9,
];

const _: () = assert!(ACTIONBAR_ACTIONS.len() == ACTIONBAR_SLOT_COUNT);

fn actionbar_key_pressed(
    keys: &ButtonInput<KeyCode>,
    settings: &ClientSettings,
    slot: usize,
) -> bool {
    ACTIONBAR_ACTIONS
        .get(slot)
        .is_some_and(|action| settings.keybindings.just_pressed(*action, keys))
}

pub(crate) fn send_inventory_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    command: InventoryCommand,
) {
    send_gameplay_message(
        runtime,
        error_toasts,
        ClientMessage::Inventory(command),
        "inventory command",
    );
}

pub(crate) fn send_crafting_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    command: crate::protocol::CraftingCommand,
) {
    send_gameplay_message(
        runtime,
        error_toasts,
        ClientMessage::Crafting(command),
        "crafting command",
    );
}

pub(crate) fn send_place_deployable_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    command: crate::protocol::PlaceDeployableCommand,
) {
    send_gameplay_message(
        runtime,
        error_toasts,
        ClientMessage::PlaceDeployable(command),
        "place command",
    );
}

pub(crate) fn send_furnace_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    command: crate::protocol::FurnaceCommand,
) {
    send_gameplay_message(
        runtime,
        error_toasts,
        ClientMessage::Furnace(command),
        "furnace command",
    );
}

pub(crate) fn send_loot_bag_command(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    command: crate::protocol::LootBagCommand,
) {
    send_gameplay_message(
        runtime,
        error_toasts,
        ClientMessage::LootBag(command),
        "loot bag command",
    );
}

/// Wrapper used by the E-interact handler so the call site reads as
/// "open this furnace" instead of a generic enum constructor. Inline-
/// only convenience — feel free to inline it if the call site is the
/// last consumer.
fn send_place_deployable_or_furnace_open(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    id: crate::protocol::DeployedEntityId,
) {
    send_furnace_command(
        runtime,
        error_toasts,
        crate::protocol::FurnaceCommand::Open { id },
    );
}

fn send_gameplay_message(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    message: ClientMessage,
    label: &str,
) {
    let Some(session) = runtime.session.as_mut() else {
        report_send_failure(
            runtime,
            error_toasts,
            format!("{label} failed: not connected"),
        );
        return;
    };

    if let Err(error) = session.send(message) {
        report_send_failure(runtime, error_toasts, format!("{label} failed: {error}"));
    }
}

fn report_send_failure(
    runtime: &mut ClientRuntime,
    error_toasts: &mut dyn ErrorToastSink,
    text: String,
) {
    runtime.push_error_message(text.clone());
    error_toasts.push_error(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::LocalPlayerState;
    use crate::items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID, WOOD_ID};
    use crate::protocol::{ItemStack, PlayerInventoryState, Vec3Net};
    use crate::resources::{
        BRANCH_PILE_NODE_ID, COAL_NODE_ID, PINE_TREE_NODE_ID, SURFACE_STONE_NODE_ID,
    };
    use crate::server::{PlayerLifecycle, PlayerPrivate};

    fn local_player_holding(item_id: Option<&str>) -> LocalPlayerState {
        let mut inventory = PlayerInventoryState::empty();
        if let Some(id) = item_id {
            inventory.actionbar_slots[0] = Some(ItemStack::new(id, 1));
        }
        LocalPlayerState {
            entity: None,
            public: None,
            private: Some(PlayerPrivate {
                inventory,
                crafting: Default::default(),
                open_furnace: None,
                open_loot_bag: None,
                last_processed_input: 0,
            }),
            lifecycle: None,
        }
    }

    fn target_for_node(node_id: u64, definition_id: &str) -> PickupTargetState {
        PickupTargetState {
            resource_node_id: Some(node_id),
            resource_definition_id: Some(definition_id.to_owned()),
            world_position: Some(Vec3Net::new(1.0, 2.0, 3.0)),
            ..Default::default()
        }
    }

    #[test]
    fn equipped_tool_kind_only_resolves_real_tools() {
        // A hatchet resolves to its tool kind.
        let with_axe = local_player_holding(Some(BASIC_HATCHET_ID));
        assert_eq!(equipped_tool_kind(&with_axe), Some(ToolKind::Axe));

        let with_pick = local_player_holding(Some(BASIC_PICKAXE_ID));
        assert_eq!(equipped_tool_kind(&with_pick), Some(ToolKind::Pickaxe));

        // Bare hands -> no tool.
        let empty = local_player_holding(None);
        assert_eq!(equipped_tool_kind(&empty), None);

        // A non-tool item (wood) -> no tool.
        let with_wood = local_player_holding(Some(WOOD_ID));
        assert_eq!(equipped_tool_kind(&with_wood), None);
    }

    #[test]
    fn equipped_tool_profile_falls_back_to_hands() {
        // Empty hand falls back to the synthesized HANDS_TOOL.
        let empty = local_player_holding(None);
        assert_eq!(equipped_tool_profile(&empty).kind, ToolKind::Hands);

        // A real tool returns its own profile.
        let with_pick = local_player_holding(Some(BASIC_PICKAXE_ID));
        assert_eq!(equipped_tool_profile(&with_pick).kind, ToolKind::Pickaxe);
    }

    #[test]
    fn resource_target_is_crude_only_for_hand_harvestable_nodes() {
        // Branch piles + surface stones are crude (Hands).
        let branch = target_for_node(1, BRANCH_PILE_NODE_ID);
        assert!(resource_target_is_crude(&branch));
        let stone = target_for_node(2, SURFACE_STONE_NODE_ID);
        assert!(resource_target_is_crude(&stone));

        // Ore + trees are not crude.
        let ore = target_for_node(3, COAL_NODE_ID);
        assert!(!resource_target_is_crude(&ore));
        let tree = target_for_node(4, PINE_TREE_NODE_ID);
        assert!(!resource_target_is_crude(&tree));

        // Missing / unknown definition -> false.
        assert!(!resource_target_is_crude(&PickupTargetState::default()));
        let bogus = target_for_node(5, "not_a_real_node");
        assert!(!resource_target_is_crude(&bogus));
    }

    #[test]
    fn harvest_check_matches_tool_to_node_requirement() {
        // Pickaxe vs ore vein -> allowed.
        let pick = local_player_holding(Some(BASIC_PICKAXE_ID));
        let ore = target_for_node(1, COAL_NODE_ID);
        assert!(equipped_tool_can_harvest_target(&pick, &ore));

        // Hatchet vs ore vein -> rejected (wrong tool).
        let axe = local_player_holding(Some(BASIC_HATCHET_ID));
        assert!(!equipped_tool_can_harvest_target(&axe, &ore));

        // Hatchet vs tree -> allowed.
        let tree = target_for_node(2, PINE_TREE_NODE_ID);
        assert!(equipped_tool_can_harvest_target(&axe, &tree));

        // Crude nodes are E-pickup-only: a swing (even bare hands) is
        // rejected so the player learns the quick-pickup key.
        let empty = local_player_holding(None);
        let branch = target_for_node(3, BRANCH_PILE_NODE_ID);
        assert!(!equipped_tool_can_harvest_target(&empty, &branch));
        // A real tool can't swing-harvest a crude node either.
        assert!(!equipped_tool_can_harvest_target(&pick, &branch));

        // Bare hands vs ore -> rejected.
        assert!(!equipped_tool_can_harvest_target(&empty, &ore));

        // No definition id on the target -> rejected.
        assert!(!equipped_tool_can_harvest_target(
            &pick,
            &PickupTargetState::default()
        ));
    }

    #[test]
    fn resource_target_anchor_requires_matching_node_id() {
        let target = target_for_node(42, COAL_NODE_ID);
        let anchor = resource_target_anchor(&target, 42).expect("matching id resolves an anchor");
        assert_eq!(anchor, Vec3::new(1.0, 2.0, 3.0));

        // Mismatched id -> None even though a world position exists.
        assert!(resource_target_anchor(&target, 7).is_none());

        // No world position -> None.
        let mut no_pos = target_for_node(42, COAL_NODE_ID);
        no_pos.world_position = None;
        assert!(resource_target_anchor(&no_pos, 42).is_none());
    }

    #[test]
    fn resource_target_model_resolves_definition_model() {
        let tree = target_for_node(1, PINE_TREE_NODE_ID);
        assert!(resource_target_model(&tree).is_some());
        // Unknown / missing definition -> None.
        assert!(resource_target_model(&PickupTargetState::default()).is_none());
    }

    #[test]
    fn resource_target_surface_resolves_only_for_known_definition() {
        let ore = target_for_node(1, COAL_NODE_ID);
        assert!(resource_target_surface(&ore).is_some());
        assert!(resource_target_surface(&PickupTargetState::default()).is_none());
    }

    #[test]
    fn swing_spray_direction_defaults_up_without_local_view() {
        // A default runtime has no predicted local player, so the spray
        // falls back to straight up.
        let runtime = ClientRuntime::default();
        let dir = swing_spray_direction(&runtime, Vec3::new(5.0, 0.0, 5.0));
        assert_eq!(dir, Vec3::Y);
    }

    #[test]
    fn actionbar_key_pressed_out_of_range_slot_is_false() {
        let keys = ButtonInput::<KeyCode>::default();
        let settings = ClientSettings::default();
        // Slot index past the actionbar count never maps to an action.
        assert!(!actionbar_key_pressed(
            &keys,
            &settings,
            ACTIONBAR_SLOT_COUNT + 5
        ));
    }

    #[test]
    fn send_inventory_command_reports_not_connected() {
        let mut runtime = ClientRuntime::default();
        let mut sink: Vec<String> = Vec::new();
        send_inventory_command(
            &mut runtime,
            &mut sink,
            InventoryCommand::SelectActionbarSlot { slot: 0 },
        );
        assert_eq!(sink.len(), 1);
        assert!(sink[0].contains("not connected"));
        assert!(sink[0].starts_with("inventory command failed"));
    }

    #[test]
    fn send_furnace_command_reports_not_connected() {
        let mut runtime = ClientRuntime::default();
        let mut sink: Vec<String> = Vec::new();
        send_furnace_command(
            &mut runtime,
            &mut sink,
            crate::protocol::FurnaceCommand::Open { id: 1 },
        );
        assert_eq!(sink.len(), 1);
        assert!(sink[0].contains("not connected"));
    }

    #[test]
    fn send_gameplay_message_pushes_to_both_runtime_and_sink() {
        let mut runtime = ClientRuntime::default();
        let mut sink: Vec<String> = Vec::new();
        send_gameplay_message(
            &mut runtime,
            &mut sink,
            ClientMessage::Inventory(InventoryCommand::SelectActionbarSlot { slot: 1 }),
            "test label",
        );
        assert_eq!(sink.len(), 1);
        assert!(sink[0].starts_with("test label failed: not connected"));
        // The runtime also records the error message.
        assert!(!runtime.messages.is_empty());
    }

    #[test]
    fn dead_lifecycle_matches_dying_check_used_by_the_swing_gate() {
        // The swing gate treats a Dead lifecycle as "can't swing"; verify
        // the lifecycle shape we rely on.
        let mut player = local_player_holding(Some(BASIC_HATCHET_ID));
        player.lifecycle = Some(PlayerLifecycle::Dead {
            since_tick: 0,
            killer: None,
        });
        assert!(matches!(
            player.lifecycle,
            Some(PlayerLifecycle::Dead { .. })
        ));
        // Alive (or none) does not.
        let alive = local_player_holding(Some(BASIC_HATCHET_ID));
        assert!(!matches!(
            alive.lifecycle,
            Some(PlayerLifecycle::Dead { .. })
        ));
    }
}
