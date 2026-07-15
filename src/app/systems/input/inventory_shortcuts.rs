use bevy::{
    ecs::system::SystemParam,
    input::mouse::MouseWheel,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{
        ClientErrorToast, ClientRuntime, ClientSettings, GatherInputState, InventoryUiState,
        KeyAction, MenuState, PickupTargetState, PredictionState, RangedDrawState, SwingTarget,
        ToolSwapState,
    },
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientMessage, InventoryCommand, ItemContainerSlot, LootBagCommand,
        SwingStartCommand,
    },
};

use super::gating::{gameplay_accepts_controls, primary_window_focused};

mod consumable;
mod explosive;
mod predict;
mod ranged;
mod send;
mod swing;

#[cfg(test)]
mod tests;

pub(crate) use ranged::{PredictedArrowEvent, RangedFireSampler};
pub(crate) use send::*;

pub(in crate::app::systems::input) use send::send_gameplay_message;
use send::send_place_deployable_or_furnace_open;

use predict::{predict_pickup, predict_resource_node_pickup};
use swing::{
    dispatch_swing_impact, equipped_swing, equipped_tool_can_harvest_target,
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
    ranged_input: ResMut<'w, RangedDrawState>,
    throw_charge: ResMut<'w, crate::app::state::ThrowChargeState>,
    inventory_ui: ResMut<'w, InventoryUiState>,
    menu: ResMut<'w, MenuState>,
    pickup_target: Res<'w, PickupTargetState>,
    swap_state: Res<'w, ToolSwapState>,
    settings: Res<'w, ClientSettings>,
    camera_kick: ResMut<'w, crate::app::systems::CameraImpactKick>,
    combat_feedback: ResMut<'w, crate::app::state::CombatFeedbackState>,
    wheel: Res<'w, crate::app::state::WheelMenuState>,
    error_toasts: MessageWriter<'w, ClientErrorToast>,
    play_sound: MessageWriter<'w, crate::app::audio::PlaySound>,
    predicted_arrows: MessageWriter<'w, PredictedArrowEvent>,
    primary_window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    analytics: Res<'w, crate::analytics::Analytics>,
    ranged_fire_sampler: ResMut<'w, ranged::RangedFireSampler>,
    consume_charge: ResMut<'w, crate::app::state::ConsumeChargeState>,
}

pub(crate) fn gameplay_inventory_shortcuts_system(mut params: GameplayInventoryShortcutsParams) {
    if !gameplay_accepts_controls(&params.menu, primary_window_focused(&params.primary_window)) {
        params.mouse_wheel.clear();
        params.gather_input.cancel();
        // An overlay opening (or losing focus) mid-charge abandons the bomb
        // wind-up too (no throw fires from behind a menu), and abandons a
        // half-wrapped bandage: opening the inventory must not finish binding a
        // wound for you. Costs nothing, the item is only spent on completion.
        params.throw_charge.cancel();
        consumable::cancel_if_active(&mut params);
        // An overlay opening (or losing focus) mid-draw abandons the shot: send a
        // DrawCancel so the server lowers the bow and restores movement, instead of
        // leaving the player stuck at draw speed behind the menu. The local reload
        // clock keeps burning through the overlay, mirroring the server's cooldown
        // (gameplay never pauses; only controls are gated here).
        ranged::idle_tick_and_cancel(&mut params);
        return;
    }
    // While a radial wheel is open the mouse drives the wheel pointer
    // and E may be the wheel's hold trigger; swings, pickups, and slot
    // selection all wait for the wheel to close.
    if params.wheel.blocks_input() {
        params.mouse_wheel.clear();
        params.gather_input.cancel();
        params.throw_charge.cancel();
        consumable::cancel_if_active(&mut params);
        ranged::idle_tick_and_cancel(&mut params);
        return;
    }

    let active_actionbar_slot = params
        .local_player
        .private
        .as_ref()
        .map(|private| private.inventory.active_actionbar_slot);
    for slot in 0..ACTIONBAR_SLOT_COUNT {
        if actionbar_key_pressed(&params.keys, &params.settings, slot) {
            // The selection tick only fires when the slot actually
            // changes; re-pressing the active slot's key stays silent.
            if active_actionbar_slot != Some(slot) {
                params
                    .play_sound
                    .write(crate::app::audio::PlaySound::non_spatial(
                        crate::app::audio::SoundId::HotbarSelect,
                    ));
            }
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::SelectActionbarSlot { slot },
            );
        }
    }

    let wheel_delta = wheel_step(params.mouse_wheel.read().map(|event| (event.x, event.y)));
    if wheel_delta != 0 {
        // Wheel scrolling always lands on a different slot (the offset
        // wraps), so it always ticks.
        params
            .play_sound
            .write(crate::app::audio::PlaySound::non_spatial(
                crate::app::audio::SoundId::HotbarSelect,
            ));
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
        // The drop shortcut is the one audible loss that happens with every
        // item UI closed; the intent window lets its cue through while
        // ammo/charge consumption stays silent.
        params.inventory_ui.note_drop_intent();
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
        } else if let Some(projectile_id) = params.pickup_target.projectile_id {
            // Pull a stuck (at-rest) arrow back into the bag before its despawn
            // TTL. Not predicted: the grant arrives via the normal inventory
            // replication + acquisition toast, and a rejected recovery (someone
            // else grabbed it first, bag full) simply leaves the world as-is.
            send_inventory_command(
                &mut params.runtime,
                &mut params.error_toasts,
                InventoryCommand::RecoverProjectile { projectile_id },
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
            // server-side interactive view. Furnace opens its smelt grid;
            // workbench opens its upgrade UI. Both track a per-client open
            // pointer server-side and reply via `PlayerPrivate`. Other
            // deployable kinds no-op for now.
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
                    crate::app::systems::input::send_workbench_command(
                        &mut params.runtime,
                        &mut params.error_toasts,
                        crate::protocol::WorkbenchCommand::Open { id },
                    );
                }
                // Door E (tap = open / code prompt, hold = pick-up wheel)
                // lives in the hold-aware `super::super::wheel` path, like
                // the sleeping bag and cupboard; nothing fires on plain
                // press so a hold can't also toggle the door open.
                Some(DeployableKind::Door { .. }) => {}
                // Sleeping bag E handling (tap = pick up, hold = rename
                // wheel) lives in the hold-aware path in
                // `super::super::wheel`; nothing fires on plain press.
                Some(DeployableKind::SleepingBag) => {}
                // Storage boxes and ruin caches share the container view: both
                // store their slots on the deployable and open through the same
                // `OpenStorageBox` message (the server accepts either kind). The
                // server validates range + kind and replies by populating
                // `PlayerPrivate.open_loot_bag` (shared container view), so the
                // transfer panel appears on the next replication tick.
                Some(DeployableKind::StorageBox { .. } | DeployableKind::RuinCache) => {
                    send_gameplay_message(
                        &mut params.runtime,
                        &mut params.error_toasts,
                        ClientMessage::OpenStorageBox { id },
                        "container open",
                    );
                }
                // Tool Cupboard E (tap = authorize/deauthorize yourself,
                // hold = options wheel) lives in the hold-aware
                // `super::super::wheel` path, like the sleeping bag;
                // nothing fires on plain press here.
                Some(DeployableKind::ToolCupboard) => {}
                // Building blocks, torches, and armed charges have no E
                // interaction on plain press; the hammer is the building
                // interface, a torch is just a light, and a charge's defuse is
                // the hold-E wheel (in the explosive VFX package).
                Some(
                    DeployableKind::Building { .. }
                    | DeployableKind::Torch { .. }
                    | DeployableKind::Explosive { .. },
                )
                | None => {}
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
        } else if params.pickup_target.sleeping_player.is_some() {
            // Loot a logged-out sleeping body. The server spills their pack
            // into a loot bag at their feet and opens it, so the transfer
            // rides the same `PlayerPrivate.open_loot_bag` path. Only
            // `sleeping_player` targets route here; a live (awake) player
            // isn't lootable.
            if let Some(target_id) = params.pickup_target.player_id {
                send_gameplay_message(
                    &mut params.runtime,
                    &mut params.error_toasts,
                    ClientMessage::LootSleeper {
                        client_id: target_id,
                    },
                    "loot sleeper",
                );
            }
        }
    }

    // Tool-swap entry locks out swings, the new tool is still being
    // lifted into view, so it can't be used yet. Death does the same:
    // a corpse can't swing.
    let local_dead = matches!(
        params.local_player.lifecycle,
        Some(crate::server::PlayerLifecycle::Dead { .. })
    );

    // Ranged weapons (bow, crossbow) run their own draw/fire/reload loop, not the
    // melee swing state machine. When one is held, drive it and skip the melee path
    // entirely (a ranged weapon has no ToolProfile/WeaponProfile, so `equipped_swing`
    // already returns None for it and the swing below would be a no-op, but taking
    // this branch also keeps any stale melee swing cancelled and drives the draw).
    let swapping = params.swap_state.is_swapping();
    // A thrown explosive (powder bomb) intercepts before both the ranged and the
    // melee paths: it neither draws nor lands a melee hit. It DOES drive the swing
    // state machine (with the `ThrownBomb` archetype) so its overhand toss pose
    // plays, the release cue + throw fire at the pose's release frame, and the
    // recovery beat gates re-throw; so we do NOT cancel the swing here, and we
    // still drain the SwingStart below so peers see the toss.
    // A consumable (the bandage) intercepts before every other path: it neither
    // swings, fires, nor throws. It does NOT drive the swing state machine at all
    // (unlike the thrown bomb, whose toss IS a swing), so cancel any stale melee
    // swing left over from a just-swapped-away tool and claim the frame.
    if consumable::drive_consumable_input(&mut params, local_dead, swapping) {
        params.gather_input.cancel();
        return;
    }
    if explosive::drive_explosive_input(&mut params, local_dead, swapping) {
        if let Some((seq, model)) = params.gather_input.take_swing_start() {
            send_gameplay_message(
                &mut params.runtime,
                &mut params.error_toasts,
                ClientMessage::SwingStart(SwingStartCommand { seq, model }),
                "swing start",
            );
        }
        return;
    }
    if ranged::drive_ranged_input(&mut params, local_dead, swapping) {
        // A ranged weapon is active: it does not swing, so no SwingStart is queued
        // and the melee dispatch is skipped. Make sure a leftover melee swing from a
        // just-swapped-away tool is cleared.
        params.gather_input.cancel();
        return;
    }

    // The swing archetype (for timing/poses) paired with the impact identity
    // (`ToolKind`, or the interim `Hands` default for weapons). `Some` for any
    // real tool or weapon, `None` for bare hands / non-combat items, so the swing
    // start below gates on it exactly as it did on the old tool kind.
    let equipped = if params.swap_state.is_swapping() || local_dead {
        params.gather_input.cancel();
        None
    } else {
        equipped_swing(&params.local_player)
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
            && equipped.is_some()
        {
            Some(SwingTarget::Player(player_id))
        } else if let Some(node_id) = params.pickup_target.resource_node_id.filter(|_| {
            equipped_tool_can_harvest_target(&params.local_player, &params.pickup_target)
        }) {
            Some(SwingTarget::ResourceNode(node_id))
        } else if let Some(deployable_id) = params.pickup_target.deployable_id
            && equipped.is_some()
        {
            Some(SwingTarget::Deployable(deployable_id))
        } else {
            None
        };
    // Dev combat-feel timing scales (neutral by default, so a release build and an
    // untouched dev session swing exactly as shipped). Read straight off settings
    // each frame, the same direct-read pattern the lighting sliders use.
    let feel = crate::app::state::SwingFeelScales {
        duration_scale: params.settings.dev.combat.swing_duration_scale,
        impact_fraction_offset: params.settings.dev.combat.impact_fraction_offset,
    };
    let impact = params.gather_input.update(
        params.time.delta_secs(),
        params.mouse_buttons.just_pressed(MouseButton::Left),
        params.mouse_buttons.pressed(MouseButton::Left),
        equipped,
        target,
        feel,
    );
    if let Some(impact) = impact {
        dispatch_swing_impact(&mut params, impact);
    }
    // Tell the server a swing began (cosmetic): it stamps the swinger's
    // peer-visible PlayerAction so other players see the matching third-person
    // swing on the rigged body. Fires on whiffs too; the impact dispatch above
    // only handles swings that connect.
    if let Some((seq, model)) = params.gather_input.take_swing_start() {
        send_gameplay_message(
            &mut params.runtime,
            &mut params.error_toasts,
            ClientMessage::SwingStart(SwingStartCommand { seq, model }),
            "swing start",
        );
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

/// Collapse a frame's mouse-wheel deltas to a single hotbar step (`-1`, `0`, or
/// `+1`).
///
/// Two macOS/winit subtleties make the naive `event.y.signum()` wrong:
/// 1. Holding **Shift** makes the platform deliver the wheel as a *horizontal*
///    scroll, so the magnitude lands on `event.x` and `event.y` becomes `0.0`.
///    Reading only `y` would then see nothing meaningful from a Shift+scroll.
/// 2. `f32::signum(0.0)` returns `+1.0` (it is a two-way sign, never `0`). So the
///    old per-event `event.y.signum()` mapped every Shift+scroll event (with
///    `y == 0.0`) to `+1`, the frame summed positive, and the hotbar locked to a
///    single direction regardless of which way the user scrolled.
///
/// Fix: read whichever axis carries the gesture, accumulate the signed
/// magnitude, and sign the *total* once. A genuinely empty frame sums to `0.0`
/// and yields `0` (no step), exactly as before.
fn wheel_step(deltas: impl Iterator<Item = (f32, f32)>) -> i8 {
    let raw: f32 = deltas.map(|(x, y)| if y != 0.0 { y } else { x }).sum();
    if raw > 0.0 {
        1
    } else if raw < 0.0 {
        -1
    } else {
        0
    }
}

fn actionbar_key_pressed(
    keys: &ButtonInput<KeyCode>,
    settings: &ClientSettings,
    slot: usize,
) -> bool {
    ACTIONBAR_ACTIONS
        .get(slot)
        .is_some_and(|action| settings.keybindings.just_pressed(*action, keys))
}
