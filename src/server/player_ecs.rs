//! ECS mirror for authoritative players.
//!
//! Companion to [`crate::server::resource_node_ecs`]. Player state lives
//! in `GameServer::clients: HashMap<ClientId, ServerClient>`; the
//! `sync_player_entities` system in `net/host.rs` reconciles that map
//! into ECS entities so chunk-room replication can attach `Replicate`
//! per entity.
//!
//! Following the project rule of one component per mutable field group
//! (Lightyear ships whole-component values, not field diffs), the
//! peer-visible state is split into [`PlayerProfile`] / [`PlayerPose`] /
//! [`PlayerHealth`] / [`PlayerChatBubble`], and the owner-only state
//! into [`PlayerInventory`] / [`PlayerCrafting`] /
//! [`PlayerOpenContainers`] / [`PlayerInputAck`]. The previous
//! mega-components re-shipped the full inventory at 20 Hz because the
//! input ack ticking every tick made the bundled value compare unequal.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    protocol::{
        AccountId, ClientId, OpenFurnaceView, OpenLootBagView, PlayerCraftingState,
        PlayerInventoryState, Vec3Net,
    },
    world::ChunkCoord,
};

/// Identity. Immutable after spawn. The wire-stable `client_id` is the
/// link back to the Lightyear `ClientOf` connection entity, and is what
/// every gameplay message refers to.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub client_id: ClientId,
    pub account_id: AccountId,
}

/// Peer-visible profile: display name + admin badge. Practically
/// immutable (set at connect, admin grants are rare), split from the
/// 20 Hz pose so the name string doesn't re-ship with every movement
/// diff.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerProfile {
    pub name: String,
    pub is_admin: bool,
}

/// Peer-visible movement state, the only player component that changes
/// every tick while moving. Kept lean so the per-tick wire diff stays
/// minimal.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerPose {
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
}

/// Peer-visible health. Changes on damage and heal only.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerHealth(pub f32);

/// Most recent chat bubble text, or `None` once the bubble window has
/// expired. Only the text is public, the expiry tick is server-only
/// bookkeeping. Split out so a live bubble's text doesn't ride along
/// in every movement diff for its whole 6 s lifetime.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerChatBubble(pub Option<String>);

/// Owner-only inventory state. Replication is gated to the owning
/// client's sender via `ComponentReplicationOverrides` (see
/// `net/host/rooms.rs`); peers never receive the wire bytes. Changes on
/// inventory mutation only.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerInventory(pub PlayerInventoryState);

/// Owner-only crafting queue. Changes while jobs are queued (progress
/// ticks each server tick during an active job).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerCrafting(pub PlayerCraftingState);

/// Owner-only views of whatever container UI the player has open.
/// `None`/`None` (the common case) is a few bytes; while a furnace is
/// open and burning, the progress fractions tick per server tick, which
/// is why this lives apart from the inventory.
#[derive(Component, Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerOpenContainers {
    /// Full per-client view of the currently-opened furnace, if any.
    /// Carrying the full [`OpenFurnaceView`] (slots + progress) rather
    /// than just the id keeps the furnace UI reachable from the
    /// replicated component alone.
    pub open_furnace: Option<OpenFurnaceView>,
    /// Full per-client view of the currently-opened loot bag, if any.
    /// Mirrors `open_furnace` for the bag UI.
    pub open_loot_bag: Option<OpenLootBagView>,
}

/// Owner-only input/action acknowledgement. Ticks at 20 Hz while the
/// player moves, which is exactly why it is its own tiny component: when
/// it was bundled with the inventory, every ack diff re-shipped the full
/// inventory bytes.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerInputAck {
    pub last_processed_input: u64,
    /// Highest optimistic-prediction action sequence the server has processed
    /// for this client (accepted *or* rejected). The client prunes pending
    /// overlay ops with `seq <= applied_action_seq`; see
    /// `src/app/state/prediction.rs`.
    pub applied_action_seq: u32,
}

/// Client-side assembled view of the owner-only player state. **Not a
/// wire shape**: the server replicates [`PlayerInventory`],
/// [`PlayerCrafting`], [`PlayerOpenContainers`], and [`PlayerInputAck`]
/// as separate components; `update_local_player_state_system`
/// reassembles them into this struct so UI consumers keep one handle.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerPrivate {
    pub inventory: PlayerInventoryState,
    pub crafting: PlayerCraftingState,
    pub open_furnace: Option<OpenFurnaceView>,
    pub open_loot_bag: Option<OpenLootBagView>,
    pub last_processed_input: u64,
    pub applied_action_seq: u32,
}

/// Authoritative damage reduction (0–100, percent). Replicated to every
/// peer in the same chunk room because the future HUD wants to read its
/// own armor straight off the replicated component instead of hand-rolling
/// a separate `ServerMessage`. Today every player ships with `0`, there
/// are no armor items defined, but the wire path is live so adding one
/// is purely a server-side change.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerArmor(pub u8);

/// Authoritative life state. Replicated to every peer in the room so
/// remote clients can render the dead avatar with the tilt-and-fade
/// "corpse" animation and the local owner can show the death splash.
///
/// `Alive` is the spawn default. `Dead` carries:
///   - `since_tick`: when death happened, for the corpse animation
///     timer.
///   - `killer`: the attacker's client id, so the death splash on the
///     victim's screen can name them.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum PlayerLifecycle {
    #[default]
    Alive,
    Dead {
        since_tick: u64,
        killer: Option<ClientId>,
    },
}

impl PlayerLifecycle {
    pub fn is_alive(self) -> bool {
        matches!(self, Self::Alive)
    }

    pub fn is_dead(self) -> bool {
        !self.is_alive()
    }
}

/// Whether this body is a logged-out "sleeping" body (the player
/// disconnected but their body stays in the world). Replicated so peers can
/// render the sleeping pose plus a look-at tooltip, and loot/attack it. The
/// local owner is never shown their own body as sleeping (they only exist as
/// a body while connected). `false` for live players.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerSleeping(pub bool);

/// Anchor chunk for room subscription. Updated when the player crosses
/// a chunk boundary (mirror reads `ChunkManager::player_chunk`).
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerChunk(pub ChunkCoord);

crate::server::entity_index::entity_index! {
    /// `ClientId → Entity` lookup so gather/chat/inventory paths can resolve
    /// a player in O(1).
    PlayerIndex, ClientId;
    despawn_player_entity
}

/// Wire-shape view used by the mirror to spawn or refresh a player
/// entity. Mirrors `ServerClient` without taking a copy of its internal
/// shape; one field per replicated component so the mirror sync can
/// compare-and-write each at its own cadence.
pub struct PlayerView {
    pub client_id: ClientId,
    pub account_id: AccountId,
    pub profile: PlayerProfile,
    pub pose: PlayerPose,
    pub health: PlayerHealth,
    pub chat_bubble: PlayerChatBubble,
    pub inventory: PlayerInventory,
    pub crafting: PlayerCrafting,
    pub containers: PlayerOpenContainers,
    pub input_ack: PlayerInputAck,
    pub armor: PlayerArmor,
    pub lifecycle: PlayerLifecycle,
    pub sleeping: PlayerSleeping,
}

pub fn spawn_player_entity(world: &mut World, view: PlayerView, chunk: ChunkCoord) -> Entity {
    let id = view.client_id;
    let entity = world
        .spawn((
            Player {
                client_id: view.client_id,
                account_id: view.account_id,
            },
            view.profile,
            view.pose,
            view.health,
            view.chat_bubble,
            view.inventory,
            view.crafting,
            view.containers,
            view.input_ack,
            view.armor,
            view.lifecycle,
            view.sleeping,
            PlayerChunk(chunk),
        ))
        .id();
    world.resource_mut::<PlayerIndex>().insert(id, entity);
    entity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_world() -> World {
        let mut world = World::new();
        world.init_resource::<PlayerIndex>();
        world
    }

    fn sample_view(client_id: ClientId) -> PlayerView {
        PlayerView {
            client_id,
            account_id: 42,
            profile: PlayerProfile {
                name: "Alice".to_owned(),
                is_admin: false,
            },
            pose: PlayerPose {
                position: Vec3Net::ZERO,
                velocity: Vec3Net::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                grounded: true,
            },
            health: PlayerHealth(100.0),
            chat_bubble: PlayerChatBubble::default(),
            inventory: PlayerInventory(PlayerInventoryState::empty()),
            crafting: PlayerCrafting(PlayerCraftingState::default()),
            containers: PlayerOpenContainers::default(),
            input_ack: PlayerInputAck::default(),
            armor: PlayerArmor::default(),
            lifecycle: PlayerLifecycle::default(),
            sleeping: PlayerSleeping::default(),
        }
    }

    #[test]
    fn spawn_and_despawn_round_trip_index() {
        let mut world = fresh_world();
        let entity = spawn_player_entity(&mut world, sample_view(1), ChunkCoord::new(0, 0));
        assert_eq!(world.resource::<PlayerIndex>().get(1), Some(entity));

        let profile = world.get::<PlayerProfile>(entity).expect("profile");
        assert_eq!(profile.name, "Alice");
        let ack = world.get::<PlayerInputAck>(entity).expect("input ack");
        assert_eq!(ack.last_processed_input, 0);
        let armor = world.get::<PlayerArmor>(entity).expect("armor");
        assert_eq!(armor.0, 0);

        let despawned = despawn_player_entity(&mut world, 1);
        assert_eq!(despawned, Some(entity));
        assert!(world.resource::<PlayerIndex>().get(1).is_none());
    }
}
