use super::*;
use super::{
    dropped_items::{DROPPED_ITEM_LIFETIME_TICKS, DROPPED_ITEM_MERGE_RADIUS, DROPPED_ITEM_RADIUS},
    movement::SERVER_EYE_HEIGHT,
};
use crate::{
    items::{BASIC_HATCHET_ID, COAL_ID},
    protocol::{
        ChatMessage, ClientMessage, GAME_VERSION, InventoryCommand, ItemContainerSlot, ItemStack,
        PROTOCOL_VERSION, PlayerEvent, PlayerMovement, ResourceGatherCommand, ResourceNodeState,
        SERVER_TICK_RATE_HZ, Vec3Net,
    },
    resources::COAL_NODE_ID,
    save::WorldSave,
};

// The canonical `server()` / `movement()` / `connect_host()` /
// `equip_basic_tools()` harness lives in `crate::server::test_support` so the
// colocated test modules share it too. Re-exported here so the submodules below
// keep reaching them through `use super::*`.
pub(super) use crate::server::test_support::{connect_host, equip_basic_tools, movement, server};

mod building;
mod claim;
mod combat;
mod commands;
mod connection;
mod door;
mod dropped_items;
mod furnace;
mod inventory;
mod loot_bag;
mod movement;
mod resource_nodes;
mod sleeping_bag;
mod storage_box;
mod tool_wear;
