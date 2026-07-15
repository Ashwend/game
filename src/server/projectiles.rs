//! Server-authoritative projectile (arrow) simulation and the ranged-weapon
//! fire path.
//!
//! A bow or crossbow does not swing. It draws (tracked on `ServerClient::
//! draw_started_tick`) and fires a projectile the server owns end to end: the
//! `RangedCommand` handlers here validate the weapon, ammo, cooldown, and aim,
//! compute the draw-scaled damage, take one arrow, and spawn a [`Projectile`]
//! into `GameServer::projectiles`. `tick_projectiles` then integrates each one
//! under gravity, sweeps the tick's segment against blocks, players, and
//! deployables, and resolves the first hit through the shared post-hit tail
//! (`apply_player_damage`) or the deployable damage path.
//!
//! ## Collision-grid perf choice
//!
//! Each `tick_projectiles` call builds the everything-solid [`BlockGrid`] once
//! via [`GameServer::spawn_collision_grid`] and reuses it for every live
//! projectile that tick, rather than rebuilding per projectile. Projectiles are
//! few and short-lived (an 8 s cap, `SERVER_TICK_RATE_HZ` ticks/s), so one build
//! per tick amortises across all of them. This mirrors how the spawn picker
//! builds the grid once per pick; the melee LOS path builds per swing because a
//! swing is a single query, but the projectile tick is a batch, so a single
//! shared grid is the right trade here.

use crate::{
    combat::{DamageKind, damage_after_armor, effective_armor_after_pierce},
    controller::BlockGrid,
    items::{ExplosiveKind, ItemModel, item_definition},
    protocol::{
        ClientId, ExplosiveCommand, ProjectileId, ProjectileSurface, RangedCommand, ServerMessage,
        Vec3Net,
    },
    server::{DeliveryTarget, GameServer, ServerEnvelope},
};

use super::combat::{PlayerDamageHit, ray_aabb_entry};
use crate::inventory::{count_items_in_inventory, take_items_from_inventory};

pub(super) use crate::game_balance::{
    BOW_DRAW_MOVE_MULTIPLIER, COMBAT_ATTACKER_EYE_HEIGHT as ATTACKER_EYE_HEIGHT,
    CROSSBOW_RELOAD_MOVE_MULTIPLIER, IMPACT_MESSAGE_RANGE_M,
    PROJECTILE_DEPLOYABLE_EFFECTIVENESS_PCT, PROJECTILE_GRAVITY, PROJECTILE_MAX_FLIGHT_SECONDS,
    PROJECTILE_REST_DIR_EPSILON, PROJECTILE_SELF_HIT_GRACE_TICKS, PROJECTILE_STUCK_TTL_SECONDS,
};
use crate::protocol::SERVER_TICK_RATE_HZ;

/// Ticks after an arrow comes to rest during which the mirror keeps re-affirming
/// its room membership (see `sync_projectile_entities`). The 0.28 room model
/// latches per-client visibility on a `Rooms` (re)insert and never recomputes it
/// per tick; a rested arrow stops moving (so it never re-anchors on its own), so
/// without this window its visibility would freeze at the rest tick and a
/// stationary shooter would never see it stick. ~1 s at the server tick rate is
/// ample for client AoI subscriptions to settle after the shot lands.
const PROJECTILE_REST_REAFFIRM_TICKS: u64 = SERVER_TICK_RATE_HZ as u64;

/// What a projectile is, which decides its impact behaviour. An arrow damages
/// on contact and sticks recoverably on a world rest; a thrown explosive deals
/// NO contact damage: its fuse is lit on the throw, it bounces and rolls off
/// every solid it meets, and it detonates in place (mid-air, mid-roll, or at
/// rest) when the fuse runs out. Server-only: this never rides the wire (the
/// replicated mirror carries only the `model`, which the client uses for the
/// impact cue), so a new projectile archetype costs nothing on the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectileKind {
    /// A fired arrow: contact damage; a world rest sticks and is E-recoverable.
    Arrow,
    /// A thrown explosive (the powder bomb): no contact damage, bounces and
    /// rolls, detonates as this `ExplosiveKind` when its fuse expires.
    ThrownExplosive(ExplosiveKind),
}

/// One live in-flight projectile. Server-authoritative; the client renders it via
/// the replicated [`crate::server::Projectile`] / [`crate::server::
/// ProjectileTransform`] mirror. Transient (never persisted). `pub(crate)` to
/// match the other authoritative map values (`DeployedEntity`) so the field on
/// `GameServer` stays a clean visibility.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Projectile {
    pub(super) id: ProjectileId,
    /// What this projectile is (arrow vs thrown explosive), which gates its
    /// impact behaviour. Server-only, never replicated.
    pub(super) kind: ProjectileKind,
    /// The client that fired this projectile (skipped for self-hits during the
    /// spawn grace window, and credited on a kill).
    pub(super) owner: ClientId,
    /// The firing weapon's archetype (Bow / Crossbow), carried onto the hit's
    /// impact identity and the peer `ProjectileImpact`.
    pub(super) model: ItemModel,
    /// The ammo item id this shot consumed, so a recovered arrow drops the right
    /// item back into the world.
    pub(super) ammo_item: &'static str,
    /// Post-armor-independent (pre-armor) damage this shot deals on a player hit.
    /// Fixed at fire time from the draw fraction; armor + pierce apply on impact.
    pub(super) damage: u32,
    /// Knockback impulse magnitude the hit applies, in m/s.
    pub(super) knockback_speed: f32,
    pub(super) position: Vec3Net,
    pub(super) velocity: Vec3Net,
    /// Server tick the projectile was spawned, for the self-hit grace window and
    /// the max-flight cap.
    pub(super) spawn_tick: u64,
    /// Ticks left on a thrown explosive's fuse, lit at the throw. Counts down
    /// every tick through flight, bounce, and roll; at zero the bomb detonates
    /// in place via `resolve_explosion`. Always 0 (and unread) for an arrow.
    pub(super) fuse_ticks_left: u32,
}

/// The outcome of stepping one projectile a single tick: keep flying, or it hit
/// something and should be removed after applying `envelopes`.
enum StepResult {
    Flying,
    /// Struck a player / deployable / world. Carries the resolved envelopes and
    /// the impact position + surface for the peer `ProjectileImpact` and (for a
    /// world hit) the recovery drop.
    Hit {
        envelopes: Vec<ServerEnvelope>,
        position: Vec3Net,
        surface: ProjectileSurface,
        /// True only for a world hit: the projectile came to rest and may leave a
        /// recoverable arrow + a cosmetic stuck entity.
        rest_in_world: bool,
    },
}

impl GameServer {
    /// Handle a `ClientMessage::Ranged`. Draw start/cancel track the draw state
    /// and the movement slow; fire validates and launches the projectile.
    pub(super) fn apply_ranged_command(
        &mut self,
        client_id: ClientId,
        command: RangedCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            RangedCommand::DrawStart => self.begin_ranged_draw(client_id),
            RangedCommand::DrawCancel => {
                self.clear_ranged_draw(client_id);
                Vec::new()
            }
            RangedCommand::Fire { aim_dir } => self.fire_ranged(client_id, aim_dir),
        }
    }

    /// Handle a `ClientMessage::Explosive`: throwing a held bomb, or defusing a
    /// placed charge (the defender's counterplay, in `super::defuse`).
    pub(super) fn apply_explosive_command(
        &mut self,
        client_id: ClientId,
        command: ExplosiveCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            ExplosiveCommand::Throw { aim_dir, power } => {
                self.throw_explosive(client_id, aim_dir, power)
            }
            ExplosiveCommand::Defuse { id } => self.defuse_charge(client_id, id),
        }
    }

    /// Throw the held explosive along `aim_dir` with the client's charge
    /// fraction `power`. Validates the active item is a `Thrown` explosive,
    /// consumes one, and launches a heavier-ballistics projectile
    /// (`ProjectileKind::ThrownExplosive`) from the thrower's eye with its fuse
    /// lit. The bomb deals NO contact damage: it bounces and rolls off solids
    /// and detonates in place (`resolve_explosion`) when the fuse runs out.
    /// Same gravity as an arrow, but a slower launch so it arcs and drops;
    /// `power` scales launch speed between the bomb's min and max, clamped
    /// server-side so a forged value can neither exceed the max nor undercut
    /// the min-charge floor.
    fn throw_explosive(
        &mut self,
        client_id: ClientId,
        aim_dir: Vec3Net,
        power: f32,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if client.lifecycle.is_dead() {
            return Vec::new();
        }
        // Resolve the held item and require it to be a Thrown explosive.
        let Some(explosive) = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|def| def.explosive)
        else {
            return Vec::new();
        };
        if explosive.delivery != crate::items::ExplosiveDelivery::Thrown {
            return Vec::new();
        }
        let item_id = client
            .inventory
            .active_actionbar_stack()
            .map(|s| s.item_id.clone())
            .expect("active stack resolved the explosive above");

        // Reject a non-finite or zero aim before touching inventory.
        let dir = aim_dir.normalize_or_zero();
        if dir == Vec3Net::ZERO || !dir.x.is_finite() || !dir.y.is_finite() || !dir.z.is_finite() {
            return Vec::new();
        }

        // Consume exactly one bomb; reject if none is present.
        let took = {
            let Some(client) = self.clients.get_mut(&client_id) else {
                return Vec::new();
            };
            take_items_from_inventory(&mut client.inventory, &item_id, 1)
        };
        if took == 0 {
            return Vec::new();
        }

        let eye = {
            let client = self.clients.get(&client_id).expect("client present");
            let pos = client.controller.position;
            Vec3Net::new(pos.x, pos.y + ATTACKER_EYE_HEIGHT, pos.z)
        };
        // Charge fraction -> launch speed, clamped to the legal window: a forged
        // power can neither exceed the max nor slip under the min-charge floor
        // (and a NaN collapses to the floor).
        let power = if power.is_finite() {
            power.clamp(crate::game_balance::POWDER_BOMB_MIN_THROW_FRACTION, 1.0)
        } else {
            crate::game_balance::POWDER_BOMB_MIN_THROW_FRACTION
        };
        let speed = crate::game_balance::POWDER_BOMB_MIN_THROW_SPEED_MPS
            + (crate::game_balance::POWDER_BOMB_MAX_THROW_SPEED_MPS
                - crate::game_balance::POWDER_BOMB_MIN_THROW_SPEED_MPS)
                * power;
        let velocity = dir.scale(speed);
        let id = self.next_projectile_id;
        self.next_projectile_id += 1;
        let projectile = Projectile {
            id,
            kind: ProjectileKind::ThrownExplosive(explosive.kind),
            owner: client_id,
            // The bomb's registry model (`ThrownBomb`) drives the client visual
            // (the tumbling lit bomb) and the peer impact cue.
            model: item_definition(&item_id)
                .map(|def| def.model)
                .unwrap_or(ItemModel::Bag),
            // A thrown bomb never becomes a recoverable item, but the field must
            // carry something; the bomb's own id keeps it honest.
            ammo_item: crate::items::POWDER_BOMB_ID,
            // Contact damage is unused for a thrown explosive (the blast does the
            // damage); keep it zero so a stray contact-damage path is a no-op.
            damage: 0,
            knockback_speed: 0.0,
            position: eye,
            velocity,
            spawn_tick: self.tick,
            // The fuse is lit the moment the bomb leaves the hand.
            fuse_ticks_left: explosive.fuse_ticks,
        };
        self.insert_projectile(id, projectile);
        // No cooldown / durability: a bomb is a consumable, not a weapon with a
        // reload. Consuming one is the whole cost.
        Vec::new()
    }

    /// Begin a draw: validate the held weapon is a ranged weapon and the shooter
    /// has ammo, then record the draw start tick and slow movement. No-op (and no
    /// draw started) otherwise, so a client can't slow-walk with a non-bow.
    fn begin_ranged_draw(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some((_ranged, has_ammo)) = self.held_ranged_and_ammo(client_id) else {
            return Vec::new();
        };
        if !has_ammo {
            return Vec::new();
        }
        let tick = self.tick;
        if let Some(client) = self.clients.get_mut(&client_id) {
            // Dead players can't draw.
            if client.lifecycle.is_dead() {
                return Vec::new();
            }
            client.draw_started_tick = Some(tick);
            client.run_speed_multiplier = BOW_DRAW_MOVE_MULTIPLIER;
        }
        Vec::new()
    }

    /// Clear any active draw and restore movement. Idempotent. This is the single
    /// restore path every exit funnels through (fire, cancel, item swap, death),
    /// so the move multiplier can never get stuck at the draw value.
    pub(super) fn clear_ranged_draw(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id)
            && client.draw_started_tick.take().is_some()
        {
            // Only restore if we actually cleared a draw, so this never stomps a
            // concurrently-set admin `/speed` multiplier when no draw was active.
            client.run_speed_multiplier = 1.0;
        }
    }

    /// Clear an active crossbow reload movement slow and restore movement.
    /// Idempotent: only restores when we actually had a reload slow armed, so it
    /// never stomps a concurrently-set admin `/speed` multiplier. This is the
    /// single restore path the reload slow funnels through, called from the
    /// per-tick housekeeping when the reload window elapses, and eagerly on item
    /// swap / death so a slowed player is never stuck at reload speed after
    /// dropping the crossbow.
    pub(super) fn clear_reload_slow(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id)
            && client.reload_slow_active
        {
            client.reload_slow_active = false;
            client.run_speed_multiplier = 1.0;
        }
    }

    /// Restore movement for every client whose crossbow reload window has just
    /// elapsed. Runs once per server tick; O(reloading clients), a no-op for
    /// anyone not mid-reload. Keyed off `next_ranged_tick` (the authoritative
    /// reload floor) so the slow lifts on exactly the tick the next bolt becomes
    /// available.
    pub(super) fn tick_reload_slows(&mut self) {
        let tick = self.tick;
        for client in self.clients.values_mut() {
            if client.reload_slow_active && tick >= client.next_ranged_tick {
                client.reload_slow_active = false;
                client.run_speed_multiplier = 1.0;
            }
        }
    }

    /// Release a shot. Validates weapon, ammo, cooldown, and finite aim; scales
    /// damage by the observed draw fraction (crossbow is flat); consumes one
    /// arrow; spawns the projectile from the shooter's eye. Always clears the draw
    /// state (and its movement slow) whether or not the shot fires.
    fn fire_ranged(&mut self, client_id: ClientId, aim_dir: Vec3Net) -> Vec<ServerEnvelope> {
        // Snapshot draw start before clearing (clear restores movement).
        let draw_started = self
            .clients
            .get(&client_id)
            .and_then(|c| c.draw_started_tick);
        // Firing always ends the draw + restores movement, even on a rejected
        // shot, so a spammed Fire can't leave the player permanently slowed.
        self.clear_ranged_draw(client_id);

        let Some((ranged, _)) = self.held_ranged_and_ammo(client_id) else {
            return Vec::new();
        };
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if client.lifecycle.is_dead() {
            return Vec::new();
        }
        // Cooldown / reload floor.
        if self.tick < client.next_ranged_tick {
            return Vec::new();
        }
        // Reject a non-finite or zero aim before touching ammo.
        let dir = aim_dir.normalize_or_zero();
        if dir == Vec3Net::ZERO || !dir.x.is_finite() || !dir.y.is_finite() || !dir.z.is_finite() {
            return Vec::new();
        }

        // Minimum-draw gate: a release below the firing threshold is a cancel,
        // never a shot, so a tapped button can't loose an arrow. Checked BEFORE
        // ammo/cooldown mutations so the abandoned draw costs nothing. The
        // client mirrors this and normally sends DrawCancel instead, but the
        // server enforces it off its own observed ticks.
        let draw_ticks = draw_started
            .map(|start| self.tick.saturating_sub(start))
            .unwrap_or(0);
        if !ranged.draw_fires(draw_ticks) {
            return Vec::new();
        }

        // Consume exactly one arrow; reject the shot if none is present (the draw
        // gate should have caught this, but the ammo could have been dropped
        // mid-draw). `take_items_from_inventory` returns how many it actually
        // removed, so a return of 0 means no ammo and the shot is dropped.
        let took = {
            let Some(client) = self.clients.get_mut(&client_id) else {
                return Vec::new();
            };
            take_items_from_inventory(&mut client.inventory, ranged.ammo_item, 1)
        };
        if took == 0 {
            return Vec::new();
        }

        let damage = ranged.damage_for_draw(draw_ticks);

        let (eye, model) = {
            let client = self.clients.get(&client_id).expect("client present");
            let pos = client.controller.position;
            let eye = Vec3Net::new(pos.x, pos.y + ATTACKER_EYE_HEIGHT, pos.z);
            let model = item_definition(
                client
                    .inventory
                    .active_actionbar_stack()
                    .map(|s| s.item_id.as_ref())
                    .unwrap_or(""),
            )
            .map(|def| def.model)
            .unwrap_or(ItemModel::Bow);
            (eye, model)
        };

        // Launch pace follows the hold: a barely-committed draw lobs the arrow
        // out slow and short, a full draw sends it at the profile's full speed.
        let velocity = dir.scale(ranged.speed_for_draw(draw_ticks));
        let id = self.next_projectile_id;
        self.next_projectile_id += 1;
        let projectile = Projectile {
            id,
            kind: ProjectileKind::Arrow,
            owner: client_id,
            model,
            ammo_item: ranged.ammo_item,
            damage,
            knockback_speed: ranged.knockback_speed,
            position: eye,
            velocity,
            spawn_tick: self.tick,
            fuse_ticks_left: 0,
        };
        self.insert_projectile(id, projectile);

        if let Some(client) = self.clients.get_mut(&client_id) {
            client.next_ranged_tick = self.tick + ranged.cooldown_ticks.max(1);
            // Crossbow reload movement slow: a crossbow (no draw hold, a real
            // reload) impairs movement across its whole reload window, so an
            // ambush costs mobility on the recovery. A bow's tiny post-fire floor
            // is not a reload, so it keeps full speed the instant it looses (the
            // draw slow already covered its commitment). The restore runs in the
            // per-tick housekeeping when `next_ranged_tick` elapses (and on swap /
            // death via `clear_reload_slow`).
            if ranged.draw_ticks_to_full == 0 {
                client.run_speed_multiplier = CROSSBOW_RELOAD_MOVE_MULTIPLIER;
                client.reload_slow_active = true;
            }
        }
        // The projectile appears via the replicated mirror; no fire event needed.
        // Durability: wear the ranged weapon like a melee swing on a fired shot.
        self.consume_active_tool_durability(client_id)
    }

    /// Resolve the held item into its `RangedProfile` and whether the shooter has
    /// at least one of its ammo. `None` when the active item is not a ranged
    /// weapon.
    fn held_ranged_and_ammo(
        &self,
        client_id: ClientId,
    ) -> Option<(crate::items::RangedProfile, bool)> {
        let client = self.clients.get(&client_id)?;
        let ranged = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|def| def.ranged)?;
        let has_ammo = count_items_in_inventory(&client.inventory, ranged.ammo_item) >= 1;
        Some((ranged, has_ammo))
    }

    /// Integrate every live projectile one tick: gravity, position step, and a
    /// swept hit test against blocks, players, then deployables. Hits resolve
    /// their damage and remove the projectile; a world hit optionally drops a
    /// recoverable arrow and leaves a cosmetic stuck entity for a short TTL. The
    /// max-flight cap and the stuck-TTL expiry despawn projectiles that never hit
    /// anything. Called from the ordered tick loop.
    pub(super) fn tick_projectiles(&mut self, delta_seconds: f32) -> Vec<ServerEnvelope> {
        // Expire cosmetic stuck arrows whose TTL elapsed (remove the mirror
        // entity). Collect first so the removal doesn't overlap the map borrow.
        let expired: Vec<ProjectileId> = self
            .stuck_projectiles
            .iter()
            .filter(|(_, despawn_tick)| self.tick >= **despawn_tick)
            .map(|(&id, _)| id)
            .collect();
        for id in expired {
            self.stuck_projectiles.remove(&id);
            self.remove_projectile(id);
        }

        // Keep freshly-rested arrows in the mirror's dirty set for a short window
        // so `sync_projectile_entities` re-affirms their room membership a few
        // times after client AoI subscriptions settle. Without this a rested arrow
        // never re-anchors (the loop below skips it forever), so under the 0.28
        // latch-on-(re)insert visibility model a stationary shooter can miss its
        // stick entirely. See `PROJECTILE_REST_REAFFIRM_TICKS`.
        let stuck_ttl_ticks = (PROJECTILE_STUCK_TTL_SECONDS * SERVER_TICK_RATE_HZ) as u64;
        let reaffirm: Vec<ProjectileId> = self
            .stuck_projectiles
            .iter()
            .filter_map(|(&id, &despawn_tick)| {
                let rest_tick = despawn_tick.saturating_sub(stuck_ttl_ticks);
                (self.tick.saturating_sub(rest_tick) < PROJECTILE_REST_REAFFIRM_TICKS).then_some(id)
            })
            .collect();
        for id in reaffirm {
            self.projectiles.mark_dirty(&id);
        }

        if self.projectiles.is_empty() {
            return Vec::new();
        }

        // Build the everything-solid grid ONCE this tick and reuse it for every
        // projectile (see the module perf note).
        let grid = self.spawn_collision_grid();
        let max_flight_ticks = (PROJECTILE_MAX_FLIGHT_SECONDS * SERVER_TICK_RATE_HZ) as u64;

        // Snapshot the live projectile ids so we can mutate the map inside the
        // loop (a hit removes the projectile and may spawn drops).
        let ids: Vec<ProjectileId> = self.projectiles.keys().copied().collect();
        let mut envelopes = Vec::new();

        for id in ids {
            // Skip a projectile already resting as a stuck arrow (it stays put).
            if self.stuck_projectiles.contains_key(&id) {
                continue;
            }
            let Some(projectile) = self.projectiles.get(&id).copied() else {
                continue;
            };

            // A thrown explosive lives on its own physics: burn the fuse, then
            // bounce/roll. It never enters the arrow hit logic below.
            if let ProjectileKind::ThrownExplosive(kind) = projectile.kind {
                if projectile.fuse_ticks_left <= 1 {
                    // Fuse spent: detonate in place, wherever the bomb is right
                    // now (mid-air, mid-roll, or at rest).
                    let center = projectile.position;
                    self.remove_projectile(id);
                    envelopes.extend(self.resolve_explosion(center, kind));
                    continue;
                }
                let (new_pos, new_vel) =
                    self.step_thrown_explosive(projectile, &grid, delta_seconds);
                if let Some(p) = self.projectiles.get_mut(&id) {
                    p.fuse_ticks_left -= 1;
                    p.position = new_pos;
                    p.velocity = new_vel;
                }
                continue;
            }

            // Max-flight cap: despawn a shot that never hit anything.
            if self.tick.saturating_sub(projectile.spawn_tick) >= max_flight_ticks {
                self.remove_projectile(id);
                continue;
            }

            match self.step_projectile(projectile, &grid, delta_seconds) {
                StepResult::Flying => {
                    // No hit this tick: commit the advanced pos/vel so the mirror
                    // ships the new transform for client extrapolation.
                    let (new_pos, new_vel) =
                        advance_kinematics(projectile.position, projectile.velocity, delta_seconds);
                    if let Some(p) = self.projectiles.get_mut(&id) {
                        p.position = new_pos;
                        p.velocity = new_vel;
                    }
                }
                StepResult::Hit {
                    envelopes: hit_envelopes,
                    position,
                    surface,
                    rest_in_world,
                } => {
                    envelopes.extend(hit_envelopes);
                    // Peer VFX/SFX: a small ProjectileImpact fan-out (mirrors
                    // PlayerImpact's range), excluding the shooter. The shooter
                    // never appears in this fan-out (`except`), so the owner copy
                    // below can't double-deliver.
                    envelopes.extend(self.envelopes_within_range(
                        position,
                        IMPACT_MESSAGE_RANGE_M,
                        Some(projectile.owner),
                        ServerMessage::ProjectileImpact {
                            position,
                            model: projectile.model,
                            surface,
                            owner_confirmation: false,
                        },
                    ));
                    // Own-hit confirmation: on a Player or Deployable hit send the
                    // shooter one owner-tagged copy so their client raises the
                    // crosshair hit marker (the melee attacker's confirmation).
                    // Skipped for a World rest: the shooter's client already cues
                    // that from the arrow's moving -> stuck transition. The peer
                    // fan-out excludes the owner, so this is the shooter's only
                    // copy, no dedupe needed.
                    if matches!(
                        surface,
                        ProjectileSurface::Player | ProjectileSurface::Deployable
                    ) {
                        envelopes.push(ServerEnvelope {
                            target: DeliveryTarget::Client(projectile.owner),
                            message: ServerMessage::ProjectileImpact {
                                position,
                                model: projectile.model,
                                surface,
                                owner_confirmation: true,
                            },
                        });
                    }

                    if rest_in_world {
                        self.rest_projectile_in_world(projectile, position);
                    } else {
                        // Player / deployable hit: the arrow is spent, remove it.
                        self.remove_projectile(id);
                    }
                }
            }
        }

        // Re-anchor chunk membership for the projectiles that moved (mirror does
        // the room move; here we keep nothing, the sync reads position directly).
        envelopes
    }

    /// Step one projectile a single tick and test the segment it traversed for a
    /// hit against blocks, players, then deployables. Returns `Flying` if it hit
    /// nothing, or `Hit` with the resolved damage envelopes.
    fn step_projectile(
        &mut self,
        projectile: Projectile,
        grid: &BlockGrid,
        delta_seconds: f32,
    ) -> StepResult {
        let origin = projectile.position;
        let (end, _new_vel) = advance_kinematics(origin, projectile.velocity, delta_seconds);
        let segment = end.minus(origin);
        let length = segment.length_squared().sqrt();
        if length <= f32::EPSILON {
            return StepResult::Flying;
        }
        let inv = length.recip();
        let dir = Vec3Net::new(segment.x * inv, segment.y * inv, segment.z * inv);

        // Gather the three candidate hits with their entry distances along the
        // segment, then resolve whichever is nearest. Order matters: a player
        // standing behind a wall must not be hit if the wall intercepts first,
        // and a deployable collider between shooter and target stops the arrow
        // before the target. `nearest_block_hit` covers the everything-solid grid
        // (world + resource + deployable colliders); `nearest_deployable_hit` is
        // what attributes the deployable *damage* when the arrow's first solid is
        // a structure. Since deployable colliders are already in the grid, the
        // block distance and the deployable distance agree when the nearest solid
        // is a deployable; comparing them lets a plain world/terrain block win a
        // tie and read as a world rest.
        let skip_shooter =
            self.tick.saturating_sub(projectile.spawn_tick) < PROJECTILE_SELF_HIT_GRACE_TICKS;
        let player_hit =
            self.nearest_player_hit(projectile.owner, skip_shooter, origin, dir, length);
        // The open ground counts as a world solid: an arrow that arcs into the
        // terrain lodges at the surface exactly like a tree hit (owner report:
        // ground shots used to sail through the floor and despawn unrecovered).
        let block_dist = merge_nearest(
            nearest_block_hit(grid, origin, dir, length),
            ground_plane_hit(origin, dir, length),
        );
        let deployable_hit = self.nearest_deployable_hit(origin, dir, length);

        // The nearest solid (world block or deployable) that would stop the arrow.
        let solid_dist = match (block_dist, deployable_hit) {
            (Some(b), Some((_, d))) => Some(b.min(d)),
            (Some(b), None) => Some(b),
            (None, Some((_, d))) => Some(d),
            (None, None) => None,
        };

        // If a solid is nearer than any player, the arrow hits the solid first.
        let player_first = match (player_hit, solid_dist) {
            (Some((_, pd)), Some(sd)) => pd <= sd,
            (Some(_), None) => true,
            _ => false,
        };
        if player_first && let Some((target_id, _)) = player_hit {
            return self.resolve_player_hit(projectile, target_id);
        }

        // Otherwise the nearest solid stops the arrow: a deployable takes ranged
        // damage; a plain world block is a rest point (recoverable arrow + stuck).
        match (block_dist, deployable_hit) {
            (Some(bd), Some((dep_id, dep_dist))) => {
                if dep_dist <= bd {
                    self.resolve_deployable_hit(projectile, dep_id, origin, dir, dep_dist)
                } else {
                    self.rest_world_hit(origin, dir, bd)
                }
            }
            (Some(bd), None) => self.rest_world_hit(origin, dir, bd),
            (None, Some((dep_id, dep_dist))) => {
                self.resolve_deployable_hit(projectile, dep_id, origin, dir, dep_dist)
            }
            (None, None) => StepResult::Flying,
        }
    }

    /// Find the nearest player whose body box the segment enters, as
    /// `(target_id, entry_distance)`, or `None`.
    fn nearest_player_hit(
        &self,
        owner: ClientId,
        skip_shooter: bool,
        origin: Vec3Net,
        dir: Vec3Net,
        length: f32,
    ) -> Option<(ClientId, f32)> {
        let mut best: Option<(ClientId, f32)> = None;
        for client in self.clients.values() {
            if client.lifecycle.is_dead() {
                continue;
            }
            if skip_shooter && client.client_id == owner {
                continue;
            }
            // A live projectile can hit its own shooter after the grace window
            // (a lobbed arrow arcing back), but not on the frames right after
            // launch when it is inside the shooter's own collider column.
            let feet = client.controller.position;
            let sleeping = !client.online;
            if let Some(entry) = crate::combat::player_body_ray_entry(origin, dir, feet, sleeping)
                && entry <= length
                && best.map(|(_, d)| entry < d).unwrap_or(true)
            {
                best = Some((client.client_id, entry));
            }
        }
        best
    }

    /// Resolve a projectile hit on a player through the shared post-hit tail.
    fn resolve_player_hit(&mut self, projectile: Projectile, target_id: ClientId) -> StepResult {
        let Some(target) = self.clients.get(&target_id) else {
            return StepResult::Flying;
        };
        let target_pos = target.controller.position;
        let target_armor = target.protection.for_kind(DamageKind::Projectile);
        let attacker_name = self
            .clients
            .get(&projectile.owner)
            .map(|c| c.name.clone())
            .unwrap_or_default();

        // Projectiles carry no armor pierce (only the mace does today); still
        // route through the pierce helper so a future piercing bolt just works.
        let effective_armor = effective_armor_after_pierce(target_armor, 0);
        let damage_dealt = damage_after_armor(projectile.damage, effective_armor);

        let hit_anchor = Vec3Net::new(
            target_pos.x,
            target_pos.y + crate::game_balance::COMBAT_TARGET_CHEST_HEIGHT,
            target_pos.z,
        );

        if damage_dealt == 0 {
            // Fully absorbed: the arrow is still spent, but no HP change or peer
            // impact (mirrors the melee "fully blocked" path).
            return StepResult::Hit {
                envelopes: Vec::new(),
                position: hit_anchor,
                surface: ProjectileSurface::Player,
                rest_in_world: false,
            };
        }

        let knockback = projectile_knockback(projectile.velocity, projectile.knockback_speed);
        let envelopes = self.apply_player_damage(PlayerDamageHit {
            target_id,
            attacker_id: Some(projectile.owner),
            attacker_name: &attacker_name,
            damage_dealt,
            kind: DamageKind::Projectile,
            knockback,
            // The projectile's own ProjectileImpact fan-out drives the peer VFX,
            // so the melee-style PlayerImpact fan-out is suppressed here (None).
            impact: None,
        });

        StepResult::Hit {
            envelopes,
            position: hit_anchor,
            surface: ProjectileSurface::Player,
            rest_in_world: false,
        }
    }

    /// Find the nearest deployable whose resolved collider blocks the segment
    /// enters, as `(id, entry_distance)`.
    fn nearest_deployable_hit(
        &self,
        origin: Vec3Net,
        dir: Vec3Net,
        length: f32,
    ) -> Option<(crate::protocol::DeployedEntityId, f32)> {
        let mut best: Option<(crate::protocol::DeployedEntityId, f32)> = None;
        for entity in self.deployed_entities.values() {
            for block in entity.resolved_collider_blocks() {
                if let Some(entry) = ray_aabb_entry(origin, dir, block)
                    && entry >= 0.0
                    && entry <= length
                    && best.map(|(_, d)| entry < d).unwrap_or(true)
                {
                    best = Some((entity.id, entry));
                }
            }
        }
        best
    }

    /// Resolve a projectile hit on a deployable: apply the sticks-tier ranged
    /// effectiveness rule (weapons are not raid tools) and remove the arrow.
    fn resolve_deployable_hit(
        &mut self,
        projectile: Projectile,
        deployable_id: crate::protocol::DeployedEntityId,
        origin: Vec3Net,
        dir: Vec3Net,
        entry: f32,
    ) -> StepResult {
        // Modest, sticks-tier-only damage: a bow never raids a base. One rule,
        // one constant, applied to every deployable kind uniformly.
        let damage = projectile
            .damage
            .saturating_mul(PROJECTILE_DEPLOYABLE_EFFECTIVENESS_PCT)
            / 100;
        if let Some(entity) = self.deployed_entity_mut(deployable_id) {
            entity.health = entity.health.saturating_sub(damage);
            let dead = entity.health == 0;
            if dead {
                self.destroy_deployed_entity(deployable_id);
            }
        }
        let hit_point = origin.plus(dir.scale(entry));
        StepResult::Hit {
            envelopes: Vec::new(),
            position: hit_point,
            surface: ProjectileSurface::Deployable,
            rest_in_world: false,
        }
    }

    /// Advance a thrown explosive one tick: integrate under gravity, and on
    /// meeting a solid (world block, the ground plane, or a deployable collider)
    /// BOUNCE off it instead of stopping: the normal component of the velocity
    /// reflects scaled by the restitution, the tangential component keeps
    /// rolling scaled by the bounce friction. A slow ground-ish contact comes to
    /// rest (zero velocity) so the roll tail doesn't jitter forever. The bomb
    /// ignores players entirely (no contact damage; the blast does the damage)
    /// and never converts into a deployable: it stays a projectile until its
    /// fuse detonates it in place.
    fn step_thrown_explosive(
        &self,
        projectile: Projectile,
        grid: &BlockGrid,
        delta_seconds: f32,
    ) -> (Vec3Net, Vec3Net) {
        use crate::game_balance::{
            POWDER_BOMB_BALL_RADIUS_M, POWDER_BOMB_BOUNCE_FRICTION, POWDER_BOMB_REST_SPEED_MPS,
            POWDER_BOMB_RESTITUTION,
        };
        let origin = projectile.position;
        // At rest: stay put (skipping gravity so it doesn't sink into the
        // surface it settled on) and just burn the fuse.
        if projectile.velocity == Vec3Net::ZERO {
            return (origin, projectile.velocity);
        }
        let (end, new_vel) = advance_kinematics(origin, projectile.velocity, delta_seconds);
        let segment = end.minus(origin);
        let length = segment.length_squared().sqrt();
        if length <= f32::EPSILON {
            return (origin, new_vel);
        }
        let inv = length.recip();
        let dir = Vec3Net::new(segment.x * inv, segment.y * inv, segment.z * inv);

        // Nearest solid along this tick's segment, with its surface normal:
        // world blocks, the open ground plane (+Y), and deployable colliders.
        // The bomb is a SPHERE of the ball's radius, not a point: solids are
        // Minkowski-inflated by the radius (and the ground plane raised by
        // it), so the position being swept is the ball CENTER and only the
        // ball collides, never the fuse cap. That is what makes the roll
        // smooth: contact always happens one ball-radius from the center.
        let ball = POWDER_BOMB_BALL_RADIUS_M;
        let mut nearest: Option<(f32, Vec3Net)> =
            nearest_block_hit_normal(grid, origin, dir, length, ball);
        let ball_bottom = Vec3Net::new(origin.x, origin.y - ball, origin.z);
        if let Some(dist) = ground_plane_hit(ball_bottom, dir, length)
            && nearest.map(|(d, _)| dist < d).unwrap_or(true)
        {
            nearest = Some((dist, Vec3Net::new(0.0, 1.0, 0.0)));
        }
        if let Some((dist, normal)) = self.nearest_deployable_hit_normal(origin, dir, length, ball)
            && nearest.map(|(d, _)| dist < d).unwrap_or(true)
        {
            nearest = Some((dist, normal));
        }

        let Some((dist, normal)) = nearest else {
            return (end, new_vel);
        };

        // Contact point, nudged off the surface along its normal so next tick's
        // segment starts outside the solid instead of re-entering it.
        let contact = origin
            .plus(dir.scale((dist - 1e-3).max(0.0)))
            .plus(normal.scale(0.01));

        // Split the velocity at contact into its normal and tangential parts:
        // reflect the normal part scaled by restitution, damp the tangential
        // part by the bounce friction. Axis-aligned normals only (everything
        // solid is an AABB), which reads fine at this scale.
        let vn = new_vel.dot(normal);
        let normal_part = normal.scale(vn);
        let tangent_part = new_vel.minus(normal_part);
        let bounced = tangent_part
            .scale(POWDER_BOMB_BOUNCE_FRICTION)
            .plus(normal.scale(-vn * POWDER_BOMB_RESTITUTION));

        // A slow contact on a ground-ish surface rests the bomb; on a wall it
        // just drops (gravity re-accelerates it next tick).
        let speed = bounced.length_squared().sqrt();
        if normal.y > 0.7 && speed < POWDER_BOMB_REST_SPEED_MPS {
            return (contact, Vec3Net::ZERO);
        }
        (contact, bounced)
    }

    /// Like [`Self::nearest_deployable_hit`] but returning the entry-face normal
    /// of the nearest deployable collider instead of the entity id, for the
    /// thrown-bomb bounce response. `inflate` Minkowski-expands each collider
    /// by the bomb's ball radius so the swept point is the ball center.
    fn nearest_deployable_hit_normal(
        &self,
        origin: Vec3Net,
        dir: Vec3Net,
        length: f32,
        inflate: f32,
    ) -> Option<(f32, Vec3Net)> {
        let mut best: Option<(f32, Vec3Net)> = None;
        for entity in self.deployed_entities.values() {
            for block in entity.resolved_collider_blocks() {
                if let Some((entry, normal)) = ray_aabb_entry_normal(origin, dir, block, inflate)
                    && entry >= 0.0
                    && entry <= length
                    && best.map(|(d, _)| entry < d).unwrap_or(true)
                {
                    best = Some((entry, normal));
                }
            }
        }
        best
    }

    /// A pure world hit: the arrow comes to rest against terrain / a perimeter
    /// wall. Returns the rest point so the caller can drop a recoverable arrow and
    /// leave a cosmetic stuck entity.
    fn rest_world_hit(&self, origin: Vec3Net, dir: Vec3Net, block_dist: f32) -> StepResult {
        let rest = origin.plus(dir.scale(block_dist));
        StepResult::Hit {
            envelopes: Vec::new(),
            position: rest,
            surface: ProjectileSurface::World,
            rest_in_world: true,
        }
    }

    /// Handle a projectile that came to rest against the world: snap it to rest
    /// and park it as a STUCK arrow for the TTL, during which any player can
    /// walk up and pull it back out with E ([`Self::apply_recover_projectile`]).
    /// Every world rest sticks: an earlier design broke a share of arrows at
    /// rest (instant despawn), which read as the arrow phasing through the tree
    /// with no impact at all (owner report); the ammo economy is carried by the
    /// TTL (uncollected arrows are lost) and by hits consuming the arrow.
    pub(super) fn rest_projectile_in_world(&mut self, projectile: Projectile, rest: Vec3Net) {
        // Snap the projectile to rest: near-zero speed (below any stuck
        // threshold), but keep the final flight DIRECTION encoded as an epsilon
        // velocity so clients orient the stuck arrow along the shot that
        // planted it. A true zero left late-arriving clients with no flight
        // direction at all, and their stuck arrows pointed straight up.
        let dir = projectile.velocity.normalize_or_zero();
        let rest_velocity = if dir == Vec3Net::ZERO {
            // Degenerate flight (should not happen): point it nose-down.
            Vec3Net::new(0.0, -PROJECTILE_REST_DIR_EPSILON, 0.0)
        } else {
            dir.scale(PROJECTILE_REST_DIR_EPSILON)
        };
        if let Some(p) = self.projectiles.get_mut(&projectile.id) {
            p.position = rest;
            p.velocity = rest_velocity;
        }
        let despawn_tick = self.tick + (PROJECTILE_STUCK_TTL_SECONDS * SERVER_TICK_RATE_HZ) as u64;
        self.stuck_projectiles.insert(projectile.id, despawn_tick);
    }

    /// Handle an `InventoryCommand::RecoverProjectile`: pull a stuck (at-rest)
    /// arrow back into the player's bag. Only a projectile parked in
    /// `stuck_projectiles` is recoverable (an in-flight arrow can't be snatched
    /// out of the air); reach uses the same lenient distance-only check as a
    /// dropped-item pickup. A full bag leaves the arrow stuck (it can be
    /// retried until the TTL expires).
    pub(super) fn apply_recover_projectile(
        &mut self,
        client_id: ClientId,
        projectile_id: ProjectileId,
    ) -> Vec<ServerEnvelope> {
        use super::inventory::accepted_inventory_quantity;
        use super::toasts::{inventory_full_toast_envelopes, item_acquired_toast_envelopes};
        use crate::items::within_pickup_reach;

        if !self.stuck_projectiles.contains_key(&projectile_id) {
            return Vec::new();
        }
        let Some(projectile) = self.projectiles.get(&projectile_id).copied() else {
            return Vec::new();
        };
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if client.lifecycle.is_dead() {
            return Vec::new();
        }
        if !within_pickup_reach(
            super::movement::player_eye_position(client.controller.position),
            projectile.position,
            crate::game_balance::PICKUP_SERVER_REACH_SLACK_M,
        ) {
            return Vec::new();
        }
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let stack = crate::protocol::ItemStack::new(projectile.ammo_item, 1);
        let ammo_id = stack.item_id.clone();
        let accepted = accepted_inventory_quantity(&mut client.inventory, stack);
        if accepted == 0 {
            return inventory_full_toast_envelopes(client_id);
        }
        self.remove_projectile(projectile_id);
        item_acquired_toast_envelopes(client_id, &ammo_id, accepted)
    }

    // ---- mandatory-entry map helpers (mirror-sync dirty tracking) ----

    /// Insert (or replace) a projectile and flag it for the next mirror sync.
    pub(super) fn insert_projectile(&mut self, id: ProjectileId, projectile: Projectile) {
        self.projectiles.insert(id, projectile);
    }

    /// Remove a projectile (and its stuck-arrow bookkeeping), flagging the mirror
    /// to despawn the replicated entity.
    pub(super) fn remove_projectile(&mut self, id: ProjectileId) -> Option<Projectile> {
        self.stuck_projectiles.remove(&id);
        self.projectiles.remove(&id)
    }

    /// Read a projectile as the wire-shape view the mirror needs.
    pub fn projectile_view(
        &self,
        id: ProjectileId,
    ) -> Option<super::projectile_ecs::ProjectileView> {
        self.projectiles
            .get(&id)
            .map(|p| super::projectile_ecs::ProjectileView {
                id: p.id,
                model: p.model,
                owner: p.owner,
                position: p.position,
                velocity: p.velocity,
            })
    }

    /// Drain the accumulated mirror-sync deltas: `(dirty ids, removed ids)`.
    pub fn drain_projectile_sync(&mut self) -> (Vec<ProjectileId>, Vec<ProjectileId>) {
        self.projectiles.drain_sync()
    }
}

/// Integrate one projectile a tick: gravity onto velocity, then position by the
/// updated velocity. A simple semi-implicit Euler step; at 20 Hz with a gamey
/// gravity this is stable and the arc reads well.
fn advance_kinematics(
    position: Vec3Net,
    velocity: Vec3Net,
    delta_seconds: f32,
) -> (Vec3Net, Vec3Net) {
    let new_vel = Vec3Net::new(
        velocity.x,
        velocity.y + PROJECTILE_GRAVITY * delta_seconds,
        velocity.z,
    );
    let new_pos = position.plus(new_vel.scale(delta_seconds));
    (new_pos, new_vel)
}

/// Entry distance along the segment where it crosses the world floor plane
/// (y = 0, the flat terrain every block-free column bottoms out on), or `None`
/// when the segment stays above it. Only a downward crossing counts: a
/// projectile somehow below the floor is left to the max-flight despawn rather
/// than snapped back up.
fn ground_plane_hit(origin: Vec3Net, dir: Vec3Net, length: f32) -> Option<f32> {
    if dir.y >= -1e-6 || origin.y <= 0.0 {
        return None;
    }
    let dist = origin.y / -dir.y;
    (dist <= length).then_some(dist)
}

/// The nearer of two optional entry distances.
fn merge_nearest(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Nearest world-block entry distance along the segment, or `None`. Uses the
/// swept-candidate query exactly like `line_of_sight_clear`.
fn nearest_block_hit(grid: &BlockGrid, origin: Vec3Net, dir: Vec3Net, length: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for index in grid.candidates_for_swept(origin, dir.x * length, dir.z * length) {
        let block = grid.block(index);
        if let Some(entry) = ray_aabb_entry(origin, dir, block)
            && entry >= 0.0
            && entry <= length
            && best.map(|d| entry < d).unwrap_or(true)
        {
            best = Some(entry);
        }
    }
    best
}

/// Nearest world-block entry along the segment WITH the entry-face normal, for
/// the thrown-bomb bounce. Mirrors [`nearest_block_hit`] but keeps the normal.
/// `inflate` Minkowski-expands each block by the bomb's ball radius so the
/// swept point is the ball center.
fn nearest_block_hit_normal(
    grid: &BlockGrid,
    origin: Vec3Net,
    dir: Vec3Net,
    length: f32,
    inflate: f32,
) -> Option<(f32, Vec3Net)> {
    let mut best: Option<(f32, Vec3Net)> = None;
    for index in grid.candidates_for_swept(origin, dir.x * length, dir.z * length) {
        let block = grid.block(index);
        if let Some((entry, normal)) = ray_aabb_entry_normal(origin, dir, block, inflate)
            && entry >= 0.0
            && entry <= length
            && best.map(|(d, _)| entry < d).unwrap_or(true)
        {
            best = Some((entry, normal));
        }
    }
    best
}

/// Slab-method ray/AABB entry like [`ray_aabb_entry`], additionally returning
/// the outward normal of the face the ray entered through (axis-aligned: the
/// slab whose entry time is the latest, facing against the ray). Used by the
/// thrown-bomb bounce, which needs a reflection plane, not just a distance.
/// `inflate` expands the box uniformly (Minkowski sum with the bomb's ball),
/// approximating a sphere sweep well enough at this scale; pass `0.0` for a
/// plain point ray.
fn ray_aabb_entry_normal(
    origin: Vec3Net,
    direction: Vec3Net,
    block: crate::world::WorldBlock,
    inflate: f32,
) -> Option<(f32, Vec3Net)> {
    let pad = Vec3Net::new(inflate, inflate, inflate);
    let min = block.min().minus(pad);
    let max = block.max().plus(pad);
    let mut t_near: f32 = f32::NEG_INFINITY;
    let mut t_far: f32 = f32::INFINITY;
    let mut entry_axis = 0usize;
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
        if t1 > t_near {
            t_near = t_near.max(t1);
            entry_axis = axis;
        }
        t_far = t_far.min(t2);
        if t_near > t_far {
            return None;
        }
    }
    if t_near < 0.0 || !t_near.is_finite() {
        return None;
    }
    let component = match entry_axis {
        0 => direction.x,
        1 => direction.y,
        _ => direction.z,
    };
    let sign = if component > 0.0 { -1.0 } else { 1.0 };
    let normal = match entry_axis {
        0 => Vec3Net::new(sign, 0.0, 0.0),
        1 => Vec3Net::new(0.0, sign, 0.0),
        _ => Vec3Net::new(0.0, 0.0, sign),
    };
    Some((t_near, normal))
}

/// Knockback impulse for a projectile hit: shove along the projectile's
/// horizontal travel direction, with the shared vertical pop fraction. Falls
/// back to a straight-up pop if the shot is purely vertical.
fn projectile_knockback(velocity: Vec3Net, speed: f32) -> Vec3Net {
    use crate::game_balance::COMBAT_KNOCKBACK_VERTICAL_FRACTION;
    let horizontal_sq = velocity.x * velocity.x + velocity.z * velocity.z;
    if horizontal_sq <= f32::EPSILON {
        return Vec3Net::new(0.0, speed * COMBAT_KNOCKBACK_VERTICAL_FRACTION, 0.0);
    }
    let inv = horizontal_sq.sqrt().recip();
    Vec3Net::new(
        velocity.x * inv * speed,
        speed * COMBAT_KNOCKBACK_VERTICAL_FRACTION,
        velocity.z * inv * speed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gravity_pulls_a_flat_shot_downward() {
        // A flat shot along -Z drops after a step (gravity acts on Y).
        let (pos, vel) = advance_kinematics(
            Vec3Net::new(0.0, 2.0, 0.0),
            Vec3Net::new(0.0, 0.0, -35.0),
            0.05,
        );
        assert!(pos.z < 0.0, "moved forward along -Z");
        assert!(vel.y < 0.0, "gained downward velocity");
        assert!(pos.y < 2.0, "dropped below the launch height");
    }

    #[test]
    fn ballistic_arc_hits_a_known_point() {
        // Integrate a shot and confirm the arc lands within tolerance of the
        // closed-form projectile position after N steps. Launch flat along -Z at
        // 20 m/s from y=10; after 1 s of 20 Hz steps the closed-form drop is
        // ~0.5*g*t^2 (semi-implicit Euler overshoots slightly, so allow slack).
        let dt = 1.0 / SERVER_TICK_RATE_HZ;
        let mut pos = Vec3Net::new(0.0, 10.0, 0.0);
        let mut vel = Vec3Net::new(0.0, 0.0, -20.0);
        for _ in 0..(SERVER_TICK_RATE_HZ as usize) {
            let (p, v) = advance_kinematics(pos, vel, dt);
            pos = p;
            vel = v;
        }
        // Forward travel ~= 20 m/s * 1 s = 20 m along -Z.
        assert!((pos.z - (-20.0)).abs() < 0.5, "forward travel {}", pos.z);
        // Drop is around 0.5*12*1 = 6 m from y=10 => ~4; semi-implicit Euler
        // drops a touch more, so allow a metre of slack.
        assert!(pos.y < 5.0 && pos.y > 3.0, "drop landed at y={}", pos.y);
    }

    #[test]
    fn projectile_knockback_pushes_along_travel() {
        // A shot travelling along -Z shoves the target along -Z with an upward pop.
        let kb = projectile_knockback(Vec3Net::new(0.0, 0.0, -35.0), 2.0);
        assert!(kb.z < 0.0, "knockback carries the arrow's -Z travel");
        assert!(kb.y > 0.0, "knockback has an upward pop");
    }
}
