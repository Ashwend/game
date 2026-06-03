use bevy::{
    ecs::system::SystemParam,
    input::mouse::MouseWheel,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{
        ClientErrorToast, ClientRuntime, ClientSettings, GatherInputState, InventoryUiState,
        KeyAction, MenuState, PickupTargetState, PredictionState, SwingTarget, ToolSwapState,
    },
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientMessage, InventoryCommand, ItemContainerSlot, LootBagCommand,
    },
};

use super::gating::{gameplay_accepts_controls, primary_window_focused};

mod predict;
mod send;
mod swing;

#[cfg(test)]
mod tests;

pub(crate) use send::*;

use send::{send_gameplay_message, send_place_deployable_or_furnace_open};

use predict::{predict_pickup, predict_resource_node_pickup};
use swing::{
    dispatch_swing_impact, equipped_tool_can_harvest_target, equipped_tool_kind,
    resource_target_is_crude,
};

#[derive(SystemParam)]
pub(crate) struct GameplayInventoryShortcutsParams<'w, 's> {
    commands: Commands<'w, 's>,
    time: Res<'w, Time>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    mouse_buttons: Res<'w, ButtonInput<MouseButton>>,
    mouse_wheel: MessageReader<'w, 's, MouseWheel>,
    runtime: ResMut<'w, ClientRuntime>,
    local_player: Res<'w, crate::app::state::LocalPlayerState>,
    prediction: ResMut<'w, PredictionState>,
    gather_input: ResMut<'w, GatherInputState>,
    inventory_ui: ResMut<'w, InventoryUiState>,
    menu: ResMut<'w, MenuState>,
    crafting_ui: ResMut<'w, crate::app::state::CraftingUiState>,
    pickup_target: Res<'w, PickupTargetState>,
    swap_state: Res<'w, ToolSwapState>,
    settings: Res<'w, ClientSettings>,
    camera_kick: ResMut<'w, crate::app::systems::CameraImpactKick>,
    combat_feedback: ResMut<'w, crate::app::state::CombatFeedbackState>,
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
        let from = ItemContainerSlot::actionbar(active_actionbar_slot);
        // Predict the bag removal instantly; the dropped entity itself still
        // appears via server replication (no local ground ghost in Tier 1).
        let seq = params.prediction.alloc_seq();
        params.prediction.push_drop(seq, from, Some(1));
        send_inventory_command(
            &mut params.runtime,
            &mut params.error_toasts,
            InventoryCommand::Drop {
                from,
                quantity: Some(1),
                seq,
            },
        );
    }

    if params
        .settings
        .keybindings
        .just_pressed(KeyAction::PickUp, &params.keys)
    {
        if let Some(dropped_item_id) = params.pickup_target.dropped_item_id {
            // Predict the gain instantly and (when the whole stack fits) hide
            // the world item. A rejected/partial pickup reconciles when the
            // server advances `applied_action_seq`: the add evaporates / the
            // item un-hides. `seq == 0` means "not predicted" (unknown stack
            // or full bag), the server still processes the command.
            let seq = predict_pickup(
                &mut params.prediction,
                &params.local_player,
                dropped_item_id,
                &params.pickup_target,
            );
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::PickUp {
                    dropped_item_id,
                    seq,
                },
            );
            params.inventory_ui.note_pickup_intent();
        } else if let Some(resource_node_id) = params.pickup_target.resource_node_id
            && resource_target_is_crude(&params.pickup_target)
        {
            // Crude nodes (branches, surface stones, grass tufts) can be
            // picked up with E. Predict the full drain into the bag and,
            // when the whole node fits, hide the world visual instantly,
            // exactly like a dropped-item pickup. The server gates on the
            // same crude check and a view-ray ping, so a rejected pickup
            // reverts (and the node un-hides) when `applied_action_seq`
            // advances. `seq == 0` means "not predicted" (full bag); the
            // server still processes the command.
            let seq = predict_resource_node_pickup(
                &mut params.prediction,
                &params.local_player,
                resource_node_id,
                &params.pickup_target,
            );
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::PickUpResourceNode {
                    resource_node_id,
                    seq,
                },
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

    // Tool-swap entry locks out swings, the new tool is still being
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
    //     intent is unambiguous, if you're aiming at the avatar of
    //     someone running past a tree, that's the target you mean.
    //     Gated on a real tool being equipped (bare hands deal no PvP
    //     damage; the server rejects too).
    //  2. A resource node the held tool can actually harvest. Wrong-
    //     tool nodes turn into "no target" so the impact frame resolves
    //     to a clean miss instead of a hit the server would reject.
    //  3. A placed structure the player is aimed at. Reaching this
    //     branch already implies a real tool is equipped, bare hands
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
