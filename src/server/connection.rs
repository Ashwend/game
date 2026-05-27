use anyhow::{Result, bail};
use bevy::log::info_span;

use crate::{
    controller::PlayerController,
    protocol::{
        ClientId, GAME_VERSION, PROTOCOL_VERSION, PlayerEvent, PlayerState, ServerMessage, SteamId,
        WorldSnapshot,
    },
    steam::verify_auth_ticket,
};

use super::{
    CLIENT_STALE_TIMEOUT_TICKS, DeliveryTarget, GameServer, ServerClient, ServerEnvelope,
    crafting::starting_crafting_state, inventory::starting_inventory, movement::clean_player_name,
    persisted_player_from,
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

        // Returning players (saved on a prior shutdown or disconnect) keep
        // their inventory, position, and admin status. Their last display
        // name is overwritten with whatever the client provides this session.
        let persisted = self.take_persisted_player(steam_id);
        let is_admin = self.is_admin(steam_id) || persisted.as_ref().is_some_and(|p| p.is_admin);
        let name = clean_player_name(&display_name, client_id);
        let (controller, inventory) = match persisted {
            Some(player) => (
                PlayerController::from_persisted(
                    player.position,
                    player.velocity,
                    player.yaw,
                    player.pitch,
                    player.health,
                    player.grounded,
                    player.last_processed_input,
                ),
                player.inventory,
            ),
            None => (PlayerController::spawn(), starting_inventory()),
        };
        let client = ServerClient {
            client_id,
            steam_id,
            name: name.clone(),
            controller,
            inventory,
            is_admin,
            last_seen_tick: self.tick,
            next_gather_tick: self.tick,
            chat_bubble: None,
            view_tier: crate::protocol::ViewRadiusTier::default(),
            crafting: starting_crafting_state(),
            next_craft_job_id: 1,
            open_furnace: None,
        };

        let initial_position = client.controller.position;
        self.clients.insert(client_id, client);
        self.steam_to_client.insert(steam_id, client_id);
        // Register the player with the chunk anchor index so the AoI
        // path can find them. Done before building the welcome snapshot
        // so peers already in the world see the new arrival on their
        // next tick.
        self.chunk_manager.track_player(client_id, initial_position);

        let snapshot = self.snapshot_for(client_id);
        let world_time = self.world_time_snapshot();
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
                        world_time,
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
        // Refund any queued crafting inputs before we snapshot the
        // persisted state — the player shouldn't pay for jobs that never
        // finished. Overflow lands as drops at their last position.
        self.cancel_all_jobs_for_disconnect(client_id);
        // Drop any open-furnace reference so the next snapshot for any
        // peer doesn't reach into a stale client entry.
        self.close_furnace(client_id);

        let Some(client) = self.clients.remove(&client_id) else {
            return Vec::new();
        };

        // Snapshot the client's live state before tearing them down so the
        // next shutdown save (or reconnect) sees their final position and
        // inventory, not the prior persisted copy.
        let persisted = persisted_player_from(&client);
        self.steam_to_client.remove(&client.steam_id);
        self.chunk_manager.untrack_player(client_id);
        let name = client.name;
        self.remember_player(persisted);

        // The trailing Disconnect envelope signals the host transport layer
        // to insert Lightyear's `Disconnecting` and drop the entity↔client
        // mapping. The carried message is ignored at routing time but kept
        // legible for logs in case an envelope dump is dredged out of a bug
        // report.
        vec![
            ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::PlayerEvent(PlayerEvent::Left { client_id, name }),
            },
            ServerEnvelope {
                target: DeliveryTarget::Disconnect(client_id),
                message: ServerMessage::Heartbeat,
            },
        ]
    }

    /// Snapshot intended for `for_client` — only that client's player gets
    /// their full inventory in the payload, peer entries leave it `None`.
    pub fn snapshot_for(&self, for_client: ClientId) -> WorldSnapshot {
        self.snapshot_inner(Some(for_client))
    }

    /// Snapshot with no inventories attached. Used for places that don't
    /// belong to any specific client (tests, the initial broadcast slot
    /// while a client is still in handshake, etc.).
    pub fn snapshot(&self) -> WorldSnapshot {
        self.snapshot_inner(None)
    }

    fn snapshot_inner(&self, for_client: Option<ClientId>) -> WorldSnapshot {
        let _span = info_span!(
            "snapshot_inner",
            client = for_client.map(|cid| cid as i64).unwrap_or(-1)
        )
        .entered();
        // Single AoI gate: ask the chunk manager which chunks this
        // client should see, then build every networked-entity vector
        // from chunk membership. `for_client = None` (tests, the brief
        // handshake window) means no client to filter against — the
        // legacy unfiltered behaviour, used so test fixtures aren't
        // accidentally starved of state.
        let chunk_filter = for_client
            .and_then(|cid| self.clients.get(&cid))
            .map(|client| {
                self.chunk_manager
                    .visible_chunks(client.controller.position, client.view_tier)
            });

        let players_span = info_span!("snapshot_players");
        let mut players = players_span.in_scope(|| {
            self.clients
                .values()
                .filter(|client| match (&chunk_filter, for_client) {
                    // No filter (tests / handshake snapshot): include everyone.
                    (None, _) => true,
                    // Always include the local player, regardless of which
                    // chunk their controller currently anchors to.
                    (Some(_), Some(local)) if client.client_id == local => true,
                    // Peers visible only if their anchor chunk falls inside
                    // the local player's AoI ring. Falls back to the raw
                    // controller position if the manager hasn't recorded
                    // the player yet (transient state on connect).
                    (Some(visible), _) => self
                        .chunk_manager
                        .player_chunk(client.client_id)
                        .map(|coord| visible.contains(&coord))
                        .unwrap_or_else(|| {
                            let coord = crate::world::ChunkCoord::from_world(
                                client.controller.position.x,
                                client.controller.position.z,
                            );
                            visible.contains(&coord)
                        }),
                })
                .map(|client| {
                    let is_local = Some(client.client_id) == for_client;
                    let inventory = if is_local {
                        Some(client.inventory.clone())
                    } else {
                        None
                    };
                    // Same reasoning as inventory: only the owning client
                    // needs to render their own crafting queue, and peers
                    // shouldn't see what someone else is building.
                    let crafting = if is_local {
                        Some(client.crafting.clone())
                    } else {
                        None
                    };
                    // Furnace view is owner-only too — peers can see the
                    // structure's public `active` flag via the deployable
                    // snapshot, but the inventory itself stays private.
                    let open_furnace = if is_local {
                        self.open_furnace_view_for(client.client_id)
                    } else {
                        None
                    };
                    PlayerState {
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
                        chat_bubble: client
                            .chat_bubble
                            .as_ref()
                            .map(|bubble| bubble.text.clone()),
                        inventory,
                        crafting,
                        open_furnace,
                    }
                })
                .collect::<Vec<_>>()
        });
        players.sort_by_key(|player| player.client_id);

        let dropped_items = info_span!("snapshot_dropped_items").in_scope(|| {
            let visible_drops = chunk_filter.as_ref().map(|chunks| {
                chunks
                    .iter()
                    .flat_map(|coord| self.chunk_manager.dropped_items_in(*coord))
                    .collect::<std::collections::HashSet<_>>()
            });
            let mut dropped_items = self
                .dropped_items
                .values()
                .filter(|body| match &visible_drops {
                    None => true,
                    Some(visible) => visible.contains(&body.item.id),
                })
                .map(|body| body.item.clone())
                .collect::<Vec<_>>();
            dropped_items.sort_by_key(|item| item.id);
            dropped_items
        });

        let resource_nodes = info_span!("snapshot_resource_nodes").in_scope(|| {
            let visible_nodes = chunk_filter.as_ref().map(|chunks| {
                chunks
                    .iter()
                    .flat_map(|coord| self.chunk_manager.nodes_in(*coord))
                    .collect::<std::collections::HashSet<_>>()
            });
            let mut resource_nodes = self
                .resource_nodes
                .values()
                .filter(|node| match &visible_nodes {
                    None => true,
                    Some(visible) => visible.contains(&node.id),
                })
                .cloned()
                .collect::<Vec<_>>();
            resource_nodes.sort_by_key(|node| node.id);
            resource_nodes
        });

        let deployed_entities = info_span!("snapshot_deployables").in_scope(|| {
            let visible_deployables = chunk_filter.as_ref().map(|chunks| {
                chunks
                    .iter()
                    .flat_map(|coord| self.chunk_manager.deployed_entities_in(*coord))
                    .collect::<std::collections::HashSet<_>>()
            });
            self.deployed_entities_for_snapshot(visible_deployables.as_ref())
        });

        WorldSnapshot {
            tick: self.tick,
            players,
            dropped_items,
            resource_nodes,
            deployed_entities,
        }
    }

    fn is_admin(&self, steam_id: SteamId) -> bool {
        self.settings.singleplayer_host == Some(steam_id) || self.save.admins.contains(&steam_id)
    }

    pub fn kick_all(&mut self, reason: impl Into<String>) -> Vec<ServerEnvelope> {
        let reason = reason.into();
        let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
        let mut envelopes = client_ids
            .iter()
            .copied()
            .map(|client_id| ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Kicked {
                    reason: reason.clone(),
                },
            })
            .collect::<Vec<_>>();

        for client_id in client_ids {
            envelopes.extend(self.disconnect(client_id));
        }

        envelopes
    }

    pub(super) fn mark_client_seen(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.last_seen_tick = self.tick;
        }
    }

    pub(super) fn disconnect_stale_clients(&mut self) -> Vec<ServerEnvelope> {
        let stale_client_ids = self
            .clients
            .values()
            .filter(|client| {
                self.tick.saturating_sub(client.last_seen_tick) > CLIENT_STALE_TIMEOUT_TICKS
            })
            .map(|client| client.client_id)
            .collect::<Vec<_>>();

        stale_client_ids
            .into_iter()
            .flat_map(|client_id| self.disconnect(client_id))
            .collect()
    }
}
