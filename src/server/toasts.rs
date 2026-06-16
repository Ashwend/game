//! Shared toast envelopes.
//!
//! Every single-client toast (gather/pickup payouts, building/door/deployable
//! placement results, crafting, sleeping-bag and container warnings) funnels
//! through [`toast`] so the `ServerEnvelope` + `ServerMessage::Toast` shape
//! lives in exactly one place and the per-subsystem helpers are one-liners.

use crate::{
    items::{ItemId, item_definition},
    protocol::{ClientId, ServerMessage, ToastKind, ToastMessage},
};

use super::{DeliveryTarget, ServerEnvelope};

/// Single-client toast envelope. The single home for the `ServerEnvelope` +
/// `ServerMessage::Toast` construction; subsystem helpers delegate here.
pub(super) fn toast(
    client_id: ClientId,
    kind: ToastKind,
    text: impl Into<String>,
) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}

/// Shortcut for a [`ToastKind::Warning`] toast.
pub(super) fn warn(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    toast(client_id, ToastKind::Warning, text)
}

/// Shortcut for a [`ToastKind::Success`] toast.
pub(super) fn success(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    toast(client_id, ToastKind::Success, text)
}

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
    success(client_id, format!("+{quantity} {}", definition.name))
}

/// "Your inventory is full" warning. Sent when a pickup or gather succeeds
/// in every other respect (line of sight, valid tool, valid target) but the
/// resulting stack cannot fit in the player's bag. Without this the action
/// fails silently and the player just sees nothing happen.
pub(super) fn inventory_full_toast_envelopes(client_id: ClientId) -> Vec<ServerEnvelope> {
    warn(client_id, "Inventory is full")
}
