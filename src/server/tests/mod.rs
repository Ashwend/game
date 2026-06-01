use super::*;
use super::{
    dropped_items::{DROPPED_ITEM_LIFETIME_TICKS, DROPPED_ITEM_MERGE_RADIUS, DROPPED_ITEM_RADIUS},
    movement::SERVER_EYE_HEIGHT,
};
use crate::{
    items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID, COAL_ID},
    protocol::{
        ChatMessage, ClientMessage, GAME_VERSION, InventoryCommand, ItemContainerSlot, ItemStack,
        PROTOCOL_VERSION, PlayerEvent, PlayerMovement, ResourceGatherCommand, ResourceNodeState,
        SERVER_TICK_RATE_HZ, Vec3Net,
    },
    resources::COAL_NODE_ID,
    save::WorldSave,
};

fn server() -> GameServer {
    GameServer::new(
        WorldSave::new("Test", Some(1)),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
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
            String::new(),
        )
        .expect("host should connect")
        .0
}

/// Tests start from an empty inventory; helpers below seed the items each
/// scenario needs without taking a dependency on production starting state.
fn equip_basic_tools(server: &mut GameServer, client_id: ClientId) {
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected host should exist");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    client.inventory.actionbar_slots[1] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
}

mod combat;
mod commands;
mod connection;
mod dropped_items;
mod furnace;
mod inventory;
mod loot_bag;
mod movement;
mod resource_nodes;
