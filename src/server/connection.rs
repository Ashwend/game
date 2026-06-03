use anyhow::Result;

use crate::{
    auth::authenticate,
    controller::PlayerController,
    protocol::{
        AccountId, ClientId, GAME_VERSION, PROTOCOL_VERSION, PlayerEvent, PlayerState,
        ServerMessage,
    },
};

use super::{
    CLIENT_STALE_TIMEOUT_TICKS, DeliveryTarget, GameServer, ServerClient, ServerEnvelope,
    crafting::starting_crafting_state, inventory::starting_inventory, movement::clean_player_name,
    persisted_player_from,
};

/// Returned by [`GameServer::connect`] when the client's version doesn't match
/// the server's (either the protocol number or the human-readable build). The
/// routing layer downcasts to it to answer with a structured
/// [`ServerMessage::VersionMismatch`] carrying the server's version, so the
/// client can show the player both versions and whether they're newer or
/// older, instead of a generic auth-rejection string. The `Display` form is
/// what lands in server logs.
#[derive(Debug, Clone)]
pub struct VersionMismatchRejection {
    pub server_version: String,
    pub server_protocol: u32,
    pub client_version: Option<String>,
    pub client_protocol: u32,
}

impl std::fmt::Display for VersionMismatchRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let client = self.client_version.as_deref().unwrap_or("unknown");
        write!(
            f,
            "version mismatch: client {client} (protocol {}), server {} (protocol {})",
            self.client_protocol, self.server_version, self.server_protocol
        )
    }
}

impl std::error::Error for VersionMismatchRejection {}

impl GameServer {
    pub fn connect(
        &mut self,
        protocol_version: u32,
        client_version: Option<String>,
        account_id: AccountId,
        display_name: String,
        token: String,
    ) -> Result<(ClientId, Vec<ServerEnvelope>)> {
        // Both the protocol number and the human-readable build must line up.
        // Either kind of difference is reported the same way, a structured
        // rejection the routing layer turns into `ServerMessage::VersionMismatch`
        // so the client can show the player both versions. The netcode already
        // let the connection through (its id is fixed, not version-tied), so
        // this is the real version gate.
        if protocol_version != PROTOCOL_VERSION || client_version.as_deref() != Some(GAME_VERSION) {
            return Err(VersionMismatchRejection {
                server_version: GAME_VERSION.to_owned(),
                server_protocol: PROTOCOL_VERSION,
                client_version,
                client_protocol: protocol_version,
            }
            .into());
        }

        // Validate the handshake and resolve the identity to admit under. For
        // WorkOS/Test this comes from the *verified* token, not the client's
        // claim; `account_id` is only consulted by the `NoAuth` path.
        let verified = authenticate(
            self.settings.auth_mode,
            self.workos.as_deref(),
            account_id,
            &token,
        )?;
        let account_id = verified.account_id;
        // A verified WorkOS token can flag the user as a pre-authorized admin
        // via the `urn:ashwend:admin` claim (driven by WorkOS user metadata).
        // Only ever true under WorkOS auth, so this grants admin on public
        // servers without persisting anything or touching the loopback path.
        let token_admin = verified.is_admin;
        // WorkOS may carry a display name in the token; otherwise trust the
        // (cleaned) name the client supplied.
        let display_name = verified.display_name.unwrap_or(display_name);

        // Takeover: if this identity is still attached to a prior session, tear
        // the old one down before admitting the new one instead of rejecting
        // the reconnect. This fires on a quick reconnect (the previous link
        // hasn't timed out yet) or a second login from the same install.
        // `disconnect` snapshots the old client's inventory/position into the
        // persisted-player store, which `take_persisted_player` below then
        // restores onto the new session, so a reconnect resumes exactly where
        // it left off rather than getting bounced for the ~10s it takes the
        // stale-client / netcode timeout to release the old slot.
        let mut envelopes = match self.account_to_client.get(&account_id).copied() {
            Some(old_client_id) => self.disconnect(old_client_id),
            None => Vec::new(),
        };

        let client_id = self.next_client_id;
        self.next_client_id += 1;

        // Returning players (saved on a prior shutdown or disconnect) keep
        // their inventory, position, and admin status. Their last display
        // name is overwritten with whatever the client provides this session.
        let persisted = self.take_persisted_player(account_id);
        let is_admin = token_admin
            || self.is_admin(account_id)
            || persisted.as_ref().is_some_and(|p| p.is_admin);
        let name = clean_player_name(&display_name, client_id);
        let (controller, inventory) = match persisted {
            Some(player) => {
                // A save written before an inventory-capacity change keeps its
                // old slot count; pad it up so the returning player gets the
                // current number of slots.
                let mut inventory = player.inventory;
                inventory.normalize_capacity();
                (
                    PlayerController::from_persisted(
                        player.position,
                        player.velocity,
                        player.yaw,
                        player.pitch,
                        player.health,
                        player.grounded,
                        player.last_processed_input,
                    ),
                    inventory,
                )
            }
            None => {
                // Fresh player: drop them at a random collision-free spot in
                // the playable area instead of always at the origin. Same picker
                // as respawn, so the two behave identically.
                let mut controller = PlayerController::spawn();
                controller.position = self.pick_safe_spawn(None);
                (controller, starting_inventory())
            }
        };
        let client = ServerClient {
            client_id,
            account_id,
            name: name.clone(),
            controller,
            inventory,
            // Phase 1 ships armor on the wire (per-component replicated),
            // but every player starts with 0, there are no armor items
            // defined yet. A future armor pass mutates this field
            // server-side and the replication path picks up the diff.
            armor: 0,
            lifecycle: crate::server::PlayerLifecycle::Alive,
            is_admin,
            last_seen_tick: self.tick,
            next_gather_tick: self.tick,
            next_attack_tick: self.tick,
            chat_bubble: None,
            view_tier: crate::protocol::ViewRadiusTier::default(),
            crafting: starting_crafting_state(),
            next_craft_job_id: 1,
            open_furnace: None,
            open_loot_bag: None,
            applied_action_seq: 0,
            ping_ms: 0,
        };

        let initial_position = client.controller.position;
        let local_seed = PlayerState {
            client_id: client.client_id,
            position: client.controller.position,
            velocity: client.controller.velocity,
            yaw: client.controller.yaw,
            pitch: client.controller.pitch,
            health: client.controller.health,
            grounded: client.controller.grounded,
            last_processed_input: client.controller.last_processed_input,
        };
        self.clients.insert(client_id, client);
        self.account_to_client.insert(account_id, client_id);
        // Register the player with the chunk anchor index so the AoI
        // path can find them. Done before any subsequent peer ticks
        // so peers already in the world see the new arrival on their
        // next tick.
        self.chunk_manager.track_player(client_id, initial_position);

        let world_time = self.world_time_snapshot();
        // Any takeover teardown envelopes (Left + transport Disconnect for the
        // old session) lead, so peers see the old session leave before the new
        // one joins and the host layer tears the stale transport down first.
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Welcome {
                client_id,
                map: self.save.map.clone(),
                world: self.world.clone(),
                is_admin,
                local_seed,
                world_time,
            },
        });
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::PlayerEvent(PlayerEvent::Joined { client_id, name }),
        });
        Ok((client_id, envelopes))
    }

    pub fn disconnect(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        // Refund any queued crafting inputs before we snapshot the
        // persisted state, the player shouldn't pay for jobs that never
        // finished. Overflow lands as drops at their last position.
        self.cancel_all_jobs_for_disconnect(client_id);
        // Drop any open-furnace reference so the next snapshot for any
        // peer doesn't reach into a stale client entry.
        self.close_furnace(client_id);
        // Same for loot bags, drops the player's open-bag pointer
        // and, if no one else has the bag open and it's empty,
        // despawns the bag entity.
        self.close_loot_bag(client_id);

        let Some(client) = self.clients.remove(&client_id) else {
            return Vec::new();
        };

        // Snapshot the client's live state before tearing them down so the
        // next shutdown save (or reconnect) sees their final position and
        // inventory, not the prior persisted copy.
        let persisted = persisted_player_from(&client);
        self.account_to_client.remove(&client.account_id);
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

    fn is_admin(&self, account_id: AccountId) -> bool {
        self.settings.singleplayer_host == Some(account_id)
            || self.save.admins.contains(&account_id)
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
