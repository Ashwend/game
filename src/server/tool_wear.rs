//! Tool durability consumption.
//!
//! Every swing that *connects* with something (a gather payout, a player
//! hit, a structure hit) costs the active tool one point of durability;
//! whiffs are free. The three impact paths (`resource_nodes`, `combat`,
//! `deployables`) all funnel through [`GameServer::consume_active_tool_durability`]
//! so the wear rule lives in exactly one place. At zero the tool breaks:
//! the stack is removed from the actionbar and the owner gets a toast.
//! The inventory change replicates to the client through the normal
//! `PlayerPrivate` mirror refresh, no dedicated wire message needed.

use crate::{
    items::item_definition,
    protocol::{ClientId, ServerMessage, ToastKind, ToastMessage},
};

use super::{DeliveryTarget, GameServer, ServerEnvelope};

impl GameServer {
    /// Deduct one impact's wear from `client_id`'s active tool. No-op for
    /// stacks without durability (bare hands, non-tool items). Returns the
    /// "tool broke" toast envelope when the deduction empties the budget;
    /// the swing that broke the tool still produced its payout/damage, the
    /// caller applies wear after the impact's effect.
    pub(super) fn consume_active_tool_durability(
        &mut self,
        client_id: ClientId,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let active_slot = client.inventory.active_actionbar_slot;
        let Some(slot) = client.inventory.actionbar_slots.get_mut(active_slot) else {
            return Vec::new();
        };
        let Some(stack) = slot.as_mut() else {
            return Vec::new();
        };
        let Some(durability) = stack.durability.as_mut() else {
            return Vec::new();
        };

        *durability = durability.saturating_sub(1);
        if *durability > 0 {
            return Vec::new();
        }

        let broken_name = item_definition(&stack.item_id)
            .map(|definition| definition.name)
            .unwrap_or("Tool");
        *slot = None;
        vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Toast(ToastMessage::new(
                ToastKind::Warning,
                format!("Your {broken_name} broke"),
            )),
        }]
    }
}
