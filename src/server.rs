use std::{
    collections::HashMap,
    f32::consts::{PI, TAU},
};

use anyhow::{Result, bail};

use crate::{
    controller::{MAX_LOOK_PITCH, PlayerController},
    protocol::{
        ChatMessage, ClientId, ClientMessage, PROTOCOL_VERSION, PlayerEvent, PlayerMovement,
        PlayerState, ServerMessage, SteamId, Vec3Net, WorldSnapshot, sanitize_chat,
    },
    save::WorldSave,
    steam::{AuthMode, verify_auth_ticket},
};

#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub auth_mode: AuthMode,
    pub singleplayer_host: Option<SteamId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryTarget {
    Client(ClientId),
    Broadcast,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerEnvelope {
    pub target: DeliveryTarget,
    pub message: ServerMessage,
}

#[derive(Debug)]
pub struct GameServer {
    save: WorldSave,
    settings: ServerSettings,
    clients: HashMap<ClientId, ServerClient>,
    steam_to_client: HashMap<SteamId, ClientId>,
    next_client_id: ClientId,
    tick: u64,
}

impl GameServer {
    pub fn new(mut save: WorldSave, settings: ServerSettings) -> Self {
        if let Some(host) = settings.singleplayer_host
            && !save.admins.contains(&host)
        {
            save.admins.push(host);
        }

        Self {
            tick: save.state.last_authoritative_tick,
            save,
            settings,
            clients: HashMap::new(),
            steam_to_client: HashMap::new(),
            next_client_id: 1,
        }
    }

    pub fn world_save(&self) -> WorldSave {
        let mut save = self.save.clone();
        save.state.last_authoritative_tick = self.tick;
        save
    }

    pub fn connect(
        &mut self,
        protocol_version: u32,
        steam_id: SteamId,
        display_name: String,
        token: String,
    ) -> Result<(ClientId, Vec<ServerEnvelope>)> {
        if protocol_version != PROTOCOL_VERSION {
            bail!("protocol mismatch: client {protocol_version}, server {PROTOCOL_VERSION}");
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
            is_admin,
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
                        world: self.save.map.world_data(),
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

    pub fn receive(&mut self, client_id: ClientId, message: ClientMessage) -> Vec<ServerEnvelope> {
        match message {
            ClientMessage::Auth { .. } => vec![ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::AuthRejected {
                    reason: "client is already authenticated".to_owned(),
                },
            }],
            ClientMessage::Movement(movement) => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    apply_client_movement(&mut client.controller, movement);
                }
                Vec::new()
            }
            ClientMessage::Chat { text } => sanitize_chat(&text)
                .and_then(|text| {
                    self.clients.get(&client_id).map(|client| ServerEnvelope {
                        target: DeliveryTarget::Broadcast,
                        message: ServerMessage::Chat(ChatMessage {
                            from: client.name.clone(),
                            text,
                        }),
                    })
                })
                .into_iter()
                .collect(),
            ClientMessage::Heartbeat => Vec::new(),
            ClientMessage::Disconnect => self.disconnect(client_id),
        }
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

    pub fn tick(&mut self, _delta_seconds: f32) -> Vec<ServerEnvelope> {
        self.tick += 1;
        self.save.state.last_authoritative_tick = self.tick;

        vec![ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::Snapshot(self.snapshot()),
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
            })
            .collect::<Vec<_>>();
        players.sort_by_key(|player| player.client_id);

        WorldSnapshot {
            tick: self.tick,
            players,
        }
    }

    fn is_admin(&self, steam_id: SteamId) -> bool {
        self.settings.singleplayer_host == Some(steam_id) || self.save.admins.contains(&steam_id)
    }
}

#[derive(Debug)]
struct ServerClient {
    client_id: ClientId,
    steam_id: SteamId,
    name: String,
    controller: PlayerController,
    is_admin: bool,
}

fn apply_client_movement(controller: &mut PlayerController, movement: PlayerMovement) {
    if movement.sequence <= controller.last_processed_input || !movement_is_finite(movement) {
        return;
    }

    controller.position = movement.position;
    controller.velocity = movement.velocity;
    controller.yaw = normalize_yaw(movement.yaw);
    controller.pitch = movement.pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
    controller.grounded = movement.grounded;
    controller.last_processed_input = movement.sequence;
}

fn movement_is_finite(movement: PlayerMovement) -> bool {
    vec3_is_finite(movement.position)
        && vec3_is_finite(movement.velocity)
        && movement.yaw.is_finite()
        && movement.pitch.is_finite()
}

fn vec3_is_finite(value: Vec3Net) -> bool {
    value.x.is_finite() && value.y.is_finite() && value.z.is_finite()
}

fn normalize_yaw(yaw: f32) -> f32 {
    (yaw + PI).rem_euclid(TAU) - PI
}

fn clean_player_name(name: &str, fallback_id: ClientId) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("Player {fallback_id}")
    } else {
        trimmed.chars().take(32).collect()
    }
}

#[cfg(test)]
mod tests;
