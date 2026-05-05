use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::{
    controller::PlayerController,
    protocol::{
        ChatMessage, ClientId, ClientMessage, PROTOCOL_VERSION, PlayerEvent, PlayerState,
        ServerMessage, SteamId, WorldSnapshot, sanitize_chat,
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

    pub fn world_seed(&self) -> u64 {
        self.save.seed
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
                        world_seed: self.save.seed,
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
            ClientMessage::Input(input) => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.controller.apply_input(input);
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

    pub fn tick(&mut self, delta_seconds: f32) -> Vec<ServerEnvelope> {
        self.tick += 1;
        self.save.state.last_authoritative_tick = self.tick;

        for client in self.clients.values_mut() {
            client.controller.simulate(delta_seconds);
        }

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
                stamina: client.controller.stamina,
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

fn clean_player_name(name: &str, fallback_id: ClientId) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        format!("Player {fallback_id}")
    } else {
        trimmed.chars().take(32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        controller::JUMP_STAMINA_COST,
        protocol::{ClientMessage, MAX_STAMINA, PROTOCOL_VERSION, PlayerInput, Vec3Net},
        save::WorldSave,
        steam::offline_auth_token,
    };

    fn server() -> GameServer {
        GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        )
    }

    #[test]
    fn singleplayer_host_is_admin() {
        let mut server = server();
        let (client_id, envelopes) = server
            .connect(
                PROTOCOL_VERSION,
                1,
                "Host".to_owned(),
                offline_auth_token(1),
            )
            .expect("host should connect");

        assert_eq!(client_id, 1);
        assert!(matches!(
            &envelopes[0].message,
            ServerMessage::Welcome { is_admin: true, .. }
        ));
    }

    #[test]
    fn rejects_invalid_auth() {
        let mut server = server();
        assert!(
            server
                .connect(PROTOCOL_VERSION, 2, "Bad".to_owned(), "wrong".to_owned())
                .is_err()
        );
    }

    #[test]
    fn movement_is_authoritative_on_tick() {
        let mut server = server();
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                1,
                "Host".to_owned(),
                offline_auth_token(1),
            )
            .expect("host should connect");

        server.receive(
            client_id,
            ClientMessage::Input(PlayerInput {
                sequence: 1,
                direction: Vec3Net::new(1.0, 0.0, 0.0),
                sprint: false,
                jump: false,
                yaw: 0.0,
                pitch: 0.0,
            }),
        );
        for _ in 0..10 {
            server.tick(0.05);
        }

        let snapshot = server.snapshot();
        assert!(snapshot.players[0].position.x > 1.0);
    }

    #[test]
    fn jump_consumes_stamina_and_is_networked() {
        let mut server = server();
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                1,
                "Host".to_owned(),
                offline_auth_token(1),
            )
            .expect("host should connect");

        server.receive(
            client_id,
            ClientMessage::Input(PlayerInput {
                sequence: 1,
                direction: Vec3Net::ZERO,
                sprint: false,
                jump: true,
                yaw: 0.0,
                pitch: 0.0,
            }),
        );
        server.tick(0.05);

        let player = &server.snapshot().players[0];
        assert!(player.position.y > 0.0);
        assert!(player.stamina < MAX_STAMINA);
        assert!(!player.grounded);
    }

    #[test]
    fn jump_request_survives_following_non_jump_input_before_tick() {
        let mut server = server();
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                1,
                "Host".to_owned(),
                offline_auth_token(1),
            )
            .expect("host should connect");

        server.receive(
            client_id,
            ClientMessage::Input(PlayerInput {
                sequence: 1,
                direction: Vec3Net::ZERO,
                sprint: false,
                jump: true,
                yaw: 0.0,
                pitch: 0.0,
            }),
        );
        server.receive(
            client_id,
            ClientMessage::Input(PlayerInput {
                sequence: 2,
                direction: Vec3Net::new(0.0, 0.0, 1.0),
                sprint: true,
                jump: false,
                yaw: 0.0,
                pitch: 0.0,
            }),
        );
        server.tick(0.05);

        let player = &server.snapshot().players[0];
        assert!(player.position.y > 0.0);
        assert!(!player.grounded);
    }

    #[test]
    fn sprint_does_not_spend_stamina_before_jump_request() {
        let mut server = server();
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                1,
                "Host".to_owned(),
                offline_auth_token(1),
            )
            .expect("host should connect");

        let client = server.clients.get_mut(&client_id).expect("client exists");
        client.controller.stamina = JUMP_STAMINA_COST + 0.1;

        server.receive(
            client_id,
            ClientMessage::Input(PlayerInput {
                sequence: 1,
                direction: Vec3Net::new(0.0, 0.0, 1.0),
                sprint: true,
                jump: true,
                yaw: 0.0,
                pitch: 0.0,
            }),
        );
        server.tick(0.05);

        let player = &server.snapshot().players[0];
        assert!(player.position.y > 0.0);
        assert!(player.stamina <= 0.1);
    }
}
