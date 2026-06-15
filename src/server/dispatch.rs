use crate::protocol::{ChatMessage, ClientId, ClientMessage, ServerMessage, sanitize_chat};

use super::{
    CHAT_BUBBLE_DURATION_TICKS, ChatBubble, DeliveryTarget, GameServer, ServerEnvelope,
    movement::accept_client_movement,
};

impl GameServer {
    /// Advance a client's optimistic-prediction high-water mark. Called for
    /// every predicted command (gather, inventory move/drop/pickup) *before*
    /// the handler runs, so the value advances whether the command is accepted
    /// or rejected, the client relies on this to prune and revert pending
    /// overlay ops. `max` guards against any out-of-order or duplicate seq.
    fn note_action_seq(&mut self, client_id: ClientId, seq: u32) {
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.applied_action_seq = client.applied_action_seq.max(seq);
        }
    }

    pub fn receive(&mut self, client_id: ClientId, message: ClientMessage) -> Vec<ServerEnvelope> {
        self.mark_client_seen(client_id);

        match message {
            ClientMessage::Auth { .. } => vec![ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::AuthRejected {
                    reason: "client is already authenticated".to_owned(),
                },
            }],
            ClientMessage::Movement(movement) => {
                let new_position = if let Some(client) = self.clients.get_mut(&client_id) {
                    // Dead players can't drive their controller, we
                    // keep the corpse pinned at the death position so
                    // the tilt-and-fade animation has a stable
                    // anchor and the loot pile stays under it.
                    if client.lifecycle.is_alive() {
                        accept_client_movement(&mut client.controller, movement);
                        Some(client.controller.position)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(position) = new_position {
                    // Keep the chunk anchor in sync so the next snapshot
                    // filters every networked entity through the player's
                    // new AoI ring.
                    self.chunk_manager.update_player_chunk(client_id, position);
                }
                Vec::new()
            }
            ClientMessage::Chat { text } => {
                let Some(text) = sanitize_chat(&text) else {
                    return Vec::new();
                };
                let expires_tick = self.tick.saturating_add(CHAT_BUBBLE_DURATION_TICKS);
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return Vec::new();
                };
                client.chat_bubble = Some(ChatBubble {
                    text: text.clone(),
                    expires_tick,
                });
                let from = client.name.clone();
                vec![ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::Chat(ChatMessage { from, text }),
                }]
            }
            ClientMessage::Command { text } => self.apply_command(client_id, text),
            ClientMessage::Inventory(command) => {
                // Advance the prediction high-water mark before dispatch so a
                // rejected command (out of range, full bag, …) still lets the
                // client prune and revert its optimistic overlay op.
                if let Some(seq) = command.action_seq() {
                    self.note_action_seq(client_id, seq);
                }
                self.apply_inventory_command(client_id, command)
            }
            ClientMessage::Crafting(command) => self.apply_crafting_command(client_id, command),
            ClientMessage::Gather(command) => {
                self.note_action_seq(client_id, command.seq);
                self.apply_gather_command(client_id, command)
            }
            ClientMessage::PlaceDeployable(command) => {
                self.apply_place_deployable_command(client_id, command)
            }
            ClientMessage::Furnace(command) => self.apply_furnace_command(client_id, command),
            ClientMessage::DamageDeployable(command) => {
                self.apply_damage_deployable_command(client_id, command)
            }
            ClientMessage::AttackPlayer(command) => {
                self.apply_attack_player_command(client_id, command)
            }
            ClientMessage::SwingStart(command) => {
                // Advance the prediction high-water mark like the other
                // predicted actions, then stamp the cosmetic peer-visible
                // swing. `command.seq` is the client's per-swing counter.
                self.note_action_seq(client_id, command.seq);
                self.apply_swing_start(client_id, command)
            }
            ClientMessage::Respawn => self.apply_respawn_command(client_id),
            ClientMessage::RespawnAtBag { id } => self.apply_respawn_at_bag_command(client_id, id),
            ClientMessage::PlaceBuilding(command) => {
                self.apply_place_building_command(client_id, command)
            }
            ClientMessage::Building(command) => self.apply_building_command(client_id, command),
            ClientMessage::Door(command) => self.apply_door_command(client_id, command),
            ClientMessage::SleepingBag(command) => {
                self.apply_sleeping_bag_command(client_id, command)
            }
            ClientMessage::LootBag(command) => self.apply_loot_bag_command(client_id, command),
            ClientMessage::LootSleeper {
                client_id: target_id,
            } => self.apply_loot_sleeper(client_id, target_id),
            ClientMessage::OpenStorageBox { id } => self.apply_open_storage_box(client_id, id),
            ClientMessage::RequestWorldMap => self.apply_world_map_request(client_id),
            ClientMessage::WorldMapMarker(command) => {
                self.apply_world_map_marker_command(client_id, command)
            }
            ClientMessage::SetViewRadius { tier } => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.view_tier = tier;
                }
                Vec::new()
            }
            ClientMessage::Voice(voice) => self.apply_voice_frame(client_id, voice),
            ClientMessage::Heartbeat => Vec::new(),
            ClientMessage::Ping {
                client_time_ms,
                rtt_ms,
            } => {
                // Store the client's self-measured latency for the roster, then
                // echo the timestamp straight back so the client can take a
                // fresh round-trip sample.
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.ping_ms = rtt_ms;
                }
                vec![ServerEnvelope {
                    target: DeliveryTarget::Client(client_id),
                    message: ServerMessage::Pong { client_time_ms },
                }]
            }
            ClientMessage::Disconnect => self.disconnect(client_id),
        }
    }
}
