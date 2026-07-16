//! Golden-layout guard for the wire protocol, the sibling of the save
//! format's `world_save_postcard_layout_is_stable`. [`ClientMessage`] and
//! [`ServerMessage`] travel as postcard, a non-self-describing positional
//! format, so reordering or retyping a field, or inserting an enum variant
//! mid-enum, silently changes the wire byte layout WITHOUT
//! [`PROTOCOL_VERSION`] being bumped; a client/server build skew then
//! mis-decodes with no test going red. The fixtures below pin one instance
//! per top-level variant and hash the whole encoding.
//!
//! Fixture rules:
//! - Exactly one fixture per top-level variant, in declaration order.
//! - Every value is a fixed literal (no randomness, no registry lookups, no
//!   `Default` that might drift), so the hash only moves when the *layout*
//!   moves, never when game data does.
//! - For nested command/payload enums the fixture prefers the LAST declared
//!   variant: postcard encodes enum discriminants as varint indices, so a
//!   mid-enum insertion shifts every later index, and pinning the last
//!   variant catches an insertion anywhere before it.

use sha2::{Digest, Sha256};

use super::*;
use crate::building::BuildingPiece;
use crate::items::{ExplosiveKind, ItemModel, intern_item_id};
use crate::world::{
    BlockKind, MapType, ProceduralMapSize, WorldBlock, WorldData, WorldResourceNodeSpawn,
};
use crate::world_time::WorldTimeSnapshot;

/// One fixture per [`ClientMessage`] variant, in declaration order.
fn client_fixtures() -> Vec<ClientMessage> {
    vec![
        ClientMessage::Auth {
            // Fixed fixture value, deliberately not PROTOCOL_VERSION: the
            // live version is hashed once at the payload head instead.
            protocol_version: 46,
            client_version: Some("0.23.0".to_owned()),
            account_id: crate::protocol::AccountId(11),
            display_name: "Golden Player".to_owned(),
            token: "golden-token".to_owned(),
        },
        ClientMessage::Movement(PlayerMovement {
            sequence: 42,
            position: Vec3Net::new(1.0, 2.0, 3.0),
            velocity: Vec3Net::new(0.5, -0.5, 0.25),
            yaw: 0.25,
            pitch: -0.1,
            grounded: true,
        }),
        ClientMessage::Chat {
            text: "hello".to_owned(),
        },
        ClientMessage::Command {
            text: "time 0700".to_owned(),
        },
        ClientMessage::Inventory(InventoryCommand::Sort),
        ClientMessage::Crafting(CraftingCommand::Cancel {
            job_id: crate::protocol::CraftingJobId(7),
        }),
        ClientMessage::Gather(ResourceGatherCommand {
            resource_node_id: crate::protocol::ResourceNodeId(1000),
            seq: 5,
            hit_point: Vec3Net::new(4.0, 5.0, 6.0),
        }),
        ClientMessage::PlaceDeployable(PlaceDeployableCommand {
            item_id: intern_item_id("furnace"),
            position: Vec3Net::new(7.0, 0.0, 8.0),
            yaw: 1.5,
            wall_mounted: true,
        }),
        ClientMessage::Furnace(FurnaceCommand::QuickTransfer {
            from: FurnaceSlotRef::Item(3),
        }),
        ClientMessage::Workbench(WorkbenchCommand::Upgrade {
            id: crate::protocol::DeployedEntityId(21),
        }),
        ClientMessage::Ranged(RangedCommand::Fire {
            aim_dir: Vec3Net::new(0.0, 0.0, 1.0),
        }),
        ClientMessage::Explosive(ExplosiveCommand::Defuse {
            id: crate::protocol::DeployedEntityId(22),
        }),
        ClientMessage::DamageDeployable(DamageDeployableCommand {
            id: crate::protocol::DeployedEntityId(23),
        }),
        ClientMessage::AttackPlayer(AttackPlayerCommand {
            target_player_id: crate::protocol::ClientId(2),
        }),
        ClientMessage::SwingStart(SwingStartCommand {
            seq: 9,
            model: ItemModel::Bandage,
        }),
        ClientMessage::Respawn,
        ClientMessage::RespawnAtBag {
            id: crate::protocol::DeployedEntityId(24),
        },
        ClientMessage::PlaceBuilding(PlaceBuildingCommand {
            piece: BuildingPiece::Stairs,
            position: Vec3Net::new(10.0, 0.0, 11.0),
            yaw: 3.0,
        }),
        ClientMessage::Building(BuildingCommand::Demolish {
            id: crate::protocol::DeployedEntityId(25),
        }),
        ClientMessage::Door(DoorCommand::PickUp {
            id: crate::protocol::DeployedEntityId(26),
        }),
        ClientMessage::SleepingBag(SleepingBagCommand::PickUp {
            id: crate::protocol::DeployedEntityId(27),
        }),
        ClientMessage::Claim(ClaimCommand::ClearList {
            id: crate::protocol::DeployedEntityId(28),
        }),
        ClientMessage::LootBag(LootBagCommand::QuickTransfer {
            from: LootBagSlotRef::Bag(4),
        }),
        ClientMessage::LootSleeper {
            client_id: crate::protocol::ClientId(3),
        },
        ClientMessage::SetViewRadius {
            tier: ViewRadiusTier::High,
        },
        ClientMessage::Voice(VoiceFrame {
            sequence: 12,
            frame: vec![1, 2, 3],
        }),
        ClientMessage::Heartbeat,
        ClientMessage::Ping {
            client_time_ms: 5000,
            rtt_ms: 40,
        },
        ClientMessage::Disconnect,
        ClientMessage::OpenStorageBox {
            id: crate::protocol::DeployedEntityId(29),
        },
        ClientMessage::RequestWorldMap,
        ClientMessage::WorldMapMarker(WorldMapMarkerCommand::Remove { id: 13 }),
        ClientMessage::Consumable(ConsumableCommand::UseCancel),
    ]
}

/// One fixture per [`ServerMessage`] variant, in declaration order.
fn server_fixtures() -> Vec<ServerMessage> {
    let player_state = PlayerState {
        client_id: crate::protocol::ClientId(1),
        position: Vec3Net::new(1.0, 2.0, 3.0),
        velocity: Vec3Net::ZERO,
        yaw: 0.25,
        pitch: -0.1,
        health: 87.0,
        grounded: true,
        last_processed_input: 99,
    };
    let world_time = WorldTimeSnapshot {
        seconds_of_day: 300.0,
        multiplier: 1.0,
        server_tick: 77,
    };
    // Hand-built world payload instead of `WorldData::test_world()`: the
    // generated block list is a function of worldgen, and a worldgen tweak is
    // not a wire-layout change, so it must not trip this guard. One block and
    // one spawn keep the nested `WorldBlock`/`WorldResourceNodeSpawn` layouts
    // under the hash.
    let world = WorldData {
        floor_size: 64.0,
        blocks: vec![WorldBlock {
            center: Vec3Net::new(1.0, 2.0, 3.0),
            half_extents: Vec3Net::new(0.5, 0.5, 0.5),
            kind: BlockKind::RuinMasonry,
        }],
        resource_nodes: vec![WorldResourceNodeSpawn {
            id: crate::protocol::ResourceNodeId(1),
            definition_id: "pine_tree".to_owned(),
            position: Vec3Net::new(4.0, 0.0, 5.0),
            yaw: 0.5,
        }],
    };
    vec![
        ServerMessage::Welcome {
            client_id: crate::protocol::ClientId(1),
            map: MapType::Procedural {
                seed: 42,
                size: ProceduralMapSize::Small,
            },
            world,
            is_admin: false,
            local_seed: player_state.clone(),
            world_time,
        },
        ServerMessage::AuthRejected {
            reason: "bad token".to_owned(),
        },
        ServerMessage::VersionMismatch {
            server_version: "9.9.9".to_owned(),
            server_protocol: 46,
        },
        ServerMessage::Kicked {
            reason: "restart".to_owned(),
        },
        ServerMessage::PlayerEvent(PlayerEvent::Left {
            client_id: crate::protocol::ClientId(2),
            name: "Golden Peer".to_owned(),
        }),
        ServerMessage::Correction(player_state),
        ServerMessage::Chat(ChatMessage {
            from: "Golden Player".to_owned(),
            text: "hello".to_owned(),
        }),
        ServerMessage::ItemMerged {
            item_id: intern_item_id("wood"),
            quantity: 9,
        },
        ServerMessage::Toast(ToastMessage {
            kind: ToastKind::Error,
            text: "toast".to_owned(),
        }),
        ServerMessage::ResourceImpact {
            position: Vec3Net::new(6.0, 7.0, 8.0),
            kind: ResourceImpactKind::HayGrass,
        },
        ServerMessage::PlayerImpact {
            attacker: crate::protocol::ClientId(1),
            target: crate::protocol::ClientId(2),
            position: Vec3Net::new(9.0, 1.0, 2.0),
            attacker_position: Vec3Net::new(3.0, 4.0, 5.0),
            model: ItemModel::Bandage,
            damage_dealt: 12,
        },
        ServerMessage::ProjectileImpact {
            position: Vec3Net::new(6.0, 5.0, 4.0),
            model: ItemModel::Crossbow,
            surface: ProjectileSurface::World,
            owner_confirmation: true,
        },
        ServerMessage::Knockback {
            impulse: Vec3Net::new(0.0, 1.0, -1.0),
        },
        ServerMessage::PlayerKilled {
            killer: Some(crate::protocol::ClientId(2)),
            killer_name: Some("Golden Peer".to_owned()),
            respawn_bags: vec![RespawnBagOption {
                id: crate::protocol::DeployedEntityId(30),
                name: "camp".to_owned(),
                cooldown_seconds: 90,
            }],
        },
        ServerMessage::DoorCodePrompt {
            id: crate::protocol::DeployedEntityId(31),
        },
        ServerMessage::ResourceNodeDepleted {
            id: crate::protocol::ResourceNodeId(1001),
        },
        ServerMessage::WorldTime(world_time),
        ServerMessage::MeteorShower {
            meteors: vec![MeteorStrike {
                impact_position: Vec3Net::new(2.0, 0.0, 3.0),
                impact_tick: 999,
                trajectory_seed: 4242,
                size: 1.0,
            }],
        },
        ServerMessage::Explosion {
            position: Vec3Net::new(5.0, 0.0, 6.0),
            kind: ExplosiveKind::SatchelCharge,
        },
        ServerMessage::Voice {
            speaker: crate::protocol::ClientId(2),
            sequence: 13,
            position: Vec3Net::new(7.0, 1.0, 8.0),
            frame: vec![4, 5, 6],
        },
        ServerMessage::PerfStats(PerfStatsSnapshot {
            loaded_chunks: 10,
            live_nodes: 20,
            pending_regrows: 3,
            aoi_visible_nodes: 8,
            player_chunk_x: -1,
            player_chunk_z: 2,
            player_classification: PerfClassificationId::None,
        }),
        ServerMessage::Pong {
            client_time_ms: 5000,
        },
        ServerMessage::PlayerList(vec![PlayerListEntry {
            client_id: crate::protocol::ClientId(1),
            name: "Golden Player".to_owned(),
            ping_ms: 40,
        }]),
        ServerMessage::Heartbeat,
        ServerMessage::DoorCodeResult { accepted: true },
        ServerMessage::WorldMapMarkers {
            markers: vec![WorldMapMarker {
                id: 14,
                x: 100.0,
                z: -50.0,
                name: "home".to_owned(),
            }],
        },
    ]
}

// Compile-time exhaustiveness reminder: this match failing to compile means
// you added a wire variant: add a fixture above, bump PROTOCOL_VERSION, and
// update the golden hash.
#[allow(dead_code)]
fn _client_fixture_reminder(message: ClientMessage) {
    match message {
        ClientMessage::Auth { .. } => {}
        ClientMessage::Movement(_) => {}
        ClientMessage::Chat { .. } => {}
        ClientMessage::Command { .. } => {}
        ClientMessage::Inventory(_) => {}
        ClientMessage::Crafting(_) => {}
        ClientMessage::Gather(_) => {}
        ClientMessage::PlaceDeployable(_) => {}
        ClientMessage::Furnace(_) => {}
        ClientMessage::Workbench(_) => {}
        ClientMessage::Ranged(_) => {}
        ClientMessage::Explosive(_) => {}
        ClientMessage::DamageDeployable(_) => {}
        ClientMessage::AttackPlayer(_) => {}
        ClientMessage::SwingStart(_) => {}
        ClientMessage::Respawn => {}
        ClientMessage::RespawnAtBag { .. } => {}
        ClientMessage::PlaceBuilding(_) => {}
        ClientMessage::Building(_) => {}
        ClientMessage::Door(_) => {}
        ClientMessage::SleepingBag(_) => {}
        ClientMessage::Claim(_) => {}
        ClientMessage::LootBag(_) => {}
        ClientMessage::LootSleeper { .. } => {}
        ClientMessage::SetViewRadius { .. } => {}
        ClientMessage::Voice(_) => {}
        ClientMessage::Heartbeat => {}
        ClientMessage::Ping { .. } => {}
        ClientMessage::Disconnect => {}
        ClientMessage::OpenStorageBox { .. } => {}
        ClientMessage::RequestWorldMap => {}
        ClientMessage::WorldMapMarker(_) => {}
        ClientMessage::Consumable(_) => {}
    }
}

// Compile-time exhaustiveness reminder: this match failing to compile means
// you added a wire variant: add a fixture above, bump PROTOCOL_VERSION, and
// update the golden hash.
#[allow(dead_code)]
fn _server_fixture_reminder(message: ServerMessage) {
    match message {
        ServerMessage::Welcome { .. } => {}
        ServerMessage::AuthRejected { .. } => {}
        ServerMessage::VersionMismatch { .. } => {}
        ServerMessage::Kicked { .. } => {}
        ServerMessage::PlayerEvent(_) => {}
        ServerMessage::Correction(_) => {}
        ServerMessage::Chat(_) => {}
        ServerMessage::ItemMerged { .. } => {}
        ServerMessage::Toast(_) => {}
        ServerMessage::ResourceImpact { .. } => {}
        ServerMessage::PlayerImpact { .. } => {}
        ServerMessage::ProjectileImpact { .. } => {}
        ServerMessage::Knockback { .. } => {}
        ServerMessage::PlayerKilled { .. } => {}
        ServerMessage::DoorCodePrompt { .. } => {}
        ServerMessage::ResourceNodeDepleted { .. } => {}
        ServerMessage::WorldTime(_) => {}
        ServerMessage::MeteorShower { .. } => {}
        ServerMessage::Explosion { .. } => {}
        ServerMessage::Voice { .. } => {}
        ServerMessage::PerfStats(_) => {}
        ServerMessage::Pong { .. } => {}
        ServerMessage::PlayerList(_) => {}
        ServerMessage::Heartbeat => {}
        ServerMessage::DoorCodeResult { .. } => {}
        ServerMessage::WorldMapMarkers { .. } => {}
    }
}

/// Golden-layout guard, the wire twin of the save format's
/// `world_save_postcard_layout_is_stable`. postcard is a non-self-describing
/// positional format, so reordering or retyping a field in `ClientMessage` /
/// `ServerMessage` (or any nested payload), or inserting an enum variant
/// mid-enum, silently changes the wire byte layout WITHOUT changing
/// [`PROTOCOL_VERSION`], which would make a mismatched client/server pair
/// mis-decode each other with no test going red. This pins the SHA-256 of the
/// postcard payload of one fixture per top-level variant. When it trips, the
/// author must either revert the layout change or deliberately bump
/// `PROTOCOL_VERSION` and regenerate the hash below, turning silent
/// mis-decoding into an explicit decision. `PROTOCOL_VERSION` itself is part
/// of the hashed payload, so bumping the version REQUIRES updating the hash:
/// the two must move together.
#[test]
fn wire_protocol_postcard_layout_is_stable() {
    let client = client_fixtures();
    let server = server_fixtures();

    // Secondary tripwire: one fixture per top-level variant, in declaration
    // order. 33 ClientMessage variants and 26 ServerMessage variants today; a
    // count mismatch means a variant was added or removed without the fixture
    // list following.
    assert_eq!(
        client.len(),
        33,
        "ClientMessage fixture count drifted from the enum's variant count"
    );
    assert_eq!(
        server.len(),
        26,
        "ServerMessage fixture count drifted from the enum's variant count"
    );

    let payload =
        postcard::to_allocvec(&(PROTOCOL_VERSION, client, server)).expect("postcard encode");
    let digest = Sha256::digest(&payload);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex, "6b2d9d1f498b127fe8eaf9d135085dc58eb81d71d237ea73102ecf72f29e0258",
        "Wire protocol postcard layout changed. If intentional, bump \
         PROTOCOL_VERSION and update this golden hash to {hex}."
    );
}
