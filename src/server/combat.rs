//! Server-authoritative PvP combat path.
//!
//! Mirrors the shape of `server/deployables.rs` for placed structures:
//! one entry point per client command, all validation inline, no
//! shared "Damage" enum that fans out to wildly different code paths.
//!
//! ## Validation order
//!
//! Every rejection bails out before any state mutation, so a modified
//! client that tries to forge an `AttackPlayer` message for an
//! out-of-range or wall-hidden target gets no damage and no toast:
//!
//! 1. Cooldown — the swinger's `next_attack_tick` must have elapsed.
//! 2. Self-attack — `attacker == target` is silently dropped.
//! 3. Target alive — no chain damage on a corpse.
//! 4. Real tool — bare hands and non-tool items can't deal PvP damage.
//! 5. Range — feet-to-feet distance must be within [`ATTACK_RANGE_M`].
//! 6. View cone — target must sit inside the attacker's look cone.
//! 7. Line-of-sight — no solid block between eye and chest.
//!
//! On success the post-armor damage is subtracted from the target's
//! health, a `PlayerImpact` is broadcast to peers (except the
//! attacker, who already produced their own predicted feedback), and
//! a `Knockback` impulse is sent privately to the target. HP itself
//! ships via the replicated `PlayerPublic.health` diff — no separate
//! message.

use crate::{
    combat::{damage_after_armor, tool_player_damage},
    controller::BlockGrid,
    items::{HANDS_TOOL, item_definition},
    protocol::{AttackPlayerCommand, ClientId, MAX_HEALTH, PlayerState, ServerMessage, Vec3Net},
    server::{DeliveryTarget, GameServer, PlayerLifecycle, ServerEnvelope},
};

use crate::game_balance::{
    COMBAT_ATTACK_CONE_COS as ATTACK_CONE_COS, COMBAT_ATTACK_RANGE_M as ATTACK_RANGE_M,
    COMBAT_ATTACKER_EYE_HEIGHT as ATTACKER_EYE_HEIGHT,
    COMBAT_KNOCKBACK_VERTICAL_FRACTION as KNOCKBACK_VERTICAL_FRACTION,
    COMBAT_TARGET_CHEST_HEIGHT as TARGET_CHEST_HEIGHT,
};

impl GameServer {
    /// Process a client's `AttackPlayer` request. All validation is
    /// re-done server-side: an exploited client that fabricates the
    /// message gets the same rejections as a legitimate near-miss.
    pub(super) fn apply_attack_player_command(
        &mut self,
        attacker_id: ClientId,
        command: AttackPlayerCommand,
    ) -> Vec<ServerEnvelope> {
        let target_id = command.target_player_id;
        if target_id == attacker_id {
            return Vec::new();
        }

        let Some(attacker) = self.clients.get(&attacker_id) else {
            return Vec::new();
        };
        if self.tick < attacker.next_attack_tick {
            return Vec::new();
        }
        let attacker_pos = attacker.controller.position;
        let attacker_yaw = attacker.controller.yaw;
        let attacker_pitch = attacker.controller.pitch;
        let attacker_name = attacker.name.clone();
        let tool_profile = attacker
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|def| def.tool)
            .unwrap_or(HANDS_TOOL);

        let Some(damage_instance) = tool_player_damage(tool_profile.kind, attacker_id) else {
            // Hands or non-combat tool — nothing to do. Cooldown is not
            // touched because no swing was accepted (the client gates
            // bare-hand swings too; defence in depth).
            return Vec::new();
        };

        // Attackers themselves must be alive to swing. A dying-frame
        // race could otherwise let a corpse fire one last attack.
        let Some(attacker) = self.clients.get(&attacker_id) else {
            return Vec::new();
        };
        if attacker.lifecycle.is_dead() {
            return Vec::new();
        }

        let Some(target) = self.clients.get(&target_id) else {
            return Vec::new();
        };
        // Lifecycle is the authoritative "is this a corpse?" check.
        // HP-at-zero with `Alive` would only happen for the single
        // tick between damage landing and `kill_player` flipping
        // lifecycle; the inequality covers that too.
        if target.lifecycle.is_dead() || target.controller.health <= 0.0 {
            return Vec::new();
        }
        let target_pos = target.controller.position;
        let target_armor = target.armor;
        let target_health_before = target.controller.health;

        let attacker_eye = Vec3Net::new(
            attacker_pos.x,
            attacker_pos.y + ATTACKER_EYE_HEIGHT,
            attacker_pos.z,
        );
        let target_chest = Vec3Net::new(
            target_pos.x,
            target_pos.y + TARGET_CHEST_HEIGHT,
            target_pos.z,
        );

        // Range — feet-to-feet horizontal distance keeps the check
        // close to "can my swing reach them?" without bias from
        // height differences (a target standing on a one-block step
        // is still meleeable).
        let dx = target_pos.x - attacker_pos.x;
        let dz = target_pos.z - attacker_pos.z;
        if (dx * dx + dz * dz).sqrt() > ATTACK_RANGE_M {
            return Vec::new();
        }

        // View cone — direction from eye to target chest must sit
        // inside the attacker's forward cone.
        let forward = crate::items::look_forward(attacker_yaw, attacker_pitch);
        let to_target = target_chest.minus(attacker_eye);
        let to_target_len = to_target.length_squared().sqrt();
        if to_target_len <= f32::EPSILON {
            return Vec::new();
        }
        if to_target.dot(forward) / to_target_len < ATTACK_CONE_COS {
            return Vec::new();
        }

        // LOS — refuse a swing that has to pass through a solid block.
        if !line_of_sight_clear(&self.world_grid, attacker_eye, target_chest) {
            return Vec::new();
        }

        let damage_dealt = damage_after_armor(damage_instance.raw, target_armor);
        let mut envelopes = Vec::new();
        if damage_dealt == 0 {
            // Fully blocked by armor today is impossible (armor is 0)
            // but the path handles it: cooldown still ticks, no HP
            // diff, no feedback to peers (no actual hit landed).
            self.set_attack_cooldown(attacker_id, tool_profile.cooldown_ticks);
            return envelopes;
        }

        let new_health = (target_health_before - damage_dealt as f32).max(0.0);
        if let Some(target_mut) = self.clients.get_mut(&target_id) {
            target_mut.controller.health = new_health;
        }
        let _ = target_health_before;

        // Knockback direction: horizontal attacker → target, with a
        // small upward component so the target slides instead of
        // grinding into the floor.
        let knockback_impulse =
            knockback_impulse(attacker_pos, target_pos, damage_instance.knockback_speed);
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Client(target_id),
            message: ServerMessage::Knockback {
                impulse: knockback_impulse,
            },
        });

        // Peers see the impact; the attacker already produced their
        // own feedback via prediction.
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::BroadcastExcept(attacker_id),
            message: ServerMessage::PlayerImpact {
                attacker: attacker_id,
                target: target_id,
                position: target_chest,
                tool: tool_profile.kind,
                damage_dealt,
            },
        });

        // Phase 5 hooks in: if HP just hit zero, this is also a kill.
        if new_health <= 0.0 {
            envelopes.extend(self.kill_player(target_id, Some(attacker_id), &attacker_name));
        }

        self.set_attack_cooldown(attacker_id, tool_profile.cooldown_ticks);
        envelopes
    }

    /// Honour a client's respawn request. Rejected when the issuer is
    /// already alive (no resurrecting yourself between hits) or when
    /// they aren't connected. On success the controller is reset to a
    /// safe random spawn position, health refilled, lifecycle flipped
    /// to `Alive`, and a `Correction` message snaps the client
    /// predictor onto the new pose.
    pub(super) fn apply_respawn_command(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.lifecycle.is_dead() {
            return Vec::new();
        }

        let spawn = self.pick_safe_respawn_position(client_id);

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        client.controller.position = spawn;
        client.controller.velocity = Vec3Net::ZERO;
        client.controller.health = MAX_HEALTH;
        client.controller.grounded = true;
        client.lifecycle = PlayerLifecycle::Alive;
        // Don't keep the cooldown from before-death — the player just
        // returned to the world, swinging shouldn't be stunlocked.
        client.next_attack_tick = self.tick;
        client.next_gather_tick = self.tick;

        // Re-anchor chunk membership so AoI rooms update before the
        // client even sees the respawn frame.
        self.chunk_manager.update_player_chunk(client_id, spawn);

        let state = PlayerState {
            client_id,
            position: spawn,
            velocity: Vec3Net::ZERO,
            yaw: self
                .clients
                .get(&client_id)
                .map(|c| c.controller.yaw)
                .unwrap_or(0.0),
            pitch: self
                .clients
                .get(&client_id)
                .map(|c| c.controller.pitch)
                .unwrap_or(0.0),
            health: MAX_HEALTH,
            grounded: true,
            last_processed_input: self
                .clients
                .get(&client_id)
                .map(|c| c.controller.last_processed_input)
                .unwrap_or(0),
        };
        vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Correction(state),
        }]
    }

    /// Final kill chain. Called from the damage path when post-armor HP
    /// hits zero. Drops every inventory + actionbar slot at the death
    /// position so the killer can loot the corpse, flips the
    /// lifecycle to `Dead`, and ships a `PlayerKilled` to the dying
    /// client so its UI can open the death splash.
    fn kill_player(
        &mut self,
        target_id: ClientId,
        killer_id: Option<ClientId>,
        killer_name: &str,
    ) -> Vec<ServerEnvelope> {
        // Snapshot the death position before doing anything else; the
        // drop loop borrows the inventory mutably and we'll lose access
        // to the controller mid-iteration otherwise.
        let Some(client) = self.clients.get(&target_id) else {
            return Vec::new();
        };
        let death_position = client.controller.position;

        // Drain every inventory + actionbar slot into a single loot
        // bag at the death position. Looters scoop the corpse with
        // one E-open instead of vacuuming a pile of individual
        // dropped stacks. The bag despawns when emptied + closed
        // (see `loot_bag::close_loot_bag`).
        let drops: Vec<crate::protocol::ItemStack> = {
            let Some(client) = self.clients.get_mut(&target_id) else {
                return Vec::new();
            };
            let mut drops = Vec::new();
            for slot in client.inventory.actionbar_slots.iter_mut() {
                if let Some(stack) = slot.take() {
                    drops.push(stack);
                }
            }
            for slot in client.inventory.inventory_slots.iter_mut() {
                if let Some(stack) = slot.take() {
                    drops.push(stack);
                }
            }
            drops
        };
        if !drops.is_empty() {
            self.spawn_loot_bag(death_position, 0.0, drops);
        }

        // Now flip lifecycle + lock health at zero so any pending
        // damage path with stale state can't double-kill or knock the
        // corpse around.
        if let Some(client) = self.clients.get_mut(&target_id) {
            client.lifecycle = PlayerLifecycle::Dead {
                since_tick: self.tick,
                killer: killer_id,
            };
            client.controller.health = 0.0;
            client.controller.velocity = Vec3Net::ZERO;
        }

        let killer_name = (!killer_name.is_empty()).then(|| killer_name.to_owned());
        vec![ServerEnvelope {
            target: DeliveryTarget::Client(target_id),
            message: ServerMessage::PlayerKilled {
                killer: killer_id,
                killer_name,
            },
        }]
    }

    fn set_attack_cooldown(&mut self, attacker_id: ClientId, cooldown_ticks: u64) {
        if let Some(client) = self.clients.get_mut(&attacker_id) {
            client.next_attack_tick = self.tick + cooldown_ticks.max(1);
        }
    }

    /// Sample candidate spawn positions until one lands clear of other
    /// players and inside playable bounds. The world floor is a flat
    /// plane at y = 0, so "valid" today just means: at least
    /// [`RESPAWN_MIN_DISTANCE_M`] from every other player. Bails out to
    /// world origin after a fixed number of attempts so the call can't
    /// hang.
    fn pick_safe_respawn_position(&self, respawner: ClientId) -> Vec3Net {
        const ATTEMPTS: u32 = 24;
        const RADIUS_M: f32 = 32.0;
        const MIN_RADIUS_M: f32 = 6.0;

        let alive_positions: Vec<Vec3Net> = self
            .clients
            .values()
            .filter(|c| c.client_id != respawner && c.lifecycle.is_alive())
            .map(|c| c.controller.position)
            .collect();

        // RNG state mixes tick + respawner id so back-to-back respawns
        // by the same client land on different spots, and simultaneous
        // respawns by different clients don't collide on a single
        // picked square. SplitMix64 keeps this self-contained — no
        // dependency on the `commands.rs` private RNG type.
        let mut rng_state = self
            .tick
            .wrapping_mul(0x9E3779B97F4A7C15)
            .wrapping_add(respawner.wrapping_mul(0xBF58476D1CE4E5B9))
            .wrapping_add(0xD1B54A32D192ED03);

        for _ in 0..ATTEMPTS {
            let angle = next_f32(&mut rng_state) * std::f32::consts::TAU;
            let radius = MIN_RADIUS_M + next_f32(&mut rng_state) * (RADIUS_M - MIN_RADIUS_M);
            let candidate = Vec3Net::new(angle.cos() * radius, 0.0, angle.sin() * radius);

            let clear = alive_positions.iter().all(|other| {
                let dx = candidate.x - other.x;
                let dz = candidate.z - other.z;
                (dx * dx + dz * dz).sqrt() >= RESPAWN_MIN_DISTANCE_M
            });
            if clear {
                return candidate;
            }
        }
        // Fallback: world origin. Better a maybe-overlapping respawn
        // than an infinite loop in the picker.
        Vec3Net::ZERO
    }
}

/// Tiny SplitMix64 step → [0, 1). Self-contained so the combat module
/// doesn't reach into `commands::SmallRng` (private) or pull in the
/// `rand` crate for a one-off sample.
fn next_f32(state: &mut u64) -> f32 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z = z ^ (z >> 31);
    ((z >> 40) as f32) / ((1u64 << 24) as f32)
}

use crate::game_balance::RESPAWN_MIN_DISTANCE_M;

fn knockback_impulse(attacker_pos: Vec3Net, target_pos: Vec3Net, speed: f32) -> Vec3Net {
    let dx = target_pos.x - attacker_pos.x;
    let dz = target_pos.z - attacker_pos.z;
    let len_sq = dx * dx + dz * dz;
    if len_sq <= f32::EPSILON {
        // Co-located edge case: shove straight up so the target
        // separates from the attacker before re-grounding.
        return Vec3Net::new(0.0, speed * KNOCKBACK_VERTICAL_FRACTION, 0.0);
    }
    let inv = len_sq.sqrt().recip();
    Vec3Net::new(
        dx * inv * speed,
        speed * KNOCKBACK_VERTICAL_FRACTION,
        dz * inv * speed,
    )
}

/// True when no solid block sits between `from` and `to`. Walks the
/// candidate blocks the spatial chunk hands back for the swept query
/// and runs a ray-AABB entry test against each. Returns `false` (LOS
/// blocked) as soon as a hit is found before the target.
fn line_of_sight_clear(grid: &BlockGrid, from: Vec3Net, to: Vec3Net) -> bool {
    let direction = to.minus(from);
    let length = direction.length_squared().sqrt();
    if length <= f32::EPSILON {
        return true;
    }
    let inv_length = length.recip();
    let dir_normalised = Vec3Net::new(
        direction.x * inv_length,
        direction.y * inv_length,
        direction.z * inv_length,
    );
    // Use the swept query so even a long melee step (e.g. one player
    // on a step pad above the other) reads every cell the ray
    // crosses, not just the two endpoints.
    let candidates = grid.candidates_for_swept(from, direction.x, direction.z);
    for index in candidates {
        let block = grid.block(index);
        if let Some(distance) = ray_aabb_entry(from, dir_normalised, block)
            && distance >= 0.0
            && distance < length
        {
            return false;
        }
    }
    true
}

/// Slab-method ray-AABB intersection returning the entry distance
/// along `direction` (which is assumed normalised). `None` when the
/// ray misses or the box is entirely behind the origin.
fn ray_aabb_entry(
    origin: Vec3Net,
    direction: Vec3Net,
    block: crate::world::WorldBlock,
) -> Option<f32> {
    let min = block.min();
    let max = block.max();
    let mut t_near: f32 = f32::NEG_INFINITY;
    let mut t_far: f32 = f32::INFINITY;
    for axis in 0..3 {
        let (o, d, mn, mx) = match axis {
            0 => (origin.x, direction.x, min.x, max.x),
            1 => (origin.y, direction.y, min.y, max.y),
            _ => (origin.z, direction.z, min.z, max.z),
        };
        if d.abs() < 1e-6 {
            if o < mn || o > mx {
                return None;
            }
            continue;
        }
        let inv = d.recip();
        let mut t1 = (mn - o) * inv;
        let mut t2 = (mx - o) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_near = t_near.max(t1);
        t_far = t_far.min(t2);
        if t_near > t_far {
            return None;
        }
    }
    if t_far < 0.0 {
        return None;
    }
    Some(t_near.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{WorldBlock, WorldData};

    fn grid_with_blocks(blocks: Vec<WorldBlock>) -> BlockGrid {
        let world = WorldData {
            floor_size: 64.0,
            blocks,
            resource_nodes: Vec::new(),
        };
        BlockGrid::build(&world)
    }

    #[test]
    fn los_passes_through_empty_world() {
        let grid = grid_with_blocks(Vec::new());
        let from = Vec3Net::new(0.0, 1.6, 0.0);
        let to = Vec3Net::new(0.0, 1.0, -3.0);
        assert!(line_of_sight_clear(&grid, from, to));
    }

    #[test]
    fn los_blocked_by_wall_between_attacker_and_target() {
        // Wall at z = -1.5, blocking the path from origin → z = -3.
        let grid = grid_with_blocks(vec![WorldBlock::new(
            Vec3Net::new(0.0, 1.0, -1.5),
            Vec3Net::new(2.0, 1.0, 0.25),
        )]);
        let from = Vec3Net::new(0.0, 1.6, 0.0);
        let to = Vec3Net::new(0.0, 1.0, -3.0);
        assert!(!line_of_sight_clear(&grid, from, to));
    }

    #[test]
    fn los_passes_when_block_sits_past_the_target() {
        // Block behind the target shouldn't block the attack.
        let grid = grid_with_blocks(vec![WorldBlock::new(
            Vec3Net::new(0.0, 1.0, -5.0),
            Vec3Net::new(2.0, 1.0, 0.25),
        )]);
        let from = Vec3Net::new(0.0, 1.6, 0.0);
        let to = Vec3Net::new(0.0, 1.0, -3.0);
        assert!(line_of_sight_clear(&grid, from, to));
    }
}
