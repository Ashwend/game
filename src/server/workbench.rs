//! Workbench interaction: open/close, and the kind-agnostic in-place tier
//! upgrade.
//!
//! The workbench has no item slots (unlike the furnace); its only mutating
//! operation is the tier upgrade, which is deliberately generic. The handler
//! consults the compile-time upgrade table ([`crate::items::upgrade_for`]) by
//! the entity's *current* kind, so a future furnace tier (or any other
//! upgradable station) is one new table row with zero new plumbing here.
//!
//! Every mutating command re-validates the player's distance to the open
//! workbench so a client whose UI persisted after they walked away can't drive
//! an upgrade out of range, matching the lazy range re-check the furnace uses.

use crate::{
    game_balance::WORKBENCH_INTERACT_RANGE_M,
    inventory::{count_items_in_inventory, take_items_from_inventory},
    items::{DeployableKind, upgrade_for},
    protocol::{
        ClientId, DeployedEntityId, OpenWorkbenchView, ServerMessage, ToastKind, WorkbenchCommand,
    },
};

use super::{DeliveryTarget, GameServer, ServerEnvelope};

impl GameServer {
    pub(in crate::server) fn apply_workbench_command(
        &mut self,
        client_id: ClientId,
        command: WorkbenchCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            WorkbenchCommand::Open { id } => self.open_workbench(client_id, id),
            WorkbenchCommand::Close => {
                self.close_workbench(client_id);
                Vec::new()
            }
            WorkbenchCommand::Upgrade { id } => {
                if !self.open_workbench_in_range(client_id) {
                    self.close_workbench(client_id);
                    return Vec::new();
                }
                self.upgrade_deployable(client_id, id)
            }
        }
    }

    fn open_workbench(&mut self, client_id: ClientId, id: DeployedEntityId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let player_pos = client.controller.position;
        let Some(entity) = self.deployed_entities.get(&id) else {
            return workbench_toast(
                client_id,
                ToastKind::Warning,
                "Workbench not found".to_owned(),
            );
        };
        if !matches!(entity.kind, DeployableKind::Workbench { .. }) {
            return workbench_toast(client_id, ToastKind::Warning, "Not a workbench".to_owned());
        }
        if !player_pos.within_horizontal_range(entity.position, WORKBENCH_INTERACT_RANGE_M) {
            return workbench_toast(
                client_id,
                ToastKind::Warning,
                "Too far from the workbench".to_owned(),
            );
        }
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.open_workbench = Some(id);
        }
        Vec::new()
    }

    pub(in crate::server) fn close_workbench(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.open_workbench = None;
        }
    }

    /// Re-validate that the client's currently-open workbench is still within
    /// interact range. `true` when there is nothing open (no constraint) or the
    /// player is in range; `false` when the open workbench exists but the
    /// player walked away (caller should `close_workbench` and drop the
    /// command). Mirrors `open_furnace_in_range`.
    fn open_workbench_in_range(&self, client_id: ClientId) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let Some(workbench_id) = client.open_workbench else {
            return true;
        };
        let Some(entity) = self.deployed_entities.get(&workbench_id) else {
            return false;
        };
        client
            .controller
            .position
            .within_horizontal_range(entity.position, WORKBENCH_INTERACT_RANGE_M)
    }

    /// Kind-agnostic in-place upgrade: look up the upgrade row for the entity's
    /// current kind, validate range + affordability, consume the cost, and swap
    /// the kind. The mutation re-inserts the entity under the SAME id via a
    /// plain remove + insert (never the `_tracked` variant, which would untrack
    /// the chunk and close the open view) so the immutable mirror `Deployable`
    /// identity component is despawned and respawned with the new kind.
    fn upgrade_deployable(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        let Some(entity) = self.deployed_entities.get(&id) else {
            return workbench_toast(
                client_id,
                ToastKind::Warning,
                "Workbench not found".to_owned(),
            );
        };
        let Some(upgrade) = upgrade_for(entity.kind) else {
            return workbench_toast(
                client_id,
                ToastKind::Warning,
                "Nothing to upgrade here".to_owned(),
            );
        };
        let entity_position = entity.position;

        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client
            .controller
            .position
            .within_horizontal_range(entity_position, WORKBENCH_INTERACT_RANGE_M)
        {
            return workbench_toast(
                client_id,
                ToastKind::Warning,
                "Too far from the workbench".to_owned(),
            );
        }

        // Affordability check before taking anything, so a short payment never
        // drains a partial cost.
        for input in upgrade.cost {
            if count_items_in_inventory(&client.inventory, input.item_id)
                < u32::from(input.quantity)
            {
                return workbench_toast(
                    client_id,
                    ToastKind::Warning,
                    "You can't afford the upgrade".to_owned(),
                );
            }
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        for input in upgrade.cost {
            take_items_from_inventory(&mut client.inventory, input.item_id, input.quantity);
        }

        // Mutate the kind on a cloned entity and re-insert under the same id.
        // `remove_deployed_entity` + `insert_deployed_entity` keep the chunk
        // tracking intact; only the mirror `Deployable` identity respawns
        // (invisible to the client, the model swaps at the same moment anyway).
        let Some(mut entity) = self.remove_deployed_entity(id) else {
            return Vec::new();
        };
        entity.kind = upgrade.to;
        self.insert_deployed_entity(id, entity);

        workbench_toast(
            client_id,
            ToastKind::Success,
            "Workbench upgraded".to_owned(),
        )
    }

    /// Build the per-client `open_workbench` view, if any, for the
    /// per-component replication path. Carries only the id + current tier; the
    /// client renders costs from the shared upgrade table.
    pub(in crate::server) fn open_workbench_view_for(
        &self,
        client_id: ClientId,
    ) -> Option<OpenWorkbenchView> {
        let workbench_id = self.clients.get(&client_id)?.open_workbench?;
        let entity = self.deployed_entities.get(&workbench_id)?;
        let DeployableKind::Workbench { tier } = entity.kind else {
            return None;
        };
        Some(OpenWorkbenchView {
            id: workbench_id,
            tier,
        })
    }
}

fn workbench_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(crate::protocol::ToastMessage::new(kind, text)),
    }]
}

#[cfg(test)]
mod tests;
