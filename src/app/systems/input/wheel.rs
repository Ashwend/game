//! Hold-to-open radial wheel input:
//!
//! - Building plan equipped + hold right mouse: piece-select wheel.
//! - Hammer equipped + hold right mouse on a building/door: upgrade /
//!   demolish wheel (owner-gated client-side; the server re-checks).
//! - Hold right mouse on a door (nothing placeable equipped): change-code
//!   wheel.
//! - Hold the pickup key on an owned sleeping bag: rename / pick-up wheel
//!   (a quick tap picks the bag up directly).
//!
//! While a wheel is open the camera freezes (`mouse_look_system`) and the
//! swing path is suppressed (`gameplay_inventory_shortcuts_system`); mouse
//! motion accumulates into the wheel pointer and a left click commits the
//! highlighted option. Releasing the trigger closes the wheel; the plan's
//! piece picker (`commit_on_release`) also commits the highlighted piece
//! on release because picking a piece only flips local state, while
//! wheels with real-world consequences (demolish, upgrade, code changes)
//! keep selection as an explicit click.

use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    app::state::{
        ActiveWheel, BuildingPlanState, ClientErrorToast, ClientRuntime, CurrentUser,
        DeployablePlacementState, KeyAction, LocalPlayerState, MenuState, PICKUP_HOLD_WHEEL_SECS,
        PickupHold, PickupTargetState, TextPrompt, TextPromptKind, WHEEL_POINTER_MAX_PX,
        WheelAction, WheelMenuState, WheelOption, WheelTrigger,
    },
    building::{BuildingPiece, BuildingTier, placement_cost, upgrade_cost},
    items::{BUILDING_PLAN_ID, DeployableKind, HAMMER_ID, item_definition},
    protocol::{BuildingCommand, ClientMessage, DeployedEntityId, SleepingBagCommand},
    server::{Deployable, DeployableLabel},
};

use super::{
    gating::{gameplay_accepts_controls, primary_window_focused},
    inventory_shortcuts::send_gameplay_message,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn wheel_menu_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    settings: Res<crate::app::state::ClientSettings>,
    mut wheel: ResMut<WheelMenuState>,
    mut plan: ResMut<BuildingPlanState>,
    mut menu: ResMut<MenuState>,
    mut runtime: ResMut<ClientRuntime>,
    mut error_toasts: MessageWriter<ClientErrorToast>,
    local_player: Res<LocalPlayerState>,
    pickup_target: Res<PickupTargetState>,
    placement: Res<DeployablePlacementState>,
    deployables: Query<(&Deployable, Option<&DeployableLabel>)>,
    user: Option<Res<CurrentUser>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
) {
    // The close-frame click guard only ever lasts one frame.
    wheel.closed_this_frame = false;

    if !gameplay_accepts_controls(&menu, primary_window_focused(&primary_window)) {
        wheel.closed_this_frame = wheel.active.is_some();
        wheel.active = None;
        wheel.pickup_hold = None;
        return;
    }

    // 1. Drive an already-open wheel: accumulate pointer, left click
    //    commits the highlighted option, trigger release closes without
    //    selecting.
    if let Some(active) = wheel.active.as_mut() {
        active.pointer =
            (active.pointer + mouse_motion.delta).clamp_length_max(WHEEL_POINTER_MAX_PX);
        if mouse.just_pressed(MouseButton::Left) {
            let action = active.selected_action();
            wheel.active = None;
            wheel.closed_this_frame = true;
            if let Some(action) = action {
                perform_wheel_action(
                    action,
                    &deployables,
                    &mut plan,
                    &mut menu,
                    &mut runtime,
                    &mut error_toasts,
                );
            }
            return;
        }
        let held = match active.trigger {
            WheelTrigger::RightMouse => mouse.pressed(MouseButton::Right),
            WheelTrigger::PickupKey => settings.keybindings.pressed(KeyAction::PickUp, &keys),
        };
        if held {
            return;
        }
        // Trigger released: non-intrusive wheels (the plan's piece
        // picker) commit the highlighted option right here, so picking a
        // piece is just "hold, point, let go". Wheels with real-world
        // consequences fall through and simply close.
        let release_action = active
            .commit_on_release
            .then(|| active.selected_action())
            .flatten();
        wheel.active = None;
        wheel.closed_this_frame = true;
        if let Some(action) = release_action {
            perform_wheel_action(
                action,
                &deployables,
                &mut plan,
                &mut menu,
                &mut runtime,
                &mut error_toasts,
            );
        }
        return;
    }

    let my_account = user.map(|user| user.0.account_id);
    let owner_of = |id: DeployedEntityId| {
        deployables
            .iter()
            .find(|(meta, _)| meta.id == id)
            .and_then(|(meta, _)| meta.owner)
    };

    // 2. Pickup-key hold on an owned sleeping bag: tap picks up, hold
    //    opens the rename wheel.
    if let Some(hold) = wheel.pickup_hold {
        if !settings.keybindings.pressed(KeyAction::PickUp, &keys) {
            wheel.pickup_hold = None;
            send_gameplay_message(
                &mut runtime,
                &mut error_toasts,
                ClientMessage::SleepingBag(SleepingBagCommand::PickUp { id: hold.bag_id }),
                "sleeping bag pickup",
            );
        } else {
            let elapsed = hold.elapsed + time.delta_secs();
            if elapsed >= PICKUP_HOLD_WHEEL_SECS {
                wheel.pickup_hold = None;
                wheel.active = Some(ActiveWheel {
                    title: "Sleeping Bag".to_owned(),
                    trigger: WheelTrigger::PickupKey,
                    options: vec![
                        WheelOption {
                            label: "Rename".to_owned(),
                            detail: None,
                            detail_ok: true,
                            enabled: true,
                            marked: false,
                            action: WheelAction::RenameBag(hold.bag_id),
                        },
                        WheelOption {
                            label: "Pick Up".to_owned(),
                            detail: None,
                            detail_ok: true,
                            enabled: true,
                            marked: false,
                            action: WheelAction::PickUpBag(hold.bag_id),
                        },
                    ],
                    pointer: Vec2::ZERO,
                    commit_on_release: false,
                });
            } else {
                wheel.pickup_hold = Some(PickupHold { elapsed, ..hold });
            }
        }
        return;
    }
    if settings.keybindings.just_pressed(KeyAction::PickUp, &keys)
        && matches!(
            pickup_target.deployable_kind,
            Some(DeployableKind::SleepingBag)
        )
        && let Some(bag_id) = pickup_target.deployable_id
        && my_account.is_some()
        && owner_of(bag_id) == my_account
    {
        wheel.pickup_hold = Some(PickupHold {
            bag_id,
            elapsed: 0.0,
        });
        return;
    }

    // 3. Right-mouse wheels. The deployable-ghost rotate/flip path owns
    //    right mouse while something placeable is equipped, so these only
    //    arm when it doesn't conflict.
    if !mouse.just_pressed(MouseButton::Right) {
        return;
    }
    let active_item = local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .map(|stack| stack.item_id.clone());
    let holding = |id: &str| active_item.as_deref() == Some(id);

    if holding(BUILDING_PLAN_ID) {
        wheel.active = Some(building_piece_wheel(plan.selected_piece, &local_player));
        return;
    }
    if holding(HAMMER_ID) {
        let target = pickup_target
            .deployable_id
            .zip(pickup_target.deployable_kind);
        if let Some((id, kind)) = target {
            let owned = my_account.is_some() && owner_of(id) == my_account;
            if let Some(wheel_menu) = hammer_wheel(id, kind, owned, &local_player) {
                wheel.active = Some(wheel_menu);
            }
        }
        return;
    }
    if placement.item_id.is_none()
        && matches!(pickup_target.deployable_kind, Some(DeployableKind::Door))
        && let Some(door_id) = pickup_target.deployable_id
    {
        wheel.active = Some(ActiveWheel {
            title: "Door".to_owned(),
            trigger: WheelTrigger::RightMouse,
            options: vec![WheelOption {
                label: "Change Code".to_owned(),
                detail: Some("Locks everyone else out".to_owned()),
                detail_ok: true,
                enabled: true,
                marked: false,
                action: WheelAction::ChangeDoorCode(door_id),
            }],
            pointer: Vec2::ZERO,
            commit_on_release: false,
        });
    }
}

/// Units of `item_id` in the local replicated inventory. The eligibility
/// readout for the wheel's cost lines; the server still re-checks.
fn held_quantity(local_player: &LocalPlayerState, item_id: &str) -> u32 {
    local_player
        .private
        .as_ref()
        .map(|private| crate::inventory::count_items_in_inventory(&private.inventory, item_id))
        .unwrap_or(0)
}

/// `"30 Sticks"` cost line plus whether the player can afford it.
fn cost_detail(local_player: &LocalPlayerState, cost: (&'static str, u16)) -> (String, bool) {
    let (cost_item, cost_quantity) = cost;
    let material = item_definition(cost_item)
        .map(|definition| definition.name)
        .unwrap_or(cost_item);
    let have = held_quantity(local_player, cost_item);
    let affordable = have >= u32::from(cost_quantity);
    let line = if affordable {
        format!("{cost_quantity} {material}")
    } else {
        format!("{cost_quantity} {material} (have {have})")
    };
    (line, affordable)
}

fn building_piece_wheel(selected: BuildingPiece, local_player: &LocalPlayerState) -> ActiveWheel {
    let options = BuildingPiece::ALL
        .iter()
        .map(|piece| {
            let (detail, affordable) = cost_detail(local_player, placement_cost(*piece));
            WheelOption {
                label: piece.label().to_owned(),
                detail: Some(detail),
                detail_ok: affordable,
                enabled: true,
                marked: *piece == selected,
                action: WheelAction::SelectPiece(*piece),
            }
        })
        .collect();
    ActiveWheel {
        title: "Build".to_owned(),
        trigger: WheelTrigger::RightMouse,
        options,
        pointer: Vec2::ZERO,
        // Picking a piece only flips local plan state, so releasing the
        // hold commits the highlighted piece directly.
        commit_on_release: true,
    }
}

/// Hammer wheel for a targeted building block or door. `None` when the
/// target kind has no hammer actions (workbench, furnace, bag).
/// Ineligible options (not yours, can't afford) stay selectable, the
/// server toasts the reason, but their detail line flags it up front.
fn hammer_wheel(
    id: DeployedEntityId,
    kind: DeployableKind,
    owned: bool,
    local_player: &LocalPlayerState,
) -> Option<ActiveWheel> {
    let upgrade = match kind {
        DeployableKind::Building { piece, tier } => tier.next().map(|next| {
            let (detail, affordable) = cost_detail(local_player, upgrade_cost(piece, next));
            let (detail, detail_ok) = if owned {
                (detail, affordable)
            } else {
                ("Builder only".to_owned(), false)
            };
            WheelOption {
                label: format!("Upgrade to {}", next.label()),
                detail: Some(detail),
                detail_ok,
                enabled: true,
                marked: false,
                action: WheelAction::UpgradeBuilding(id),
            }
        }),
        DeployableKind::Door => None,
        _ => return None,
    };
    let demolishable = matches!(kind, DeployableKind::Building { .. } | DeployableKind::Door);
    if !demolishable {
        return None;
    }
    let mut options = Vec::new();
    if let Some(upgrade) = upgrade {
        options.push(upgrade);
    } else if matches!(
        kind,
        DeployableKind::Building {
            tier: BuildingTier::Stone,
            ..
        }
    ) {
        options.push(WheelOption {
            label: "Top tier".to_owned(),
            detail: None,
            detail_ok: true,
            enabled: false,
            marked: false,
            action: WheelAction::UpgradeBuilding(id),
        });
    }
    let (demolish_detail, demolish_ok) = if owned {
        ("Within 15 min of placing".to_owned(), true)
    } else {
        ("Builder only".to_owned(), false)
    };
    options.push(WheelOption {
        label: "Demolish".to_owned(),
        detail: Some(demolish_detail),
        detail_ok: demolish_ok,
        enabled: true,
        marked: false,
        action: WheelAction::DemolishBuilding(id),
    });
    Some(ActiveWheel {
        title: "Hammer".to_owned(),
        trigger: WheelTrigger::RightMouse,
        options,
        pointer: Vec2::ZERO,
        // Demolish/upgrade are real writes: keep the explicit click.
        commit_on_release: false,
    })
}

fn perform_wheel_action(
    action: WheelAction,
    deployables: &Query<(&Deployable, Option<&DeployableLabel>)>,
    plan: &mut BuildingPlanState,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut MessageWriter<ClientErrorToast>,
) {
    match action {
        WheelAction::SelectPiece(piece) => {
            plan.selected_piece = piece;
        }
        WheelAction::UpgradeBuilding(id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::Building(BuildingCommand::Upgrade { id }),
            "building upgrade",
        ),
        WheelAction::DemolishBuilding(id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::Building(BuildingCommand::Demolish { id }),
            "building demolish",
        ),
        WheelAction::ChangeDoorCode(door_id) => {
            menu.text_prompt = Some(TextPrompt::new(TextPromptKind::DoorChangeCode { door_id }));
        }
        WheelAction::RenameBag(bag_id) => {
            let mut prompt = TextPrompt::new(TextPromptKind::RenameBag { bag_id });
            // Start from the bag's current name (replicated via
            // `DeployableLabel`) so renaming is an edit, not a retype.
            prompt.input = deployables
                .iter()
                .find(|(meta, _)| meta.id == bag_id)
                .and_then(|(_, label)| label.and_then(|label| label.0.clone()))
                .unwrap_or_default();
            menu.text_prompt = Some(prompt);
        }
        WheelAction::PickUpBag(id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::SleepingBag(SleepingBagCommand::PickUp { id }),
            "sleeping bag pickup",
        ),
    }
}
