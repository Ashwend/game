//! `/spawn` and `/tp`, world-mutation admin commands, plus the small
//! PRNG they rely on.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    protocol::{ClientId, ServerMessage, ToastKind, ToastMessage, Vec3Net},
    resources::{
        BIRCH_TREE_LARGE_NODE_ID, BIRCH_TREE_NODE_ID, BIRCH_TREE_SMALL_NODE_ID,
        BRANCH_PILE_NODE_ID, COAL_NODE_ID, HAY_GRASS_NODE_ID, IRON_NODE_ID,
        PINE_TREE_LARGE_NODE_ID, PINE_TREE_NODE_ID, PINE_TREE_SMALL_NODE_ID,
        RESOURCE_NODE_DEFINITIONS, STONE_NODE_ID, SULFUR_NODE_ID, SURFACE_STONE_NODE_ID,
        resource_node_definition, spawn_resource_node,
    },
    world::{NodeKind, WorldResourceNodeSpawn},
};

use super::super::{DeliveryTarget, GameServer, ServerEnvelope};
use super::{reply_success, reply_warning};

/// Hard limit on `/spawn` distance. Keeps an admin debug command from
/// accidentally placing a node hundreds of meters away in a flat world.
const MAX_SPAWN_DISTANCE: f32 = 30.0;
/// How far ahead the node lands when no distance is given. Far enough
/// that a tall node (tree) doesn't fill the whole screen.
const DEFAULT_SPAWN_DISTANCE: f32 = 4.0;
/// Minimum distance between the spawned node and the issuer. Keeps the
/// new node from materialising inside the player's collision radius.
const MIN_SPAWN_DISTANCE: f32 = 1.75;

/// Alias list echoed back on a bad `/spawn` argument.
const SPAWN_KINDS_HELP: &str =
    "coal, iron, sulfur, stone, pine[-small|-large], birch[-small|-large], rock, sticks, hay";

impl GameServer {
    /// `/spawn <kind> [distance]`
    ///
    /// Inserts a fresh resource node at floor level, directly in front of
    /// the issuing player (along their view yaw), `distance` meters out
    /// (default [`DEFAULT_SPAWN_DISTANCE`]). Admin-only debug command;
    /// accepts any registry node kind, see [`parse_node_token`].
    pub(super) fn command_spawn(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        let mut chosen: Option<&'static str> = None;
        let mut distance = DEFAULT_SPAWN_DISTANCE;
        for arg in args {
            if let Some(definition_id) = parse_node_token(arg) {
                chosen = Some(definition_id);
            } else if let Ok(value) = arg.parse::<f32>() {
                distance = value;
            } else {
                return reply_warning(
                    client_id,
                    format!("unknown node kind '{arg}'; kinds: {SPAWN_KINDS_HELP}"),
                );
            }
        }
        let Some(definition_id) = chosen else {
            return reply_warning(
                client_id,
                format!("usage: /spawn <kind> [distance]; kinds: {SPAWN_KINDS_HELP}"),
            );
        };
        if !distance.is_finite() || distance <= 0.0 {
            return reply_warning(client_id, "distance must be a positive number");
        }
        let distance = distance.clamp(MIN_SPAWN_DISTANCE, MAX_SPAWN_DISTANCE);

        // Straight ahead along the view yaw; same forward convention as
        // the movement sim (see `src/server/movement.rs`). Floor-aligned
        // y=0 matches the generated world's node placement.
        let player_position = client.controller.position;
        let yaw = client.controller.yaw;
        let position = Vec3Net::new(
            player_position.x - yaw.sin() * distance,
            0.0,
            player_position.z - yaw.cos() * distance,
        );

        let mut rng = SmallRng::seed_from_time_and(client_id);
        let node_id = self.allocate_resource_node_id();
        let spawn = WorldResourceNodeSpawn::new(
            node_id,
            definition_id,
            position,
            rng.next_f32() * std::f32::consts::TAU,
        );
        let Some(node) = spawn_resource_node(&spawn) else {
            return reply_warning(client_id, "could not build node: unknown kind");
        };
        // `parse_node_token` only returns registry ids, so the kind lookup
        // can't miss; the guard keeps a future registry mismatch loud.
        let Some(kind) = NodeKind::from_definition_id(definition_id) else {
            return reply_warning(client_id, "could not map node kind for chunk tracking");
        };

        // Register with the chunk anchor index so the snapshot AoI
        // includes the spawn, without this, admin-spawned nodes are
        // invisible because per-chunk membership is the AoI source of
        // truth.
        self.chunk_manager
            .track_resource_node(node_id, kind, position);
        self.insert_resource_node(node_id, node);

        let label = resource_node_definition(definition_id)
            .map(|definition| definition.name)
            .unwrap_or("Node");
        reply_success(
            client_id,
            format!("spawned {label} {distance:.1}m ahead (id {node_id})"),
        )
    }

    /// `/tp`, teleport every other connected player to the issuer's
    /// position. Bread-and-butter PvP-test command: drop both clients
    /// into the same arms-length so death/respawn/melee can be
    /// exercised without manually walking them together.
    ///
    /// Implementation: validate admin, snapshot the issuer's position,
    /// and for each other connected client move the server-side
    /// controller onto the issuer's spot, re-anchor the chunk
    /// membership, then push a `ServerMessage::Correction` so the
    /// client predictor snaps cleanly. The runtime applies the snap on
    /// a position delta above its 1 m threshold (see
    /// `apply_non_movement_correction`).
    pub(super) fn command_teleport_all(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(issuer) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !issuer.is_admin {
            return reply_warning(client_id, "admin only");
        }
        let target_position = issuer.controller.position;
        let target_yaw = issuer.controller.yaw;

        let other_ids: Vec<ClientId> = self
            .clients
            .keys()
            .copied()
            .filter(|id| *id != client_id)
            .collect();

        let mut envelopes = Vec::new();
        let mut moved = 0u32;
        for other_id in &other_ids {
            let Some(other) = self.clients.get_mut(other_id) else {
                continue;
            };
            // Stamp the controller. Velocity is zeroed so the target
            // doesn't keep their inbound momentum and immediately slide
            // off the issuer's tile.
            other.controller.position = target_position;
            other.controller.velocity = Vec3Net::ZERO;
            // Keep the target's look direction so the camera doesn't
            // snap mid-frame; only the world position should change.
            // Re-anchor chunk membership so AoI replication sees the
            // new home immediately.
            self.chunk_manager
                .update_player_chunk(*other_id, target_position);

            // Synthesize a Correction so the client prediction follows.
            let state = crate::protocol::PlayerState {
                client_id: *other_id,
                position: target_position,
                velocity: Vec3Net::ZERO,
                yaw: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.yaw)
                    .unwrap_or(target_yaw),
                pitch: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.pitch)
                    .unwrap_or(0.0),
                health: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.health)
                    .unwrap_or(crate::protocol::MAX_HEALTH),
                grounded: true,
                last_processed_input: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.last_processed_input)
                    .unwrap_or(0),
            };
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Client(*other_id),
                message: ServerMessage::Correction(state),
            });
            moved += 1;
        }

        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Toast(ToastMessage::new(
                ToastKind::Success,
                if moved == 0 {
                    "no other players to teleport".to_owned()
                } else if moved == 1 {
                    "teleported 1 player to your position".to_owned()
                } else {
                    format!("teleported {moved} players to your position")
                },
            )),
        });
        envelopes
    }
}

/// Resolve a `/spawn` kind token to a registry `definition_id`. Accepts
/// short aliases (`pine`, `sticks`, `rock`), hyphen or underscore
/// separators, and any exact registry id (`pine_tree_small`, `coal_node`).
pub(super) fn parse_node_token(arg: &str) -> Option<&'static str> {
    let normalized = arg.to_ascii_lowercase().replace('-', "_");
    let alias = match normalized.as_str() {
        "coal" => Some(COAL_NODE_ID),
        "iron" => Some(IRON_NODE_ID),
        "sulfur" | "sulphur" => Some(SULFUR_NODE_ID),
        "stone" | "stone_vein" | "vein" => Some(STONE_NODE_ID),
        "pine_small" | "pine_sapling" => Some(PINE_TREE_SMALL_NODE_ID),
        "pine" => Some(PINE_TREE_NODE_ID),
        "pine_large" | "old_pine" => Some(PINE_TREE_LARGE_NODE_ID),
        "birch_small" | "birch_sapling" => Some(BIRCH_TREE_SMALL_NODE_ID),
        "birch" => Some(BIRCH_TREE_NODE_ID),
        "birch_large" | "old_birch" => Some(BIRCH_TREE_LARGE_NODE_ID),
        "rock" | "loose_stone" => Some(SURFACE_STONE_NODE_ID),
        "sticks" | "stick" | "branch" | "branches" | "branch_pile" => Some(BRANCH_PILE_NODE_ID),
        "hay" | "grass" | "tall_grass" => Some(HAY_GRASS_NODE_ID),
        _ => None,
    };
    alias.or_else(|| {
        RESOURCE_NODE_DEFINITIONS
            .iter()
            .find(|definition| definition.id == normalized)
            .map(|definition| definition.id)
    })
}

/// Tiny xorshift32 PRNG. We don't need cryptographic randomness, just a
/// stream of "feels different" numbers between admin command invocations.
/// Avoids adding the `rand` crate just for one debug command.
pub(super) struct SmallRng {
    pub(super) state: u32,
}

impl SmallRng {
    pub(super) fn seed_from_time_and(salt: u64) -> Self {
        // SystemTime fold-in for entropy across server restarts; the salt
        // mixes in client_id so two admins spawning in the same tick don't
        // get identical sequences.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0);
        let mut state = (nanos ^ (salt as u32) ^ ((salt >> 32) as u32)).wrapping_mul(0x9E37_79B1);
        if state == 0 {
            state = 0xDEAD_BEEF;
        }
        Self { state }
    }

    pub(super) fn next_u32(&mut self) -> u32 {
        // Classic xorshift32, short period for our purposes is fine.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    pub(super) fn next_f32(&mut self) -> f32 {
        // 24 bits of mantissa is plenty for picking world positions.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}
