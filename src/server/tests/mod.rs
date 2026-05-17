use super::*;
use super::{
    dropped_items::{DROPPED_ITEM_MERGE_RADIUS, DROPPED_ITEM_RADIUS},
    movement::SERVER_EYE_HEIGHT,
};
use crate::{
    items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID, COAL_ID, TEST_ORE_ID, TEST_RELIC_ID},
    protocol::{
        ChatMessage, ClientMessage, GAME_VERSION, InventoryCommand, ItemContainerSlot, ItemStack,
        PROTOCOL_VERSION, PlayerEvent, PlayerMovement, ResourceGatherCommand, ResourceNodeState,
        SERVER_TICK_RATE_HZ, Vec3Net,
    },
    resources::COAL_NODE_ID,
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

fn movement(sequence: u64, position: Vec3Net) -> PlayerMovement {
    PlayerMovement {
        sequence,
        position,
        velocity: Vec3Net::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        grounded: true,
    }
}

fn connect_host(server: &mut GameServer) -> ClientId {
    server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            1,
            "Host".to_owned(),
            offline_auth_token(1),
        )
        .expect("host should connect")
        .0
}

mod connection;
mod dropped_items;
mod inventory;
mod movement;
mod resource_nodes;
