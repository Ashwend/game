use std::collections::HashMap;

use bevy::prelude::*;
use lightyear::{
    connection::client::Disconnecting,
    prelude::{Disconnected, MessageReceiver, MessageSender, server::ClientOf},
};

use super::super::channels::send_server_message;
use super::AuthoritativeServer;
use crate::{
    protocol::{ClientId, ClientMessage, ServerMessage},
    server::{DeliveryTarget, GameServer, ServerEnvelope},
};

#[derive(Resource, Default)]
pub(super) struct ServerConnections {
    by_entity: HashMap<Entity, ClientId>,
    client_to_entity: HashMap<ClientId, Entity>,
}

impl ServerConnections {
    /// Look up the Lightyear `ClientOf` (= sender) entity for a given
    /// game client id. Returns `None` if the client is not currently
    /// connected. Used by the chunk-room subscription updater.
    pub(super) fn entity_for_client(&self, client_id: ClientId) -> Option<Entity> {
        self.client_to_entity.get(&client_id).copied()
    }
}

pub(super) fn receive_client_messages(
    mut commands: Commands,
    mut server: ResMut<AuthoritativeServer>,
    mut connections: ResMut<ServerConnections>,
    mut receivers: Query<(Entity, &mut MessageReceiver<ClientMessage>), With<ClientOf>>,
    mut senders: Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    for (entity, mut receiver) in &mut receivers {
        let messages: Vec<ClientMessage> = receiver.receive().collect();
        for message in messages {
            handle_client_message(
                entity,
                message,
                &mut commands,
                &mut server.0,
                &mut connections,
                &mut senders,
            );
        }
    }
}

pub(super) fn handle_disconnected_clients(
    mut commands: Commands,
    mut server: ResMut<AuthoritativeServer>,
    mut connections: ResMut<ServerConnections>,
    disconnected: Query<Entity, (With<ClientOf>, Added<Disconnected>)>,
    mut senders: Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    for entity in &disconnected {
        let Some(client_id) = forget_connection(entity, &mut connections) else {
            continue;
        };
        let envelopes = server.0.disconnect(client_id);
        route_envelopes(&mut commands, &mut connections, &mut senders, envelopes);
    }
}

pub(super) fn route_envelopes(
    commands: &mut Commands,
    connections: &mut ServerConnections,
    senders: &mut Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
    envelopes: Vec<ServerEnvelope>,
) {
    for envelope in envelopes {
        match envelope.target {
            DeliveryTarget::Client(client_id) => {
                if let Some(entity) = connections.client_to_entity.get(&client_id).copied() {
                    send_to_entity(senders, entity, envelope.message);
                }
            }
            DeliveryTarget::Broadcast => {
                let entities = connections.by_entity.keys().copied().collect::<Vec<_>>();
                for entity in entities {
                    send_to_entity(senders, entity, envelope.message.clone());
                }
            }
            DeliveryTarget::BroadcastExcept(excluded_client_id) => {
                let excluded_entity = connections
                    .client_to_entity
                    .get(&excluded_client_id)
                    .copied();
                let entities = connections
                    .by_entity
                    .keys()
                    .copied()
                    .filter(|entity| Some(*entity) != excluded_entity)
                    .collect::<Vec<_>>();
                for entity in entities {
                    send_to_entity(senders, entity, envelope.message.clone());
                }
            }
            DeliveryTarget::Disconnect(client_id) => {
                if let Some(entity) = connections.client_to_entity.get(&client_id).copied() {
                    // `Disconnecting` is consumed by
                    // `lightyear_connection::server::ConnectionPlugin::disconnect`
                    // in `Last`, which marks the entity `Disconnected` and
                    // despawns it on the next frame. That fires our
                    // `handle_disconnected_clients` system on `Added<Disconnected>`,
                    // but `forget_connection` here makes that call a no-op
                    // (its early-return path) so the server doesn't try to
                    // double-disconnect a client we already cleaned up.
                    commands.entity(entity).insert(Disconnecting);
                    forget_connection(entity, connections);
                }
            }
        }
    }
}

fn handle_client_message(
    entity: Entity,
    message: ClientMessage,
    commands: &mut Commands,
    server: &mut GameServer,
    connections: &mut ServerConnections,
    senders: &mut Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    let Some(client_id) = connections.by_entity.get(&entity).copied() else {
        handle_unauthenticated_message(entity, message, commands, server, connections, senders);
        return;
    };

    if matches!(message, ClientMessage::Disconnect) {
        // server.disconnect emits a trailing `DeliveryTarget::Disconnect`
        // envelope; route_envelopes will tear down the connection. No need
        // to call forget_connection here.
        let envelopes = server.disconnect(client_id);
        route_envelopes(commands, connections, senders, envelopes);
        return;
    }

    let envelopes = server.receive(client_id, message);
    route_envelopes(commands, connections, senders, envelopes);
}

fn handle_unauthenticated_message(
    entity: Entity,
    message: ClientMessage,
    commands: &mut Commands,
    server: &mut GameServer,
    connections: &mut ServerConnections,
    senders: &mut Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    let ClientMessage::Auth {
        protocol_version,
        client_version,
        steam_id,
        display_name,
        token,
    } = message
    else {
        send_to_entity(
            senders,
            entity,
            ServerMessage::AuthRejected {
                reason: "client is not authenticated".to_owned(),
            },
        );
        return;
    };

    match server.connect(
        protocol_version,
        client_version,
        steam_id,
        display_name,
        token,
    ) {
        Ok((client_id, envelopes)) => {
            connections.by_entity.insert(entity, client_id);
            connections.client_to_entity.insert(client_id, entity);
            route_envelopes(commands, connections, senders, envelopes);
        }
        Err(error) => {
            send_to_entity(
                senders,
                entity,
                ServerMessage::AuthRejected {
                    reason: error.to_string(),
                },
            );
        }
    }
}

fn forget_connection(entity: Entity, connections: &mut ServerConnections) -> Option<ClientId> {
    let client_id = connections.by_entity.remove(&entity)?;
    connections.client_to_entity.remove(&client_id);
    Some(client_id)
}

fn send_to_entity(
    senders: &mut Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
    entity: Entity,
    message: ServerMessage,
) {
    if let Ok(mut sender) = senders.get_mut(entity) {
        send_server_message(&mut sender, message);
    }
}
