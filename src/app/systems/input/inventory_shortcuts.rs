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
            ImpactEffectKind, KeyAction, MenuState, PendingAudioCue, PendingImpactEffect,
            PickupTargetState, SwingImpact, ToolSwapState,
        },
    },
    items::{HANDS_TOOL, ToolKind, ToolProfile, item_definition},
    protocol::{
        ACTIONBAR_SLOT_COUNT, ClientMessage, InventoryCommand, ItemContainerSlot,
        ResourceGatherCommand,
    },
    resources::resource_node_definition,
};

use super::gating::{gameplay_accepts_controls, primary_window_focused};

#[derive(SystemParam)]
pub(crate) struct GameplayInventoryShortcutsParams<'w, 's> {
    time: Res<'w, Time>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    mouse_buttons: Res<'w, ButtonInput<MouseButton>>,
    mouse_wheel: MessageReader<'w, 's, MouseWheel>,
    runtime: ResMut<'w, ClientRuntime>,
    gather_input: ResMut<'w, GatherInputState>,
    menu: Res<'w, MenuState>,
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
            .runtime
            .local_player()
            .and_then(|player| player.inventory.as_ref())
            .map(|inventory| inventory.active_actionbar_slot)
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
        }
    }

    // Tool-swap entry locks out swings — the new tool is still being
    // lifted into view, so it can't be used yet.
    let equipped_tool = if params.swap_state.is_swapping() {
        params.gather_input.cancel();
        None
    } else {
        equipped_tool_kind(&params.runtime)
    };
    // Treat an unharvestable target (wrong tool for this node) as no
    // target at all so the impact frame resolves to a clean miss instead
    // of a hit attempt the server would just reject.
    let target = params
        .pickup_target
        .resource_node_id
        .filter(|_| equipped_tool_can_harvest_target(&params.runtime, &params.pickup_target));
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

fn equipped_tool_kind(runtime: &ClientRuntime) -> Option<ToolKind> {
    Some(equipped_tool_profile(runtime).kind)
}

/// Resolve the active actionbar item to a tool profile, falling back to
/// the synthesized [`HANDS_TOOL`] when no tool is held. The server runs
/// the same fallback in `apply_gather_command`, so the client's hit
/// check and the server's payout decision stay aligned for crude
/// (hand-harvestable) nodes.
fn equipped_tool_profile(runtime: &ClientRuntime) -> ToolProfile {
    runtime
        .local_player()
        .and_then(|player| player.inventory.as_ref())
        .and_then(|inventory| inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|definition| definition.tool)
        .unwrap_or(HANDS_TOOL)
}

// Single decision point for the swing: a miss queues only the whoosh, a hit
// queues the spatial impact sound, visual chips, camera kick, and the gather
// command. The swing state guarantees at most one impact event per swing, so
// hit and miss audio can never both play.
fn dispatch_swing_impact(params: &mut GameplayInventoryShortcutsParams, impact: SwingImpact) {
    let Some(node_id) = impact.target else {
        params.gather_input.set_pending_miss_audio();
        return;
    };

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

fn equipped_tool_can_harvest_target(runtime: &ClientRuntime, target: &PickupTargetState) -> bool {
    let profile = equipped_tool_profile(runtime);
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
