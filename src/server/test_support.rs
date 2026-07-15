//! Shared `GameServer` test harness.
//!
//! One source of truth for the server bootstrap every server-side test needs:
//! a `NoAuth` singleplayer-host `GameServer`, a connected client pinned to the
//! origin (so position-dependent assertions stay deterministic despite the
//! random spawn), and the basic tool loadout. Both the `crate::server::tests`
//! tree and the colocated `#[cfg(test)] mod tests` modules (deployables,
//! loot_bag, furnace, commands, ...) import from here instead of re-authoring
//! their own `make_server`/`connect`/`equip` copies, which used to drift apart
//! whenever the `GameServer::new`/connect-handshake signature changed.

use crate::{
    auth::AuthMode,
    items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID},
    protocol::{ClientId, GAME_VERSION, ItemStack, PROTOCOL_VERSION, PlayerMovement, Vec3Net},
    save::WorldSave,
};

use super::{GameServer, ServerSettings};

/// A fresh authoritative server in the standard test configuration: no auth,
/// singleplayer host id 1, deterministic world seed.
///
/// Fresh worlds spawn ruin loot caches as world deployables (a pure function of
/// the seed). Those would break the many tests that assert on absolute
/// `deployed_entities` counts / emptiness after a placement, so this baseline
/// strips them, leaving a clean deployable map. Tests that specifically exercise
/// ruins use [`server_with_ruins`], which keeps them.
pub(crate) fn server() -> GameServer {
    let mut server = server_with_ruins();
    let cache_ids: Vec<crate::protocol::DeployedEntityId> = server
        .deployed_entities
        .iter()
        .filter(|(_, e)| matches!(e.kind, crate::items::DeployableKind::RuinCache))
        .map(|(id, _)| *id)
        .collect();
    for id in cache_ids {
        server.remove_deployed_entity_tracked(id);
    }
    // Clear the dirty/removed sync deltas so the first `drain_deployable_sync`
    // in a test starts from a truly clean slate.
    let _ = server.drain_deployable_sync();
    server
}

/// Like [`server`], but keeps the world's ruin caches. Used by the ruins tests.
pub(crate) fn server_with_ruins() -> GameServer {
    GameServer::new(
        WorldSave::new("Test", Some(crate::protocol::AccountId(1))),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: Some(crate::protocol::AccountId(1)),
        },
    )
}

/// A `PlayerMovement` at `position` with everything else zeroed and grounded.
pub(crate) fn movement(sequence: u64, position: Vec3Net) -> PlayerMovement {
    PlayerMovement {
        sequence,
        position,
        velocity: Vec3Net::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        grounded: true,
    }
}

/// Connect a client with the given display name and pin it to the origin.
/// Tests that care about a specific position set it themselves afterwards.
pub(crate) fn connect_named(server: &mut GameServer, name: &str) -> ClientId {
    let client_id = server
        .connect(
            PROTOCOL_VERSION,
            Some(GAME_VERSION.to_owned()),
            crate::protocol::AccountId(1),
            name.to_owned(),
            String::new(),
        )
        .expect("client should connect")
        .0;
    // Pin to origin so position-dependent tests (pickup distance, drop spots,
    // command ranges, placement reach) stay deterministic despite the random
    // initial spawn.
    server
        .clients
        .get_mut(&client_id)
        .expect("connected client should exist")
        .controller
        .position = Vec3Net::ZERO;
    client_id
}

/// Connect the singleplayer host client (named "Host"), pinned to the origin.
pub(crate) fn connect_host(server: &mut GameServer) -> ClientId {
    connect_named(server, "Host")
}

/// Seed the basic tool loadout: hatchet in actionbar slot 0, pickaxe in slot 1.
/// Tests start from an empty inventory, this gives them the tools without
/// depending on production starting state.
pub(crate) fn equip_basic_tools(server: &mut GameServer, client_id: ClientId) {
    let client = server
        .clients
        .get_mut(&client_id)
        .expect("connected client should exist");
    client.inventory.actionbar_slots[0] = Some(ItemStack::new(BASIC_HATCHET_ID, 1));
    client.inventory.actionbar_slots[1] = Some(ItemStack::new(BASIC_PICKAXE_ID, 1));
}

/// Insert a sticks-tier foundation directly into the authoritative map,
/// bypassing the placement command (which needs materials and snapping).
/// Goes through `insert_deployed_entity` so the mirror-sync and physics
/// collider bookkeeping run exactly as in production. Returns the id.
pub(crate) fn place_foundation(
    server: &mut GameServer,
    position: Vec3Net,
) -> crate::protocol::DeployedEntityId {
    place_building(
        server,
        crate::building::BuildingPiece::Foundation,
        position,
        0.0,
    )
}

/// Insert an arbitrary sticks-tier building piece directly into the
/// authoritative map at `position`/`yaw`, bypassing the placement command's
/// material and socket-snap validation. Used by tests that need a specific
/// collider (e.g. a vertical wall in a projectile's path). Returns the id.
pub(crate) fn place_building(
    server: &mut GameServer,
    piece: crate::building::BuildingPiece,
    position: Vec3Net,
    yaw: f32,
) -> crate::protocol::DeployedEntityId {
    let tier = crate::building::BuildingTier::Sticks;
    let max_health = crate::building::building_max_health(piece, tier);
    let id = server.next_deployed_entity_id;
    server.next_deployed_entity_id.0 += 1;
    let entity = super::deployables::DeployedEntity {
        id,
        item_id: crate::items::intern_item_id(crate::building::building_item_id(piece)),
        kind: crate::items::DeployableKind::Building { piece, tier },
        position,
        yaw,
        health: max_health,
        max_health,
        owner: Some(crate::protocol::AccountId(1)),
        furnace: None,
        placed_at_tick: 0,
        door: None,
        label: None,
        stability: 100,
        storage: None,
        torch: None,
        cupboard: None,
        ruin_cache: None,
        fuse: None,
    };
    server.insert_deployed_entity(id, entity);
    server.chunk_manager.track_deployed_entity(id, position);
    id
}
