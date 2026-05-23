use crate::{
    protocol::SteamId,
    save::{PersistedPlayer, WorldSave, WorldStateSave},
};

use super::{GameServer, persisted_player_from};

impl GameServer {
    pub fn world_save(&self) -> WorldSave {
        let mut save = self.save.clone();
        let mut persisted = self.persisted_players.clone();
        // Capture any currently connected players' live state before writing.
        for client in self.clients.values() {
            persisted.insert(client.steam_id, persisted_player_from(client));
        }
        let mut players = persisted.into_values().collect::<Vec<_>>();
        players.sort_by_key(|player| player.steam_id);

        let mut dropped_items = self
            .dropped_items
            .values()
            .map(|body| body.item.clone())
            .collect::<Vec<_>>();
        dropped_items.sort_by_key(|item| item.id);

        let mut resource_nodes = self.resource_nodes.values().cloned().collect::<Vec<_>>();
        resource_nodes.sort_by_key(|node| node.id);

        save.state = WorldStateSave {
            last_authoritative_tick: self.tick,
            players,
            dropped_items,
            resource_nodes: Some(resource_nodes),
            next_dropped_item_id: self.next_dropped_item_id,
            next_client_id: self.next_client_id,
            next_resource_node_id: self.next_resource_node_id,
            world_time_seconds_of_day: self.world_time.seconds_of_day,
            world_time_multiplier: self.world_time.multiplier,
        };
        save
    }

    pub(super) fn take_persisted_player(&mut self, steam_id: SteamId) -> Option<PersistedPlayer> {
        self.persisted_players.remove(&steam_id)
    }

    pub(super) fn remember_player(&mut self, player: PersistedPlayer) {
        self.persisted_players.insert(player.steam_id, player);
    }
}
