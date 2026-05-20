//! Shared toast envelopes for gather/pickup paths.
//!
//! Both `dropped_items` pickups and `resource_nodes` gather payouts surface
//! the same two toasts ("+N item", "inventory full"). Living in one place
//! keeps the format consistent and saves submodules from reaching across to
//! one another.

use crate::{
    items::{ItemId, item_definition},
    protocol::{ClientId, ServerMessage, ToastKind, ToastMessage},
};

use super::{DeliveryTarget, ServerEnvelope};

/// Builds the "you just acquired N items" toast envelope used by both the
/// resource gathering path and the dropped-item pickup path.
pub(super) fn item_acquired_toast_envelopes(
    client_id: ClientId,
    item_id: &ItemId,
    quantity: u16,
) -> Vec<ServerEnvelope> {
    if quantity == 0 {
        return Vec::new();
    }
    let Some(definition) = item_definition(item_id) else {
        return Vec::new();
    };
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(
            ToastKind::Success,
            format!("+{quantity} {}", definition.name),
        )),
    }]
}

/// "Your inventory is full" warning. Sent when a pickup or gather succeeds
/// in every other respect (line of sight, valid tool, valid target) but the
/// resulting stack cannot fit in the player's bag. Without this the action
/// fails silently and the player just sees nothing happen.
pub(super) fn inventory_full_toast_envelopes(client_id: ClientId) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Warning, "Inventory is full")),
    }]
}
