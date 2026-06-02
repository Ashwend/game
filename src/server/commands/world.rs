//! `/spawn-ore` and `/tp`, world-mutation admin commands, plus the small
//! PRNG and spawn-placement helpers they rely on.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    protocol::{ClientId, ServerMessage, ToastKind, ToastMessage, Vec3Net},
    resources::{
        COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID, resource_node_definition, spawn_resource_node,
    },
    world::{NodeKind, WorldResourceNodeSpawn},
};

use super::super::{DeliveryTarget, GameServer, ServerEnvelope};
use super::{reply_success, reply_warning};

/// Hard limit on `/spawn-ore` radius. Keeps an admin debug command from
/// accidentally placing a node hundreds of meters away in a flat world.
const MAX_SPAWN_ORE_RADIUS: f32 = 30.0;
const DEFAULT_SPAWN_ORE_RADIUS: f32 = 8.0;
/// Minimum distance between the spawned node and the issuer. Keeps the
/// new node from materialising inside the player's collision radius.
pub(super) const MIN_SPAWN_ORE_DISTANCE: f32 = 1.75;

impl GameServer {
    /// `/spawn-ore [coal|iron|sulfur] [radius]`
    ///
    /// Picks a random horizontal offset within `radius` of the issuing
    /// player and inserts a fresh node at floor level. Admin-only.
    pub(super) fn command_spawn_ore(
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

        let mut chosen_ore: Option<&'static str> = None;
        let mut radius = DEFAULT_SPAWN_ORE_RADIUS;
        for arg in args {
            if let Some(ore) = parse_ore_token(arg) {
                chosen_ore = Some(ore);
            } else if let Ok(value) = arg.parse::<f32>() {
                radius = value;
            } else {
                return reply_warning(
                    client_id,
                    format!("unknown argument '{arg}'; expected ore type or radius"),
                );
            }
        }

        if !radius.is_finite() || radius <= 0.0 {
            return reply_warning(client_id, "radius must be a positive number");
        }
        let radius = radius.min(MAX_SPAWN_ORE_RADIUS);

        let player_position = client.controller.position;
        let mut rng = SmallRng::seed_from_time_and(client_id);
        let ore_id = chosen_ore.unwrap_or_else(|| random_ore_id(&mut rng));
        let position = random_position_around(player_position, radius, &mut rng);

        let node_id = self.allocate_resource_node_id();
        let spawn = WorldResourceNodeSpawn::new(
            node_id,
            ore_id,
            position,
            rng.next_f32() * std::f32::consts::TAU,
        );
        let Some(node) = spawn_resource_node(&spawn) else {
            return reply_warning(client_id, "could not build node: unknown ore type");
        };

        let distance = ((position.x - player_position.x).powi(2)
            + (position.z - player_position.z).powi(2))
        .sqrt();
        let label = resource_node_definition(ore_id)
            .map(|definition| definition.name)
            .unwrap_or("Ore");

        // Register with the chunk anchor index so the snapshot AoI
        // includes the spawn, without this, admin-spawned nodes are
        // invisible because per-chunk membership is the AoI source of
        // truth.
        let kind = ore_node_kind(ore_id);
        self.chunk_manager
            .track_resource_node(node_id, kind, position);
        self.insert_resource_node(node_id, node);

        reply_success(
            client_id,
            format!("spawned {label} {distance:.1}m away (id {node_id})"),
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

pub(super) fn parse_ore_token(arg: &str) -> Option<&'static str> {
    match arg.to_ascii_lowercase().as_str() {
        "coal" => Some(COAL_NODE_ID),
        "iron" => Some(IRON_NODE_ID),
        "sulfur" | "sulphur" => Some(SULFUR_NODE_ID),
        _ => None,
    }
}

/// Map an ore `definition_id` to the matching `NodeKind` for chunk
/// membership bookkeeping. Defaults to `CoalOre` for unknown ids so the
/// node still ends up tracked rather than silently invisible, callers
/// only pass ids that came out of `parse_ore_token`, so the fallback
/// shouldn't fire in practice.
fn ore_node_kind(ore_id: &str) -> NodeKind {
    match ore_id {
        IRON_NODE_ID => NodeKind::IronOre,
        SULFUR_NODE_ID => NodeKind::SulfurOre,
        _ => NodeKind::CoalOre,
    }
}

fn random_ore_id(rng: &mut SmallRng) -> &'static str {
    const ORES: [&str; 3] = [COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID];
    ORES[(rng.next_u32() as usize) % ORES.len()]
}

pub(super) fn random_position_around(center: Vec3Net, radius: f32, rng: &mut SmallRng) -> Vec3Net {
    // Uniform sampling over an annulus (MIN_SPAWN_ORE_DISTANCE .. radius)
    // using inverse-CDF in r² so the points are area-uniform rather than
    // clustered toward the center.
    let inner = MIN_SPAWN_ORE_DISTANCE.min(radius * 0.5);
    let inner_sq = inner * inner;
    let outer_sq = radius * radius;
    let r = (inner_sq + rng.next_f32() * (outer_sq - inner_sq)).sqrt();
    let theta = rng.next_f32() * std::f32::consts::TAU;
    Vec3Net::new(
        center.x + r * theta.cos(),
        // Floor-aligned spawn, matches the hand-authored ore nodes in the
        // test world (all at y=0).
        0.0,
        center.z + r * theta.sin(),
    )
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
