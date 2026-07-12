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
    ecs::system::SystemParam,
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{PrimaryWindow, Window},
};

use crate::{
    analytics::{Analytics, Event},
    app::state::{
        ActiveWheel, BuildingPlanState, ClientErrorToast, ClientRuntime, CupboardAuthState,
        CurrentUser, DeployablePlacementState, KeyAction, LocalPlayerState, MenuState,
        PICKUP_HOLD_WHEEL_SECS, PickupHold, PickupHoldKind, PickupTargetState, TextPrompt,
        TextPromptKind, WHEEL_POINTER_MAX_PX, WheelAction, WheelMenuState, WheelOption,
        WheelTrigger,
    },
    building::{BuildingPiece, placement_cost, upgrade_cost},
    items::{BUILDING_PLAN_ID, DeployableKind, HAMMER_ID, item_definition},
    protocol::{
        BuildingCommand, ClaimCommand, ClientMessage, DeployedEntityId, DoorCommand,
        ExplosiveCommand, SleepingBagCommand,
    },
    server::{Deployable, DeployableLabel},
};

use super::{
    gating::{gameplay_accepts_controls, primary_window_focused},
    inventory_shortcuts::send_gameplay_message,
};

/// Read-only context grouped into one [`SystemParam`] so `wheel_menu_system`
/// stays under Bevy's per-system parameter limit: the current account (for the
/// owner-authorized wheel options) and the analytics sink (for `explosive_defused`).
#[derive(SystemParam)]
pub(crate) struct WheelContext<'w> {
    user: Option<Res<'w, CurrentUser>>,
    analytics: Res<'w, Analytics>,
}

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
    context: WheelContext,
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
                    &context.analytics,
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
                &context.analytics,
            );
        }
        return;
    }

    let my_account = context.user.as_ref().map(|user| user.0.account_id);
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
            // Quick release: run the kind's tap action.
            wheel.pickup_hold = None;
            match hold.kind {
                PickupHoldKind::SleepingBag => send_gameplay_message(
                    &mut runtime,
                    &mut error_toasts,
                    ClientMessage::SleepingBag(SleepingBagCommand::PickUp { id: hold.id }),
                    "sleeping bag pickup",
                ),
                PickupHoldKind::ToolCupboard => {
                    if let Some(command) = cupboard_tap_command(hold.id, &pickup_target) {
                        send_gameplay_message(
                            &mut runtime,
                            &mut error_toasts,
                            ClientMessage::Claim(command),
                            "cupboard auth",
                        );
                    }
                }
                // Quick tap on a door is the open/close interact (the
                // server prompts for the code if the player isn't yet
                // authorized), exactly the old plain-E behaviour.
                PickupHoldKind::Door => send_gameplay_message(
                    &mut runtime,
                    &mut error_toasts,
                    ClientMessage::Door(DoorCommand::Interact { id: hold.id }),
                    "door interact",
                ),
                // A charge has no tap action: you either hold-E to defuse it or
                // leave it. A quick release does nothing so a mistaken tap can
                // never accidentally arm a defuse.
                PickupHoldKind::Explosive => {}
            }
        } else {
            let elapsed = hold.elapsed + time.delta_secs();
            if elapsed >= PICKUP_HOLD_WHEEL_SECS {
                wheel.pickup_hold = None;
                wheel.active = Some(match hold.kind {
                    PickupHoldKind::SleepingBag => sleeping_bag_wheel(hold.id),
                    PickupHoldKind::ToolCupboard => cupboard_wheel(hold.id, &pickup_target),
                    PickupHoldKind::Door => door_pickup_wheel(hold.id),
                    PickupHoldKind::Explosive => charge_wheel(hold.id),
                });
            } else {
                wheel.pickup_hold = Some(PickupHold { elapsed, ..hold });
            }
        }
        return;
    }
    if settings.keybindings.just_pressed(KeyAction::PickUp, &keys) {
        // Sleeping bag: only the owner's tap (pick up) / hold (wheel)
        // does anything.
        if matches!(
            pickup_target.deployable_kind,
            Some(DeployableKind::SleepingBag)
        ) && let Some(bag_id) = pickup_target.deployable_id
            && my_account.is_some()
            && owner_of(bag_id) == my_account
        {
            wheel.pickup_hold = Some(PickupHold {
                id: bag_id,
                kind: PickupHoldKind::SleepingBag,
                elapsed: 0.0,
            });
            return;
        }
        // Tool Cupboard: anyone in reach may tap to toggle their own
        // authorization, or hold for the clear / authorize wheel.
        if matches!(
            pickup_target.deployable_kind,
            Some(DeployableKind::ToolCupboard)
        ) && let Some(id) = pickup_target.deployable_id
        {
            wheel.pickup_hold = Some(PickupHold {
                id,
                kind: PickupHoldKind::ToolCupboard,
                elapsed: 0.0,
            });
            return;
        }
        // Door: tap toggles open / prompts for the code, hold opens the
        // pick-up wheel. Routed through the hold timer (not the plain-E
        // path in `inventory_shortcuts`) so a hold never also fires the
        // open toggle.
        if matches!(
            pickup_target.deployable_kind,
            Some(DeployableKind::Door { .. })
        ) && let Some(id) = pickup_target.deployable_id
        {
            wheel.pickup_hold = Some(PickupHold {
                id,
                kind: PickupHoldKind::Door,
                elapsed: 0.0,
            });
            return;
        }
        // Placed charge: hold-E opens the defuse wheel (there is no tap action).
        // The option is always offered; the server re-checks reach + claim
        // authorization and answers any failure with a toast, so there is no
        // client-side gate to keep in sync (the same pattern the door pickup
        // wheel uses).
        if matches!(
            pickup_target.deployable_kind,
            Some(DeployableKind::Explosive { .. })
        ) && let Some(id) = pickup_target.deployable_id
        {
            wheel.pickup_hold = Some(PickupHold {
                id,
                kind: PickupHoldKind::Explosive,
                elapsed: 0.0,
            });
            return;
        }
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
        if let Some((id, kind)) = target
            && let Some(wheel_menu) = hammer_wheel(
                id,
                kind,
                pickup_target.deployable_can_modify,
                pickup_target.deployable_demolishable,
                &local_player,
            )
        {
            wheel.active = Some(wheel_menu);
        }
        return;
    }
    if placement.item_id.is_none()
        && matches!(
            pickup_target.deployable_kind,
            Some(DeployableKind::Door { .. })
        )
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

/// `"30 Wood"` cost line plus whether the player can afford it.
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

/// Hammer wheel for a targeted building block or door. Only options the
/// player can actually perform are shown: upgrade (while a higher tier
/// exists) and demolish (while still within the demolish window), and both
/// only when the player is authorized to modify the piece. Returns `None`
/// when nothing is offered, so the wheel never opens with a dead option
/// (and an unauthorized player's right-click does nothing).
fn hammer_wheel(
    id: DeployedEntityId,
    kind: DeployableKind,
    can_modify: bool,
    demolishable: bool,
    local_player: &LocalPlayerState,
) -> Option<ActiveWheel> {
    if !can_modify {
        return None;
    }
    let mut options = Vec::new();
    // Upgrade: only while a higher tier exists. The cost line still flags
    // affordability, but an unaffordable upgrade stays selectable (go
    // gather; the server re-checks).
    if let DeployableKind::Building { piece, tier } = kind
        && let Some(next) = tier.next()
    {
        let (detail, affordable) = cost_detail(local_player, upgrade_cost(piece, next));
        options.push(WheelOption {
            label: format!("Upgrade to {}", next.label()),
            detail: Some(detail),
            detail_ok: affordable,
            enabled: true,
            marked: false,
            action: WheelAction::UpgradeBuilding(id),
        });
    }
    // Demolish: only while the piece is still within its demolish window.
    if demolishable
        && matches!(
            kind,
            DeployableKind::Building { .. } | DeployableKind::Door { .. }
        )
    {
        options.push(WheelOption {
            label: "Demolish".to_owned(),
            detail: None,
            detail_ok: true,
            enabled: true,
            marked: false,
            action: WheelAction::DemolishBuilding(id),
        });
    }
    if options.is_empty() {
        return None;
    }
    Some(ActiveWheel {
        title: "Hammer".to_owned(),
        trigger: WheelTrigger::RightMouse,
        options,
        pointer: Vec2::ZERO,
        // Demolish/upgrade are real writes: keep the explicit click.
        commit_on_release: false,
    })
}

/// The sleeping bag's hold-E wheel (rename / pick up).
fn sleeping_bag_wheel(bag_id: DeployedEntityId) -> ActiveWheel {
    ActiveWheel {
        title: "Sleeping Bag".to_owned(),
        trigger: WheelTrigger::PickupKey,
        options: vec![
            WheelOption {
                label: "Rename".to_owned(),
                detail: None,
                detail_ok: true,
                enabled: true,
                marked: false,
                action: WheelAction::RenameBag(bag_id),
            },
            WheelOption {
                label: "Pick Up".to_owned(),
                detail: None,
                detail_ok: true,
                enabled: true,
                marked: false,
                action: WheelAction::PickUpBag(bag_id),
            },
        ],
        pointer: Vec2::ZERO,
        commit_on_release: false,
    }
}

/// The door's hold-E wheel: pick the door back into inventory. The option
/// is always offered; the server re-checks claim authorization and that
/// the player has unlocked the door (knows the code), answering any
/// failure with a toast, so there's no client-side gate to keep in sync.
fn door_pickup_wheel(door_id: DeployedEntityId) -> ActiveWheel {
    ActiveWheel {
        title: "Door".to_owned(),
        trigger: WheelTrigger::PickupKey,
        options: vec![WheelOption {
            label: "Pick Up".to_owned(),
            detail: Some("Back to inventory".to_owned()),
            detail_ok: true,
            enabled: true,
            marked: false,
            action: WheelAction::PickUpDoor(door_id),
        }],
        pointer: Vec2::ZERO,
        // Picking up removes the door from the world: keep the explicit click.
        commit_on_release: false,
    }
}

/// The placed charge's hold-E wheel: defuse the live charge. The option is
/// always offered; the server re-checks reach + claim authorization and refunds
/// half the materials on success, answering any failure with a toast, so (as
/// with the door pickup wheel) there is no client-side gate to keep in sync.
fn charge_wheel(charge_id: DeployedEntityId) -> ActiveWheel {
    ActiveWheel {
        title: "Charge".to_owned(),
        trigger: WheelTrigger::PickupKey,
        options: vec![WheelOption {
            label: "Defuse".to_owned(),
            detail: Some("Recover half the materials".to_owned()),
            detail_ok: true,
            enabled: true,
            marked: false,
            action: WheelAction::DefuseCharge(charge_id),
        }],
        pointer: Vec2::ZERO,
        // Defusing removes the charge from the world: keep the explicit click.
        commit_on_release: false,
    }
}

/// The Tool Cupboard's hold-E wheel. Options depend on the local
/// player's authorization: an unauthorized player gets "Authorize Me", an
/// authorized player gets "Remove Myself" + "Clear List".
fn cupboard_wheel(id: DeployedEntityId, pickup_target: &PickupTargetState) -> ActiveWheel {
    let auth = (pickup_target.deployable_id == Some(id))
        .then_some(pickup_target.deployable_cupboard_auth)
        .flatten();
    let option = |label: &str, detail: Option<&str>, action: WheelAction| WheelOption {
        label: label.to_owned(),
        detail: detail.map(str::to_owned),
        detail_ok: true,
        enabled: true,
        marked: false,
        action,
    };
    let mut options = Vec::new();
    match auth {
        Some(CupboardAuthState::Authorized) => {
            options.push(option(
                "Remove Myself",
                None,
                WheelAction::DeauthorizeCupboard(id),
            ));
            options.push(option(
                "Clear List",
                Some("Deauthorize everyone else"),
                WheelAction::ClearCupboard(id),
            ));
        }
        Some(CupboardAuthState::Unauthorized) | None => {
            options.push(option(
                "Authorize Me",
                None,
                WheelAction::AuthorizeCupboard(id),
            ));
        }
    }
    ActiveWheel {
        title: "Tool Cupboard".to_owned(),
        trigger: WheelTrigger::PickupKey,
        options,
        pointer: Vec2::ZERO,
        commit_on_release: false,
    }
}

/// The claim command a quick tap-E should send, given the cupboard's
/// current auth state. `None` for the owner (always authorized) or when
/// the player is no longer looking at this cupboard.
fn cupboard_tap_command(
    id: DeployedEntityId,
    pickup_target: &PickupTargetState,
) -> Option<ClaimCommand> {
    if pickup_target.deployable_id != Some(id) {
        return None;
    }
    match pickup_target.deployable_cupboard_auth {
        Some(CupboardAuthState::Authorized) => Some(ClaimCommand::DeauthorizeSelf { id }),
        Some(CupboardAuthState::Unauthorized) | None => Some(ClaimCommand::AuthorizeSelf { id }),
    }
}

fn perform_wheel_action(
    action: WheelAction,
    deployables: &Query<(&Deployable, Option<&DeployableLabel>)>,
    plan: &mut BuildingPlanState,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    error_toasts: &mut MessageWriter<ClientErrorToast>,
    analytics: &Analytics,
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
        WheelAction::PickUpDoor(door_id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::Door(DoorCommand::PickUp { id: door_id }),
            "door pickup",
        ),
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
        WheelAction::AuthorizeCupboard(id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::Claim(ClaimCommand::AuthorizeSelf { id }),
            "cupboard authorize",
        ),
        WheelAction::DeauthorizeCupboard(id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::Claim(ClaimCommand::DeauthorizeSelf { id }),
            "cupboard deauthorize",
        ),
        WheelAction::ClearCupboard(id) => send_gameplay_message(
            runtime,
            error_toasts,
            ClientMessage::Claim(ClaimCommand::ClearList { id }),
            "cupboard clear",
        ),
        WheelAction::DefuseCharge(id) => {
            // Resolve the charge kind from the replicated deployable so the event
            // carries which charge was defused (the wheel action only holds the id).
            if let Some(kind) = deployables.iter().find_map(|(meta, _)| {
                (meta.id == id)
                    .then_some(meta.kind)
                    .and_then(|kind| match kind {
                        DeployableKind::Explosive { kind } => Some(kind),
                        _ => None,
                    })
            }) {
                analytics.track(Event::ExplosiveDefused {
                    kind: kind.item_id().to_owned(),
                });
            }
            send_gameplay_message(
                runtime,
                error_toasts,
                ClientMessage::Explosive(ExplosiveCommand::Defuse { id }),
                "defuse charge",
            );
        }
    }
}
