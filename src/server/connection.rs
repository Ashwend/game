use anyhow::{Result, bail};

use crate::{
    controller::PlayerController,
    protocol::{
        ClientId, GAME_VERSION, PROTOCOL_VERSION, PlayerEvent, PlayerState, ServerMessage, SteamId,
        WorldSnapshot,
    },
    steam::verify_auth_ticket,
};

use super::{
    DeliveryTarget, GameServer, ServerClient, ServerEnvelope, inventory::starting_inventory,
    movement::clean_player_name,
};

impl GameServer {
    pub fn connect(
        &mut self,
        protocol_version: u32,
        client_version: Option<String>,
        steam_id: SteamId,
        display_name: String,
        token: String,
    ) -> Result<(ClientId, Vec<ServerEnvelope>)> {
        if protocol_version != PROTOCOL_VERSION {
            bail!("protocol mismatch: client {protocol_version}, server {PROTOCOL_VERSION}");
        }

        match client_version.as_deref() {
            Some(GAME_VERSION) => {}
            Some(client_version) => {
                bail!("version mismatch: client {client_version}, server {GAME_VERSION}");
            }
            None => {
                bail!("version mismatch: client version is unknown, server {GAME_VERSION}");
            }
        }

        verify_auth_ticket(self.settings.auth_mode, steam_id, &token)?;

        if self.steam_to_client.contains_key(&steam_id) {
            bail!("this Steam user is already connected");
        }

        let client_id = self.next_client_id;
        self.next_client_id += 1;

        let is_admin = self.is_admin(steam_id);
        let name = clean_player_name(&display_name, client_id);
        let client = ServerClient {
            client_id,
            steam_id,
            name: name.clone(),
            controller: PlayerController::spawn(),
            inventory: starting_inventory(),
            is_admin,
            last_seen_tick: self.tick,
            next_gather_tick: self.tick,
        };

        self.clients.insert(client_id, client);
        self.steam_to_client.insert(steam_id, client_id);

        let snapshot = self.snapshot();
        Ok((
            client_id,
            vec![
                ServerEnvelope {
                    target: DeliveryTarget::Client(client_id),
                    message: ServerMessage::Welcome {
                        client_id,
                        map: self.save.map.clone(),
                        world: self.world.clone(),
                        is_admin,
                        snapshot,
                    },
                },
                ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::PlayerEvent(PlayerEvent::Joined { client_id, name }),
                },
            ],
        ))
    }

    pub fn disconnect(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.remove(&client_id) else {
            return Vec::new();
        };

        self.steam_to_client.remove(&client.steam_id);
        vec![ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::PlayerEvent(PlayerEvent::Left {
                client_id,
                name: client.name,
            }),
        }]
    }

    pub fn snapshot(&self) -> WorldSnapshot {
        let mut players = self
            .clients
            .values()
            .map(|client| PlayerState {
                client_id: client.client_id,
                steam_id: client.steam_id,
                name: client.name.clone(),
                position: client.controller.position,
                velocity: client.controller.velocity,
                yaw: client.controller.yaw,
                pitch: client.controller.pitch,
                health: client.controller.health,
                grounded: client.controller.grounded,
                last_processed_input: client.controller.last_processed_input,
                is_admin: client.is_admin,
                inventory: client.inventory.clone(),
            })
            .collect::<Vec<_>>();
        players.sort_by_key(|player| player.client_id);

        let mut dropped_items = self
            .dropped_items
            .values()
            .map(|body| body.item.clone())
            .collect::<Vec<_>>();
        dropped_items.sort_by_key(|item| item.id);
        let mut resource_nodes = self.resource_nodes.values().cloned().collect::<Vec<_>>();
        resource_nodes.sort_by_key(|node| node.id);

        WorldSnapshot {
            tick: self.tick,
            players,
            dropped_items,
            resource_nodes,
        }
    }

    fn is_admin(&self, steam_id: SteamId) -> bool {
        self.settings.singleplayer_host == Some(steam_id) || self.save.admins.contains(&steam_id)
    }
}
