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
//! 1. Cooldown, the swinger's `next_attack_tick` must have elapsed.
//! 2. Self-attack, `attacker == target` is silently dropped.
//! 3. Target alive, no chain damage on a corpse.
//! 4. Real tool, bare hands and non-tool items can't deal PvP damage.
//! 5. Range, feet-to-feet distance must be within [`ATTACK_RANGE_M`].
//! 6. View cone, target must sit inside the attacker's look cone.
//! 7. Line-of-sight, no solid block between eye and chest.
//!
//! On success the post-armor damage is subtracted from the target's
//! health, a `PlayerImpact` is broadcast to peers (except the
//! attacker, who already produced their own predicted feedback), and
//! a `Knockback` impulse is sent privately to the target. HP itself
//! ships via the replicated `PlayerPublic.health` diff, no separate
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

/// Hit-point height above the feet for a logged-out sleeping body. The avatar
/// is laid flat on the ground, so the swing lands near floor level rather than
/// at standing chest height.
const SLEEPING_HIT_HEIGHT: f32 = 0.35;

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

        let Some(damage_instance) = tool_player_damage(tool_profile, attacker_id) else {
            // Hands or non-combat tool, nothing to do. Cooldown is not
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
        // A logged-out body lies flat on the ground, so its hittable point is
        // near the floor, not at standing chest height, and a standing
        // attacker is looking steeply down at it. Aim the hit point low and
        // waive the view cone for a helpless sleeper (range + line-of-sight
        // still gate the swing); otherwise the upright chest point plus the
        // tight cone make a lying body almost impossible to land on.
        let target_sleeping = !target.online;
        let target_pos = target.controller.position;
        let target_armor = target.armor;
        let target_health_before = target.controller.health;

        let attacker_eye = Vec3Net::new(
            attacker_pos.x,
            attacker_pos.y + ATTACKER_EYE_HEIGHT,
            attacker_pos.z,
        );
        let hit_height = if target_sleeping {
            SLEEPING_HIT_HEIGHT
        } else {
            TARGET_CHEST_HEIGHT
        };
        let target_chest = Vec3Net::new(target_pos.x, target_pos.y + hit_height, target_pos.z);

        // Range, feet-to-feet horizontal distance keeps the check
        // close to "can my swing reach them?" without bias from
        // height differences (a target standing on a one-block step
        // is still meleeable).
        if !attacker_pos.within_horizontal_range(target_pos, ATTACK_RANGE_M) {
            return Vec::new();
        }

        // View cone, direction from eye to target chest must sit
        // inside the attacker's forward cone (skipped for sleepers).
        let forward = crate::items::look_forward(attacker_yaw, attacker_pitch);
        let to_target = target_chest.minus(attacker_eye);
        let to_target_len = to_target.length_squared().sqrt();
        if to_target_len <= f32::EPSILON {
            return Vec::new();
        }
        if !target_sleeping && to_target.dot(forward) / to_target_len < ATTACK_CONE_COS {
            return Vec::new();
        }

        // LOS, refuse a swing that has to pass through a solid block.
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

        // Tell the victim their HP dropped. Health is server-authoritative,
        // the client never predicts its own damage, so the target's local
        // prediction (which drives their HP bar) only changes when a
        // `Correction` arrives. Peers learn the new HP through the player
        // mirror's replicated `health`, but the victim renders themselves from
        // prediction, not their own mirror, so without this their bar stays
        // full even as the server records every hit. Pushed before the
        // knockback envelope so the knockback impulse is applied last on the
        // client and survives even if this correction snaps position on a
        // high-latency link (`apply_non_movement_correction` only snaps past a
        // 1 m divergence; a normal hit just overwrites health).
        if let Some(target_ref) = self.clients.get(&target_id) {
            let controller = &target_ref.controller;
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Client(target_id),
                message: ServerMessage::Correction(PlayerState {
                    client_id: target_id,
                    position: controller.position,
                    velocity: controller.velocity,
                    yaw: controller.yaw,
                    pitch: controller.pitch,
                    health: new_health,
                    grounded: controller.grounded,
                    last_processed_input: controller.last_processed_input,
                }),
            });
        }

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

        // Peers within perception range see the impact; the attacker
        // already produced their own feedback via prediction, and
        // distant clients can neither hear nor see it.
        envelopes.extend(self.envelopes_within_range(
            target_chest,
            crate::game_balance::IMPACT_MESSAGE_RANGE_M,
            Some(attacker_id),
            ServerMessage::PlayerImpact {
                attacker: attacker_id,
                target: target_id,
                position: target_chest,
                attacker_position: attacker_pos,
                tool: tool_profile.kind,
                damage_dealt,
            },
        ));

        // Phase 5 hooks in: if HP just hit zero, this is also a kill.
        if new_health <= 0.0 {
            envelopes.extend(self.kill_player(target_id, Some(attacker_id), &attacker_name));
        }

        // The swing connected, so the attacker's tool wears. After the
        // kill handling on purpose: the killing blow lands even if it is
        // also the swing that breaks the tool.
        envelopes.extend(self.consume_active_tool_durability(attacker_id));

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

        let spawn = self.pick_safe_spawn(Some(client_id));

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        client.controller.position = spawn;
        client.controller.velocity = Vec3Net::ZERO;
        client.controller.health = MAX_HEALTH;
        client.controller.grounded = true;
        client.lifecycle = PlayerLifecycle::Alive;
        // Don't keep the cooldown from before-death, the player just
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
        // (see `loot_bag::close_container`).
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
        // If this was a sleeping body someone had open, their live-inventory
        // view just emptied into the death bag; close it so a stale Move can't
        // reach into the now-dead body.
        self.close_sleeper_views(target_id);

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
        // Offer the dying player their placed sleeping bags as spawn
        // points; the death screen renders one button per bag.
        let respawn_bags = self
            .clients
            .get(&target_id)
            .map(|client| self.respawn_bag_options(client.account_id))
            .unwrap_or_default();
        vec![ServerEnvelope {
            target: DeliveryTarget::Client(target_id),
            message: ServerMessage::PlayerKilled {
                killer: killer_id,
                killer_name,
                respawn_bags,
            },
        }]
    }

    fn set_attack_cooldown(&mut self, attacker_id: ClientId, cooldown_ticks: u64) {
        if let Some(client) = self.clients.get_mut(&attacker_id) {
            client.next_attack_tick = self.tick + cooldown_ticks.max(1);
        }
    }

    /// Build a collision grid matching what clients actually collide with: the
    /// world blocks (perimeter walls + terrain) plus resource-node and
    /// deployable colliders. Rebuilt per spawn pick; spawns are rare so the
    /// O(nodes + structures) build is cheap, and rebuilding keeps the check
    /// honest as nodes regrow and structures come and go.
    pub(super) fn spawn_collision_grid(&self) -> BlockGrid {
        let mut extras: Vec<crate::world::WorldBlock> = self
            .resource_nodes
            .values()
            .filter_map(crate::resources::resource_node_collider)
            .collect();
        extras.extend(
            self.deployed_entities
                .values()
                .flat_map(|e| e.resolved_collider_blocks()),
        );
        BlockGrid::build_with_extras(&self.world, &extras)
    }

    /// Pick a random spawn point anywhere inside the playable bounds that
    /// doesn't drop the player inside a solid collider (wall, tree, ore, or
    /// placed structure) and stays clear of other live players. `exclude` is
    /// the respawning client (skipped in the player-distance test); pass `None`
    /// for a fresh join. Used for both initial spawn and respawn so the two
    /// behave identically. Falls back to the first collider-free sample if no
    /// spot also clears the player-distance check, and only to the origin if
    /// every sample landed in geometry (effectively never on an open map).
    pub(super) fn pick_safe_spawn(&self, exclude: Option<ClientId>) -> Vec3Net {
        use crate::world::PlayableBounds;
        const ATTEMPTS: u32 = 64;
        // Keep the player capsule clear of the inner wall face / bounds edge.
        const EDGE_MARGIN_M: f32 = 4.0;

        let bounds = PlayableBounds::from_dims(self.chunk_manager.dims());
        let min_x = bounds.min_x + EDGE_MARGIN_M;
        let max_x = bounds.max_x - EDGE_MARGIN_M;
        let min_z = bounds.min_z + EDGE_MARGIN_M;
        let max_z = bounds.max_z - EDGE_MARGIN_M;
        let span_x = (max_x - min_x).max(0.0);
        let span_z = (max_z - min_z).max(0.0);

        let grid = self.spawn_collision_grid();

        let alive_positions: Vec<Vec3Net> = self
            .clients
            .values()
            .filter(|c| Some(c.client_id) != exclude && c.lifecycle.is_alive())
            .map(|c| c.controller.position)
            .collect();

        // RNG mixes tick + the (optional) client id so back-to-back picks and
        // simultaneous picks by different clients diverge. SplitMix64 keeps this
        // self-contained, no dependency on the `commands.rs` private RNG type.
        let mut rng_state = self
            .tick
            .wrapping_mul(0x9E3779B97F4A7C15)
            .wrapping_add(exclude.unwrap_or(0).wrapping_mul(0xBF58476D1CE4E5B9))
            .wrapping_add(0xD1B54A32D192ED03);

        let min_distance_sq = RESPAWN_MIN_DISTANCE_M * RESPAWN_MIN_DISTANCE_M;
        let mut collider_free_fallback: Option<Vec3Net> = None;

        for _ in 0..ATTEMPTS {
            let x = min_x + next_f32(&mut rng_state) * span_x;
            let z = min_z + next_f32(&mut rng_state) * span_z;
            let candidate = Vec3Net::new(x, 0.0, z);

            if crate::controller::player_overlaps_world(candidate, &grid) {
                continue;
            }
            if collider_free_fallback.is_none() {
                collider_free_fallback = Some(candidate);
            }
            let clear_of_players = alive_positions
                .iter()
                .all(|other| candidate.horizontal_distance_squared(*other) >= min_distance_sq);
            if clear_of_players {
                return candidate;
            }
        }
        // Better a tight-but-open spot than the origin; origin only if every
        // sample was inside geometry.
        collider_free_fallback.unwrap_or(Vec3Net::ZERO)
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
