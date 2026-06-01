use crate::{
    app::state::{ClientRuntime, ErrorToastSink},
    protocol::{ClientMessage, InventoryCommand},
};

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
pub(super) fn send_place_deployable_or_furnace_open(
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

pub(super) fn send_gameplay_message(
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
