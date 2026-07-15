use anyhow::Result;

use crate::{
    auth::authenticate,
    controller::PlayerController,
    protocol::{
        AccountId, ClientId, GAME_VERSION, MAX_HEALTH, PROTOCOL_VERSION, PlayerEvent, PlayerState,
        ServerMessage, Vec3Net,
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

        // This account may already own a body in the world. A logged-out body
        // is asleep (no live transport), so we wake it in place, the player
        // resumes exactly where they left off, including any looting or a kill
        // that happened while they slept. A body that is still *online* means a
        // second live login from the same account, so we hard-drop that session
        // (snapshotting its state) and fall through to restore it fresh under a
        // new id, which sidesteps reusing a still-mapped client id.
        let mut envelopes = Vec::new();
        if let Some(existing_id) = self.account_to_client.get(&account_id).copied() {
            if self
                .clients
                .get(&existing_id)
                .is_some_and(|client| client.online)
            {
                envelopes = self.hard_disconnect(existing_id);
            } else {
                return Ok(self.wake_sleeper(existing_id, display_name, token_admin));
            }
        }

        let client_id = self.next_client_id;
        self.next_client_id.0 += 1;

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
                let mut controller = PlayerController::from_persisted(
                    player.position,
                    player.velocity,
                    player.yaw,
                    player.pitch,
                    player.health,
                    player.grounded,
                    player.last_processed_input,
                );
                // A player who disconnected while dead persists at 0 health
                // (lifecycle isn't saved), and would otherwise return as a
                // living-but-zero-health "zombie" the combat path refuses to
                // hit (`new_health <= 0` already counts as dead). Treat a
                // reconnect at non-positive health as a fresh respawn: full
                // health at a safe spawn. Their gear was already dropped into
                // a loot bag at the moment of death, so nothing extra is lost.
                if controller.health <= 0.0 {
                    controller.position = self.pick_safe_spawn(None);
                    controller.velocity = Vec3Net::ZERO;
                    controller.health = MAX_HEALTH;
                    controller.grounded = true;
                }
                (controller, inventory)
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
        // Recompute mitigation from whatever armor the restored inventory
        // carries. A fresh player has an empty paperdoll (all-zero protection);
        // a returning player who saved wearing a set comes back protected.
        let protection = crate::items::equipped_protection(&inventory.equipment_slots);
        let client = ServerClient {
            client_id,
            account_id,
            name: name.clone(),
            online: true,
            controller,
            inventory,
            protection,
            lifecycle: crate::server::PlayerLifecycle::Alive,
            is_admin,
            run_speed_multiplier: 1.0,
            last_seen_tick: self.tick,
            next_gather_tick: self.tick,
            next_attack_tick: self.tick,
            draw_started_tick: None,
            use_started_tick: None,
            heal_over_time: None,
            next_ranged_tick: self.tick,
            reload_slow_active: false,
            chat_bubble: None,
            view_tier: crate::protocol::ViewRadiusTier::default(),
            crafting: starting_crafting_state(),
            next_craft_job_id: crate::protocol::CraftingJobId(1),
            open_furnace: None,
            open_workbench: None,
            open_container: None,
            applied_action_seq: 0,
            ping_ms: 0,
            swing_seq: 0,
            swing_model: crate::items::ItemModel::Bag,
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
        // Late-joiner resend: if a meteor shower event is live (announce through
        // crater despawn), replay the announce to this client so they see the
        // fireball / crater immediately, exactly like the WorldTime seed in
        // Welcome. The client keys the whole sky show off this one payload.
        if let Some(announce) = self.meteor_shower_announce_for(client_id) {
            envelopes.push(announce);
        }
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::PlayerEvent(PlayerEvent::Joined { client_id, name }),
        });
        Ok((client_id, envelopes))
    }

    /// A player's live connection ended (clean quit, transport drop, kick, or
    /// stale-timeout). Their body does NOT leave the world: it stays as a
    /// logged-out "sleeping" body (Rust-style), frozen in place, still
    /// replicated, lootable, and killable, until the same account reconnects
    /// and wakes it. Idempotent: the disconnect path fires more than once per
    /// drop (clean `Disconnect` message, then the netcode `Disconnected` event).
    pub fn disconnect(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        if !self
            .clients
            .get(&client_id)
            .is_some_and(|client| client.online)
        {
            return Vec::new();
        }
        // Offline housekeeping: refund queued crafting (the player shouldn't pay
        // for jobs that never finished) and close any open containers so a peer
        // snapshot can't reach into a stale open-furnace/bag pointer.
        self.cancel_all_jobs_for_disconnect(client_id);
        self.close_furnace(client_id);
        self.close_container(client_id);

        let (name, persisted) = {
            let Some(client) = self.clients.get_mut(&client_id) else {
                return Vec::new();
            };
            client.online = false;
            // Freeze the activity clock; the stale-timeout only sweeps online
            // clients, so a sleeper is never re-evicted.
            client.last_seen_tick = self.tick;
            (client.name.clone(), persisted_player_from(client))
        };
        // Mirror the final pre-sleep state into the persisted store too, so a
        // crash or auto-save still captures it even though the live body also
        // stays in `clients`.
        self.remember_player(persisted);

        vec![
            ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::PlayerEvent(PlayerEvent::Left { client_id, name }),
            },
            // Tear down only the transport: the host layer inserts Lightyear's
            // `Disconnecting` and drops the entity↔client mapping. The body
            // stays in `clients`.
            ServerEnvelope {
                target: DeliveryTarget::Disconnect(client_id),
                message: ServerMessage::Heartbeat,
            },
        ]
    }

    /// Fully evict a player: remove their body and fold their state back into
    /// the persisted store. Used when a second live login from the same account
    /// takes over, the old session is replaced outright rather than left as a
    /// sleeper. Normal logouts go through [`GameServer::disconnect`] and become
    /// sleeping bodies instead.
    fn hard_disconnect(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        self.cancel_all_jobs_for_disconnect(client_id);
        self.close_furnace(client_id);
        self.close_container(client_id);
        // This body is leaving the world, so close anyone who had it open as a
        // sleeper.
        self.close_sleeper_views(client_id);

        let Some(client) = self.clients.remove(&client_id) else {
            return Vec::new();
        };
        let persisted = persisted_player_from(&client);
        self.account_to_client.remove(&client.account_id);
        self.chunk_manager.untrack_player(client_id);
        let name = client.name;
        self.remember_player(persisted);

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

    /// Reconnect path for an account whose body is asleep in the world: wake it
    /// in place. The body keeps its current position, inventory, and health, so
    /// the player resumes exactly where they slept (including anything looted
    /// off them). A body killed while sleeping comes back as a fresh respawn.
    fn wake_sleeper(
        &mut self,
        client_id: ClientId,
        display_name: String,
        token_admin: bool,
    ) -> (ClientId, Vec<ServerEnvelope>) {
        let account_id = self
            .clients
            .get(&client_id)
            .map(|client| client.account_id)
            .unwrap_or(AccountId(0));
        // The live body is authoritative now; drop any stale crash-safety copy.
        let _ = self.take_persisted_player(account_id);
        let admin_grant = token_admin || self.is_admin(account_id);
        let name = clean_player_name(&display_name, client_id);

        // A body killed while it slept comes back dead (or at 0 HP); waking it
        // is then a fresh respawn rather than a resume-in-place.
        let respawn = self
            .clients
            .get(&client_id)
            .map(|client| client.lifecycle.is_dead() || client.controller.health <= 0.0)
            .unwrap_or(false);
        let spawn = respawn.then(|| self.pick_safe_spawn(Some(client_id)));

        if let Some(client) = self.clients.get_mut(&client_id) {
            client.online = true;
            client.name = name.clone();
            client.is_admin = client.is_admin || admin_grant;
            client.last_seen_tick = self.tick;
            // A fresh wake shouldn't inherit a pre-sleep swing/gather cooldown.
            client.next_attack_tick = self.tick;
            client.next_gather_tick = self.tick;
            if let Some(spawn) = spawn {
                client.controller.position = spawn;
                client.controller.velocity = Vec3Net::ZERO;
                client.controller.health = MAX_HEALTH;
                client.controller.grounded = true;
                client.lifecycle = crate::server::PlayerLifecycle::Alive;
            }
        }
        if let Some(spawn) = spawn {
            self.chunk_manager.update_player_chunk(client_id, spawn);
        }
        // The body is awake (and authoritative again); close anyone who was
        // looting it as a sleeper so their stale view doesn't reach in.
        self.close_sleeper_views(client_id);

        let (is_admin, local_seed) = {
            let client = self.clients.get(&client_id).expect("woken body exists");
            (
                client.is_admin,
                PlayerState {
                    client_id,
                    position: client.controller.position,
                    velocity: client.controller.velocity,
                    yaw: client.controller.yaw,
                    pitch: client.controller.pitch,
                    health: client.controller.health,
                    grounded: client.controller.grounded,
                    last_processed_input: client.controller.last_processed_input,
                },
            )
        };
        let world_time = self.world_time_snapshot();

        let mut envelopes = vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Welcome {
                client_id,
                map: self.save.map.clone(),
                world: self.world.clone(),
                is_admin,
                local_seed,
                world_time,
            },
        }];
        // Same late-joiner resend as the fresh-connect path: a reconnecting
        // sleeper who missed the broadcast still sees a live meteor / crater.
        if let Some(announce) = self.meteor_shower_announce_for(client_id) {
            envelopes.push(announce);
        }
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::PlayerEvent(PlayerEvent::Joined { client_id, name }),
        });
        (client_id, envelopes)
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
                // Sleepers are deliberately silent; only sweep live sessions
                // that have gone quiet (their transport died without a clean
                // disconnect). The sweep just puts them to sleep too.
                client.online
                    && self.tick.saturating_sub(client.last_seen_tick) > CLIENT_STALE_TIMEOUT_TICKS
            })
            .map(|client| client.client_id)
            .collect::<Vec<_>>();

        stale_client_ids
            .into_iter()
            .flat_map(|client_id| self.disconnect(client_id))
            .collect()
    }
}
