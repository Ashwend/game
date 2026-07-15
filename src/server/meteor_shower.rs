//! Server-side meteor shower event engine: scheduling, clearance-aware site
//! selection, the reliable announce, impact resolution, and crater cleanup.
//!
//! The server owns everything about the meteor. Clients render a deterministic
//! sky show from the single announce (see `crate::world::meteor_shower` and
//! `docs/meteor_shower.md`); the server never streams the fireball.
//!
//! ## Real-time schedule, not day/night time
//!
//! The scheduler counts in real server ticks (`SERVER_TICK_RATE_HZ`), converting
//! the 2 to 4 in-game-day interval through `REAL_SECONDS_PER_DAY` at cycle
//! multiplier 1. This is deliberate: the admin `/time-speed` cheat accelerates
//! only the day/night visual cycle (`world_time.multiplier`), NOT the wall
//! clock, so spinning the sun does not pull meteors closer together. Meteors are
//! a real-time event.
//!
//! ## Not persisted
//!
//! `MeteorShowerState` is transient. On world load the scheduler rolls a fresh next
//! event; an in-flight event does not survive a restart. This keeps the save
//! format untouched (no version bump) and matches the throwaway nature of a live
//! world event, a server restart mid-meteor simply reschedules.

use crate::{
    combat::DamageKind,
    game_balance::{
        METEOR_SHOWER_BUILDING_CLEARANCE_M, METEOR_SHOWER_CRATER_NODE_COUNT_MAX,
        METEOR_SHOWER_CRATER_NODE_COUNT_MIN, METEOR_SHOWER_CRATER_NODE_SPACING_M,
        METEOR_SHOWER_DESPAWN_SECONDS, METEOR_SHOWER_IMPACT_PLAYER_DAMAGE,
        METEOR_SHOWER_IMPACT_RADIUS_M, METEOR_SHOWER_INTERVAL_DAYS_MAX,
        METEOR_SHOWER_INTERVAL_DAYS_MIN, METEOR_SHOWER_SITE_BOUNDS_MARGIN_M,
        METEOR_SHOWER_SITE_CANDIDATES, METEOR_SHOWER_SITE_MIN_CENTER_DISTANCE_FRACTION,
        METEOR_SHOWER_WARNING_SECONDS,
    },
    protocol::{ClientId, ResourceNodeId, SERVER_TICK_RATE_HZ, ServerMessage, Vec3Net},
    resource_nodes::{METEORITE_NODE_ID, spawn_resource_node},
    world::{NodeKind, PlayableBounds, WorldResourceNodeSpawn, splitmix64},
    world_time::REAL_SECONDS_PER_DAY,
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope,
    combat::{PlayerDamageHit, knockback_impulse},
};

/// A live meteor shower event. Present from the announce (at T minus the warning
/// window) through impact and until the crater/crater cluster despawns.
#[derive(Debug, Clone)]
pub(super) struct ActiveMeteorShower {
    /// Ground-zero position (y is floor level). The wire announce, the crater
    /// visual, and the impact resolution all key on this.
    pub(super) impact_position: Vec3Net,
    /// Server tick the meteor strikes.
    pub(super) impact_tick: u64,
    /// Seeds the fireball's approach azimuth on the client.
    pub(super) trajectory_seed: u64,
    /// Whether the impact has already resolved (blast applied, crater nodes spawned).
    /// The tick loop resolves exactly once when the clock reaches `impact_tick`.
    pub(super) resolved: bool,
    /// Server tick the crater + crater-node cluster despawn and the event cleans up.
    pub(super) despawn_tick: u64,
    /// Crater node ids spawned by the impact, force-despawned at `despawn_tick`
    /// if still unmined. A player mining one out drops it from the live map via
    /// the normal gather path; cleanup only touches ids still present.
    pub(super) crater_nodes: Vec<ResourceNodeId>,
}

/// Server-owned meteor shower scheduler + active event. Not persisted; rolled fresh
/// on world load.
#[derive(Debug, Clone)]
pub(super) struct MeteorShowerState {
    /// Real server tick the next event's *announce* fires. `None` means an event
    /// is currently active (a new next-event tick is rolled at cleanup).
    pub(super) next_announce_tick: Option<u64>,
    /// The live event, if one is active (announce through crater despawn).
    pub(super) active: Option<ActiveMeteorShower>,
}

impl MeteorShowerState {
    /// Fresh scheduler state at world start: no active event, the first event
    /// rolled 2 to 4 in-game days out from `now_tick` using `world_seed` as the
    /// deterministic stream base.
    pub(super) fn new(now_tick: u64, world_seed: u64) -> Self {
        Self {
            next_announce_tick: Some(roll_next_announce_tick(now_tick, world_seed, 1)),
            active: None,
        }
    }
}

/// In-game-day interval converted to real server ticks. One in-game day is
/// `REAL_SECONDS_PER_DAY` real seconds at multiplier 1, and the announce fires
/// `METEOR_SHOWER_WARNING_SECONDS` before impact, so the *announce* interval is the
/// impact interval minus the warning window (clamped so it never underflows on a
/// short min interval).
fn roll_next_announce_tick(now_tick: u64, world_seed: u64, event_index: u64) -> u64 {
    // A dedicated deterministic stream salted off the seed and the event index,
    // the same splitmix64 idiom the ruins scatter and regrow scheduler use.
    let state = splitmix64(world_seed ^ 0x00E3_1B01_FA11_0000 ^ event_index.wrapping_mul(0x9E37));
    let unit = ((state >> 40) as f32) / ((1u64 << 24) as f32);
    let days = METEOR_SHOWER_INTERVAL_DAYS_MIN
        + (METEOR_SHOWER_INTERVAL_DAYS_MAX - METEOR_SHOWER_INTERVAL_DAYS_MIN) * unit;
    let impact_seconds = days * REAL_SECONDS_PER_DAY;
    // Announce lands `warning` before impact; never schedule the announce in the
    // past for a tiny interval.
    let announce_seconds = (impact_seconds - METEOR_SHOWER_WARNING_SECONDS).max(0.0);
    now_tick.saturating_add((announce_seconds * SERVER_TICK_RATE_HZ) as u64)
}

impl GameServer {
    /// Advance the meteor shower event engine one tick. Ordered like the other tick
    /// subsystems: schedule -> announce -> impact -> cleanup. Returns the
    /// envelopes to fan out (the reliable announce broadcast, per-node depletion
    /// broadcasts, near-impact blast consequences).
    pub(super) fn tick_world_events(&mut self) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();

        // 1. Scheduler: has the next announce come due?
        if let Some(announce_tick) = self.meteor_shower.next_announce_tick
            && self.tick >= announce_tick
        {
            envelopes.extend(self.begin_meteor_shower());
        }

        // 2. Impact: resolve exactly once when the clock reaches impact_tick.
        let impact_due = self
            .meteor_shower
            .active
            .as_ref()
            .is_some_and(|event| !event.resolved && self.tick >= event.impact_tick);
        if impact_due {
            envelopes.extend(self.resolve_meteor_shower_impact());
        }

        // 3. Cleanup: force-despawn any unmined crater nodes and clear the event when
        //    the crater window closes, then roll the next event.
        let despawn_due = self
            .meteor_shower
            .active
            .as_ref()
            .is_some_and(|event| event.resolved && self.tick >= event.despawn_tick);
        if despawn_due {
            envelopes.extend(self.cleanup_meteor_shower());
        }

        envelopes
    }

    /// `/meteor_shower [warning_seconds]` (admin, default 30): force an immediate
    /// meteor shower schedule for testing. Clears any in-flight event and begins a
    /// fresh one whose impact lands `warning_seconds` from now, so a tester does
    /// not have to wait the 2-to-4-in-game-day interval. Returns the announce
    /// broadcast plus a success toast to the issuer.
    pub(super) fn command_meteor_shower(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return vec![meteor_shower_toast(
                client_id,
                crate::protocol::ToastKind::Warning,
                "admin only",
            )];
        }
        let mut warning_seconds = 30.0_f32;
        if let Some(arg) = args.first() {
            match arg.parse::<f32>() {
                Ok(value) if value.is_finite() && value > 0.0 => warning_seconds = value,
                _ => {
                    return vec![meteor_shower_toast(
                        client_id,
                        crate::protocol::ToastKind::Warning,
                        "usage: /meteor_shower [warning_seconds], e.g. /meteor_shower 30",
                    )];
                }
            }
        }
        // Clear any live event so the forced one is clean.
        self.meteor_shower.active = None;
        let mut envelopes = self.begin_meteor_shower_with_warning(warning_seconds);
        envelopes.push(meteor_shower_toast(
            client_id,
            crate::protocol::ToastKind::Success,
            format!("meteor shower scheduled: impact in {warning_seconds:.0}s"),
        ));
        envelopes
    }

    /// `/meteor_shower-here [warning_seconds]` (admin, default 8): force a meteor shower
    /// whose impact lands at the CALLER'S CURRENT POSITION, so an admin can drop
    /// one exactly where they stand to watch it. Bypasses `select_meteor_shower_site`
    /// and its building-clearance check entirely: by design this can land on the
    /// caller and on buildings (the point is to place it precisely). Clears any
    /// in-flight event first, then reuses the shared announce/impact path.
    pub(super) fn command_meteor_shower_here(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return vec![meteor_shower_toast(
                client_id,
                crate::protocol::ToastKind::Warning,
                "admin only",
            )];
        }
        // Ground-zero at the caller's feet (y is floor level for the site).
        let here = Vec3Net::new(
            client.controller.position.x,
            0.0,
            client.controller.position.z,
        );

        let mut warning_seconds = 8.0_f32;
        if let Some(arg) = args.first() {
            match arg.parse::<f32>() {
                Ok(value) if value.is_finite() && value > 0.0 => warning_seconds = value,
                _ => {
                    return vec![meteor_shower_toast(
                        client_id,
                        crate::protocol::ToastKind::Warning,
                        "usage: /meteor_shower-here [warning_seconds], e.g. /meteor_shower-here 8",
                    )];
                }
            }
        }
        // Clear any live event so the forced one is clean.
        self.meteor_shower.active = None;
        let mut envelopes = self.begin_meteor_shower_at(here, warning_seconds);
        envelopes.push(meteor_shower_toast(
            client_id,
            crate::protocol::ToastKind::Success,
            format!("meteor shower on your position: impact in {warning_seconds:.0}s"),
        ));
        envelopes
    }

    /// Select an impact site and broadcast the announce. Called once when the
    /// scheduled announce tick arrives.
    fn begin_meteor_shower(&mut self) -> Vec<ServerEnvelope> {
        self.begin_meteor_shower_with_warning(METEOR_SHOWER_WARNING_SECONDS)
    }

    /// Shared announce path with an explicit warning window (real seconds until
    /// impact). The routine scheduler uses the balance constant; the admin
    /// command passes its own. Sites the impact via `select_meteor_shower_site`.
    fn begin_meteor_shower_with_warning(&mut self, warning_seconds: f32) -> Vec<ServerEnvelope> {
        // Roll a trajectory seed from the world seed + the impact tick so it is
        // reproducible for the same event but distinct across events.
        let impact_tick = self
            .tick
            .saturating_add((warning_seconds.max(0.0) * SERVER_TICK_RATE_HZ) as u64);
        let trajectory_seed =
            splitmix64(self.chunk_manager.world_seed() ^ impact_tick ^ 0x00FA_11ED_57A8_0000);

        let impact_position = self.select_meteor_shower_site(trajectory_seed);
        self.begin_meteor_shower_event(
            impact_position,
            impact_tick,
            trajectory_seed,
            warning_seconds,
        )
    }

    /// Announce a meteor shower at an EXPLICIT impact position, bypassing siting and
    /// the building-clearance check. Used only by `/meteor_shower-here`, which drops
    /// the meteor exactly where the admin stands (so it can, by design, land on
    /// the caller and on buildings).
    fn begin_meteor_shower_at(
        &mut self,
        impact_position: Vec3Net,
        warning_seconds: f32,
    ) -> Vec<ServerEnvelope> {
        let impact_tick = self
            .tick
            .saturating_add((warning_seconds.max(0.0) * SERVER_TICK_RATE_HZ) as u64);
        let trajectory_seed =
            splitmix64(self.chunk_manager.world_seed() ^ impact_tick ^ 0x00FA_11ED_57A8_0000);
        self.begin_meteor_shower_event(
            impact_position,
            impact_tick,
            trajectory_seed,
            warning_seconds,
        )
    }

    /// Register the active event at the given site/tick/seed and emit the
    /// reliable announce broadcast. Shared by the sited path
    /// ([`Self::begin_meteor_shower_with_warning`]) and the placed path
    /// ([`Self::begin_meteor_shower_at`]).
    fn begin_meteor_shower_event(
        &mut self,
        impact_position: Vec3Net,
        impact_tick: u64,
        trajectory_seed: u64,
        warning_seconds: f32,
    ) -> Vec<ServerEnvelope> {
        let despawn_tick = impact_tick
            .saturating_add((METEOR_SHOWER_DESPAWN_SECONDS * SERVER_TICK_RATE_HZ) as u64);

        bevy::log::info!(
            "meteor shower announced: impact at ({:.1}, {:.1}) on tick {impact_tick} (in {:.0}s)",
            impact_position.x,
            impact_position.z,
            warning_seconds
        );

        self.meteor_shower.next_announce_tick = None;
        self.meteor_shower.active = Some(ActiveMeteorShower {
            impact_position,
            impact_tick,
            trajectory_seed,
            resolved: false,
            despawn_tick,
            crater_nodes: Vec::new(),
        });

        vec![ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::MeteorShower {
                impact_position,
                impact_tick,
                trajectory_seed,
            },
        }]
    }

    /// The announce envelope for a client that just connected while an event is
    /// live (announce through crater despawn), or `None` if no event is active.
    /// Appended by the connection/welcome path so late joiners see the meteor or
    /// crater immediately. The client keys the sky/crater on this one payload.
    pub(super) fn meteor_shower_announce_for(&self, client_id: ClientId) -> Option<ServerEnvelope> {
        let event = self.meteor_shower.active.as_ref()?;
        Some(ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::MeteorShower {
                impact_position: event.impact_position,
                impact_tick: event.impact_tick,
                trajectory_seed: event.trajectory_seed,
            },
        })
    }

    /// Choose a clearance-safe impact point. Building safety is guaranteed by
    /// SITING, not by a damage exemption: candidates in the outer ring inside
    /// `PlayableBounds` (with margin) are rejected within
    /// `METEOR_SHOWER_BUILDING_CLEARANCE_M` of ANY deployed entity (building pieces
    /// are deployed entities too), Tool Cupboard claim footprint, or ruin
    /// footprint. On a densely-built map where every candidate fails, fall back
    /// to the candidate that maximises the distance to the nearest structure, so
    /// the meteor still lands as far from bases as the map allows.
    ///
    /// Deterministic in `trajectory_seed` so a test can assert the chosen site.
    pub(super) fn select_meteor_shower_site(&self, trajectory_seed: u64) -> Vec3Net {
        let dims = self.save.map.chunk_dims();
        let bounds = PlayableBounds::from_dims(dims);
        let min_x = bounds.min_x + METEOR_SHOWER_SITE_BOUNDS_MARGIN_M;
        let max_x = bounds.max_x - METEOR_SHOWER_SITE_BOUNDS_MARGIN_M;
        let min_z = bounds.min_z + METEOR_SHOWER_SITE_BOUNDS_MARGIN_M;
        let max_z = bounds.max_z - METEOR_SHOWER_SITE_BOUNDS_MARGIN_M;

        let playable_radius = bounds.max_x.max(1.0);
        let ring = playable_radius * METEOR_SHOWER_SITE_MIN_CENTER_DISTANCE_FRACTION;
        let ring_sq = ring * ring;

        // Precompute the structure obstacle set once: deployed-entity positions
        // (covers building pieces and placed deployables), claim footprint cells,
        // and ruin footprints.
        let obstacles = self.meteor_shower_obstacle_positions();

        let mut state = splitmix64(trajectory_seed ^ 0x0053_1737_0000_0000);
        let mut next = || {
            state = splitmix64(state);
            ((state >> 40) as f32) / ((1u64 << 24) as f32)
        };

        // Degenerate on a tiny world: no ring room. Fall back to the world edge.
        if max_x <= min_x || max_z <= min_z {
            return Vec3Net::new(max_x.max(0.0), 0.0, 0.0);
        }

        let mut best_fallback: Option<(Vec3Net, f32)> = None;
        for _ in 0..METEOR_SHOWER_SITE_CANDIDATES {
            let x = min_x + (max_x - min_x) * next();
            let z = min_z + (max_z - min_z) * next();

            // Outer ring only: keep meteors away from the central spawn area.
            if x * x + z * z < ring_sq {
                continue;
            }

            let nearest_sq = nearest_obstacle_distance_sq(x, z, &obstacles);
            // Track the max-distance candidate for the dense-map fallback.
            if best_fallback.is_none_or(|(_, best)| nearest_sq > best) {
                best_fallback = Some((Vec3Net::new(x, 0.0, z), nearest_sq));
            }
            // Accept the first candidate clear of every structure.
            if nearest_sq >= METEOR_SHOWER_BUILDING_CLEARANCE_M * METEOR_SHOWER_BUILDING_CLEARANCE_M
            {
                return Vec3Net::new(x, 0.0, z);
            }
        }

        // Every candidate was within clearance of something (heavily built map):
        // land the meteor at the safest sampled point.
        best_fallback
            .map(|(position, _)| position)
            .unwrap_or(Vec3Net::new(max_x.max(0.0), 0.0, 0.0))
    }

    /// The XZ positions the site selector keeps clear: every deployed entity
    /// (building pieces + deployables), every claim footprint cell, and every
    /// ruin footprint centre. Ruin footprints carry a radius; we conservatively
    /// treat them as points and rely on the generous clearance to keep the
    /// crater off the masonry.
    fn meteor_shower_obstacle_positions(&self) -> Vec<(f32, f32)> {
        let mut out: Vec<(f32, f32)> = Vec::new();
        for entity in self.deployed_entities.values() {
            out.push((entity.position.x, entity.position.z));
        }
        for cells in self.claim_footprints.values() {
            for (x, z) in cells {
                out.push((*x, *z));
            }
        }
        let seed = self.save.map.world_seed();
        let dims = self.save.map.chunk_dims();
        for footprint in crate::world::ruin_footprints(&crate::world::ruin_layout(seed, dims)) {
            out.push((footprint.x, footprint.z));
        }
        out
    }

    /// Resolve the meteor impact: apply the Blast to players in the radius,
    /// deplete resource nodes in the radius, and scatter the meteorite crater
    /// cluster. Marks the event resolved so it never fires twice.
    fn resolve_meteor_shower_impact(&mut self) -> Vec<ServerEnvelope> {
        let Some(event) = self.meteor_shower.active.as_ref() else {
            return Vec::new();
        };
        let center = event.impact_position;
        let trajectory_seed = event.trajectory_seed;

        let mut envelopes = Vec::new();

        // Players inside the radius take Blast damage (armor applies; ground zero
        // is lethal through any set). Routed through the shared damage helper so
        // Correction / knockback / death all flow correctly.
        envelopes.extend(self.resolve_blast_on_players(
            center,
            METEOR_SHOWER_IMPACT_RADIUS_M,
            METEOR_SHOWER_IMPACT_PLAYER_DAMAGE,
            DamageKind::Blast,
        ));

        // Deplete every resource node inside the crater. Same depletion path a
        // final gather swing takes (removal + regrow schedule + depleted
        // broadcast) so clients play the shatter/fell death effect.
        envelopes.extend(self.deplete_nodes_in_radius(center, METEOR_SHOWER_IMPACT_RADIUS_M));

        // Scatter the rich meteorite crater cluster inside the radius.
        let crater_nodes = self.spawn_meteor_shower_crater_nodes(center, trajectory_seed);

        if let Some(event) = self.meteor_shower.active.as_mut() {
            event.resolved = true;
            event.crater_nodes = crater_nodes;
        }

        envelopes
    }

    /// Apply a spherical Blast to players only: linear falloff from `center` to
    /// zero at `radius`, per-player armor via `damage_after_armor`, then the
    /// shared post-hit tail (`apply_player_damage`). Ground zero at `max_damage`
    /// is lethal through any current armor (the blast column caps well under
    /// 100%). Structure-damaging explosions are Phase 6; this is deliberately
    /// players-only and written so that generalisation is additive (Phase 6 adds
    /// structure/deployable passes beside this, reusing the same falloff math).
    ///
    /// Returns the consequence envelopes. `attacker_id`/`attacker_name` are
    /// sourceless (the meteor has no player killer), so a kill credits nobody.
    pub(super) fn resolve_blast_on_players(
        &mut self,
        center: Vec3Net,
        radius: f32,
        max_damage: f32,
        kind: DamageKind,
    ) -> Vec<ServerEnvelope> {
        if radius <= 0.0 || max_damage <= 0.0 {
            return Vec::new();
        }
        // Snapshot the affected (client_id, raw post-falloff damage) first so we
        // don't hold a borrow across the mutating damage calls.
        let mut hits: Vec<(ClientId, u32)> = Vec::new();
        for client in self.clients.values() {
            // Only live, alive bodies take the blast (a sleeper/corpse is inert).
            if !client.online || client.lifecycle.is_dead() {
                continue;
            }
            let distance = distance_xz(client.controller.position, center);
            if distance > radius {
                continue;
            }
            // Linear falloff: full at the centre, zero at the edge.
            let falloff = (1.0 - distance / radius).clamp(0.0, 1.0);
            let raw = (max_damage * falloff).round() as u32;
            if raw == 0 {
                continue;
            }
            // Armor for this kind, then post-armor damage. Ground zero (falloff
            // 1.0) at the meteor's max stays lethal through any set because the
            // blast column caps well below 100%.
            let armor = client.protection.for_kind(kind);
            let post = crate::combat::damage_after_armor(raw, armor);
            if post == 0 {
                continue;
            }
            hits.push((client.client_id, post));
        }

        let mut envelopes = Vec::new();
        for (target_id, damage_dealt) in hits {
            // Radial knockback away from the blast centre, scaled by proximity.
            let target_pos = self
                .clients
                .get(&target_id)
                .map(|c| c.controller.position)
                .unwrap_or(center);
            let knockback = knockback_impulse(center, target_pos, blast_knockback_speed(kind));
            envelopes.extend(self.apply_player_damage(PlayerDamageHit {
                target_id,
                attacker_id: None,
                attacker_name: "",
                damage_dealt,
                kind,
                knockback,
                impact: None,
            }));
        }
        envelopes
    }

    /// Deplete (and schedule normal regrow for) every resource node whose centre
    /// is inside `radius` of `center`. Broadcasts `ResourceNodeDepleted` per node
    /// so clients play the death animation, and untracks each via the shared
    /// `handle_node_depleted` chunk path.
    fn deplete_nodes_in_radius(&mut self, center: Vec3Net, radius: f32) -> Vec<ServerEnvelope> {
        let doomed: Vec<ResourceNodeId> = self
            .resource_nodes
            .values()
            .filter(|node| distance_xz(node.position, center) <= radius)
            .map(|node| node.id)
            .collect();

        let mut envelopes = Vec::new();
        for id in doomed {
            self.remove_resource_node(id);
            self.chunk_manager.handle_node_depleted(id, self.tick);
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::ResourceNodeDepleted { id },
            });
        }
        envelopes
    }

    /// Scatter `METEOR_SHOWER_CRATER_NODE_COUNT_MIN..=MAX` rich meteorite crater
    /// nodes inside the impact radius via the P5a runtime-spawn recipe. Placement
    /// is seeded from `trajectory_seed` with a minimum spacing so crater nodes do not
    /// overlap. Returns the spawned node ids so cleanup can force-despawn any
    /// unmined ones later.
    fn spawn_meteor_shower_crater_nodes(
        &mut self,
        center: Vec3Net,
        trajectory_seed: u64,
    ) -> Vec<ResourceNodeId> {
        let mut state = splitmix64(trajectory_seed ^ 0x0053_4841_5244_0000);
        let mut next = || {
            state = splitmix64(state);
            ((state >> 40) as f32) / ((1u64 << 24) as f32)
        };

        let span =
            METEOR_SHOWER_CRATER_NODE_COUNT_MAX.saturating_sub(METEOR_SHOWER_CRATER_NODE_COUNT_MIN);
        let count = METEOR_SHOWER_CRATER_NODE_COUNT_MIN
            + if span == 0 {
                0
            } else {
                (next() * (span + 1) as f32) as u32 % (span + 1)
            };

        let Some(kind) = NodeKind::from_definition_id(METEORITE_NODE_ID) else {
            return Vec::new();
        };

        // Keep crater nodes inside a slightly-inset ring so they sit within the crater,
        // not on its rim.
        let scatter_radius =
            (METEOR_SHOWER_IMPACT_RADIUS_M * 0.8).max(METEOR_SHOWER_CRATER_NODE_SPACING_M);
        let world_seed = self.chunk_manager.world_seed();

        let mut placed: Vec<Vec3Net> = Vec::new();
        let mut spawned: Vec<ResourceNodeId> = Vec::new();
        // Give each crater node a bounded number of placement attempts so a tight
        // spacing can't loop forever.
        let mut attempts = 0u32;
        while (spawned.len() as u32) < count && attempts < count * 12 {
            attempts += 1;
            // Uniform disc sample: sqrt on the radius fraction so crater nodes don't
            // clump at the centre. Seated ON the crater surface (the client
            // renders the raised-rim mound from the same shared profile), sunk
            // a touch so the boulder beds into the jittered mesh instead of
            // hovering where the visual surface dips below the analytic one.
            let r = scatter_radius * next().sqrt();
            let theta = next() * std::f32::consts::TAU;
            let position = Vec3Net::new(
                center.x + r * theta.cos(),
                (crate::world::crater_surface_height(r) - 0.15).max(0.0),
                center.z + r * theta.sin(),
            );
            if placed
                .iter()
                .any(|p| distance_xz(*p, position) < METEOR_SHOWER_CRATER_NODE_SPACING_M)
            {
                continue;
            }

            let node_id = self.allocate_resource_node_id();
            let spawn = WorldResourceNodeSpawn::new(
                node_id,
                METEORITE_NODE_ID,
                position,
                next() * std::f32::consts::TAU,
            );
            let Some(node) = spawn_resource_node(&spawn, Some(world_seed)) else {
                continue;
            };
            self.chunk_manager
                .track_resource_node(node_id, kind, position);
            self.insert_resource_node(node_id, node);
            placed.push(position);
            spawned.push(node_id);
        }
        spawned
    }

    /// Force-despawn any unmined crater nodes and clear the event, then roll the
    /// next one. Broadcasts `ResourceNodeDepleted` per remaining crater node so clients
    /// remove the visual; untracks each cleanly (NO regrow, these were event
    /// spawns, not world nodes) via `untrack_resource_node`.
    fn cleanup_meteor_shower(&mut self) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        let crater_nodes = self
            .meteor_shower
            .active
            .as_ref()
            .map(|event| event.crater_nodes.clone())
            .unwrap_or_default();

        for id in crater_nodes {
            // Only touch crater nodes still in the live map; a player may have mined one
            // out already (which removed + untracked it via the gather path).
            if self.remove_resource_node(id).is_some() {
                self.chunk_manager.untrack_resource_node(id);
                envelopes.push(ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::ResourceNodeDepleted { id },
                });
            }
        }

        // Clear the event and roll the next one off a fresh index so it differs
        // from the just-finished event's window.
        self.meteor_shower.active = None;
        self.meteor_shower.next_announce_tick = Some(roll_next_announce_tick(
            self.tick,
            self.chunk_manager.world_seed(),
            self.tick.wrapping_add(1),
        ));
        envelopes
    }
}

/// XZ (ground-plane) distance between two world positions. The blast and node
/// checks ignore vertical offset so a node on a slope or a player mid-jump reads
/// the same distance as one on flat ground.
fn distance_xz(a: Vec3Net, b: Vec3Net) -> f32 {
    a.horizontal_distance_squared(b).sqrt()
}

/// Squared distance from `(x, z)` to the nearest obstacle point, or `f32::MAX`
/// when the world has no structures at all (a fresh map, every candidate is
/// trivially clear).
fn nearest_obstacle_distance_sq(x: f32, z: f32, obstacles: &[(f32, f32)]) -> f32 {
    let mut nearest = f32::MAX;
    for (ox, oz) in obstacles {
        let dx = x - ox;
        let dz = z - oz;
        let d = dx * dx + dz * dz;
        if d < nearest {
            nearest = d;
        }
    }
    nearest
}

/// Knockback speed for a blast of the given kind. A single shared knob (in
/// `game_balance`) so the meteor and the Phase 6 explosives shove with the same
/// weight; per-kind variation, if ever wanted, slots in here without touching
/// the falloff math.
fn blast_knockback_speed(_kind: DamageKind) -> f32 {
    crate::game_balance::EXPLOSION_KNOCKBACK_SPEED
}

/// Build a toast envelope to the issuer. Local helper so the admin command does
/// not need to reach into the `commands` module's private reply helpers.
fn meteor_shower_toast(
    client_id: ClientId,
    kind: crate::protocol::ToastKind,
    text: impl Into<String>,
) -> ServerEnvelope {
    ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(crate::protocol::ToastMessage::new(kind, text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_balance::{
        METEOR_SHOWER_BUILDING_CLEARANCE_M, METEOR_SHOWER_CRATER_NODE_COUNT_MAX,
        METEOR_SHOWER_CRATER_NODE_COUNT_MIN, METEOR_SHOWER_IMPACT_RADIUS_M,
    };

    #[test]
    fn clearance_exceeds_impact_radius() {
        // The whole "sited to be safe, never damage-exempted" guarantee rests on
        // this: a base is genuine shelter only if the clearance is strictly
        // larger than the blast that would otherwise reach it. Enforced at
        // compile time so a future balance edit that inverts them fails the build,
        // not just this test.
        const {
            assert!(
                METEOR_SHOWER_BUILDING_CLEARANCE_M > METEOR_SHOWER_IMPACT_RADIUS_M,
                "building clearance must exceed the impact radius so a sited meteor \
                 can never reach a base"
            );
        }
    }

    #[test]
    fn crater_node_count_bounds_are_sane() {
        const {
            assert!(METEOR_SHOWER_CRATER_NODE_COUNT_MIN >= 1);
            assert!(METEOR_SHOWER_CRATER_NODE_COUNT_MAX >= METEOR_SHOWER_CRATER_NODE_COUNT_MIN);
        }
    }

    #[test]
    fn scheduler_rolls_within_the_configured_window() {
        // The announce lands (interval - warning) out; the impact lands `interval`
        // out. Check the derived announce tick falls inside the min/max window.
        let now = 1_000u64;
        for seed in [1u64, 2, 3, 100, 9999] {
            let announce = roll_next_announce_tick(now, seed, 1);
            let announce_offset_secs = (announce - now) as f32 / SERVER_TICK_RATE_HZ;
            let impact_secs = announce_offset_secs + METEOR_SHOWER_WARNING_SECONDS;
            let days = impact_secs / REAL_SECONDS_PER_DAY;
            assert!(
                (METEOR_SHOWER_INTERVAL_DAYS_MIN - 0.01..=METEOR_SHOWER_INTERVAL_DAYS_MAX + 0.01)
                    .contains(&days),
                "seed {seed}: rolled {days} in-game days, outside \
                 [{METEOR_SHOWER_INTERVAL_DAYS_MIN}, {METEOR_SHOWER_INTERVAL_DAYS_MAX}]"
            );
        }
    }

    #[test]
    fn different_seeds_roll_different_schedules() {
        let now = 0u64;
        let a = roll_next_announce_tick(now, 1, 1);
        let b = roll_next_announce_tick(now, 2, 1);
        assert_ne!(a, b, "distinct world seeds should roll distinct schedules");
    }

    #[test]
    fn nearest_obstacle_distance_is_max_with_no_obstacles() {
        assert_eq!(nearest_obstacle_distance_sq(10.0, 10.0, &[]), f32::MAX);
    }

    #[test]
    fn nearest_obstacle_distance_finds_the_closest() {
        let obstacles = [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)];
        let d = nearest_obstacle_distance_sq(5.0, 0.0, &obstacles);
        assert!((d - 25.0).abs() < 1e-3, "closest is (0,0) at 5m, d^2=25");
    }
}

/// Integration tests against a live `GameServer`: site selection with real
/// structures, impact resolution (falloff, armor, lethality, node clearing,
/// crater-node spawns), the announce resend to a mid-event joiner, and the
/// force-despawn + cleanup lifecycle.
#[cfg(test)]
mod server_tests {
    use crate::{
        combat::DamageKind,
        game_balance::{
            METEOR_SHOWER_BUILDING_CLEARANCE_M, METEOR_SHOWER_CRATER_NODE_COUNT_MAX,
            METEOR_SHOWER_CRATER_NODE_COUNT_MIN, METEOR_SHOWER_IMPACT_RADIUS_M,
        },
        items::{IRON_BOOTS_ID, IRON_CUIRASS_ID, IRON_GREAVES_ID, IRON_HELM_ID},
        protocol::{ItemStack, MAX_HEALTH, ServerMessage, Vec3Net},
        resource_nodes::METEORITE_NODE_ID,
        server::test_support::{connect_named, place_building, server},
        world::{NodeKind, PlayableBounds},
    };

    /// Force an event onto the server with an explicit impact site + tick, so a
    /// test can drive the impact deterministically without waiting on the
    /// scheduler.
    fn force_event(
        server: &mut crate::server::GameServer,
        impact_position: Vec3Net,
        impact_tick: u64,
    ) {
        server.meteor_shower.active = Some(super::ActiveMeteorShower {
            impact_position,
            impact_tick,
            trajectory_seed: 0xABCD_1234,
            resolved: false,
            despawn_tick: impact_tick + 1000,
            crater_nodes: Vec::new(),
        });
    }

    #[test]
    fn site_selection_never_lands_within_clearance_of_structures() {
        // A seeded dense world: scatter building pieces, a claim footprint, and
        // keep the ruins. Sample many seeds and assert every chosen site clears
        // every structure, building safety is guaranteed by SITING.
        let mut server = server();
        let host = connect_named(&mut server, "Host");
        let _ = host;

        // Build a dense cluster of pieces near the world centre and out into the
        // ring so candidates have plenty to avoid.
        for i in 0..40 {
            let angle = i as f32 * 0.618 * std::f32::consts::TAU;
            let r = 30.0 + (i as f32) * 8.0;
            let pos = Vec3Net::new(r * angle.cos(), 0.0, r * angle.sin());
            place_building(
                &mut server,
                crate::building::BuildingPiece::Foundation,
                pos,
                0.0,
            );
        }
        // A claim footprint too (fake cells around one base cluster).
        let mut cells = Vec::new();
        for gx in -3..=3 {
            for gz in -3..=3 {
                cells.push((60.0 + gx as f32 * 3.0, 60.0 + gz as f32 * 3.0));
            }
        }
        server
            .claim_footprints
            .insert(crate::protocol::DeployedEntityId(999), cells);

        let obstacles = server.meteor_shower_obstacle_positions();
        assert!(!obstacles.is_empty(), "the test world must have structures");

        // For a spread of trajectory seeds, the chosen site clears everything OR
        // (on a genuinely saturated map) is the max-distance fallback. Assert the
        // clearance holds for seeds that can be satisfied; the fallback is only
        // reached when NO candidate clears, which this moderately-built map does
        // not hit, so every seed here should clear.
        let clearance_sq = METEOR_SHOWER_BUILDING_CLEARANCE_M * METEOR_SHOWER_BUILDING_CLEARANCE_M;
        for seed in 0..64u64 {
            let site = server.select_meteor_shower_site(seed);
            let nearest_sq = obstacles
                .iter()
                .map(|(x, z)| {
                    let dx = site.x - x;
                    let dz = site.z - z;
                    dx * dx + dz * dz
                })
                .fold(f32::MAX, f32::min);
            assert!(
                nearest_sq >= clearance_sq,
                "seed {seed}: chosen site ({:.1}, {:.1}) is within clearance of a structure \
                 (nearest {:.1} m, need {:.1} m)",
                site.x,
                site.z,
                nearest_sq.sqrt(),
                METEOR_SHOWER_BUILDING_CLEARANCE_M
            );
        }
    }

    #[test]
    fn site_selection_falls_back_to_max_distance_when_saturated() {
        // Blanket the outer ring with structures so NO candidate clears; the
        // selector must still return a point, the one that maximises distance to
        // the nearest structure (never panic, never land on a base).
        let mut server = server();
        let dims = server.save.map.chunk_dims();
        let bounds = PlayableBounds::from_dims(dims);
        let radius = bounds.max_x;

        // Dense grid of foundations across the whole playable ring.
        let step = 15.0;
        let mut x = -radius + step;
        while x < radius {
            let mut z = -radius + step;
            while z < radius {
                if x * x + z * z > (radius * 0.3) * (radius * 0.3) {
                    place_building(
                        &mut server,
                        crate::building::BuildingPiece::Foundation,
                        Vec3Net::new(x, 0.0, z),
                        0.0,
                    );
                }
                z += step;
            }
            x += step;
        }

        // Must not panic and must return a finite in-bounds point.
        let site = server.select_meteor_shower_site(7);
        assert!(site.x.is_finite() && site.z.is_finite());
        assert!(bounds.contains(site.x, site.z), "fallback stays in bounds");
    }

    #[test]
    fn blast_falloff_is_linear_and_armored_takes_less_than_unarmored() {
        let mut server = server();
        let center = Vec3Net::new(0.0, 0.0, 0.0);

        // Unarmored player at the crater edge (half radius -> ~half damage).
        let a = connect_named(&mut server, "A");
        server.clients.get_mut(&a).unwrap().controller.position =
            Vec3Net::new(METEOR_SHOWER_IMPACT_RADIUS_M * 0.5, 0.0, 0.0);
        server.clients.get_mut(&a).unwrap().controller.health = MAX_HEALTH;

        let before = server.clients[&a].controller.health;
        let _ = server.resolve_blast_on_players(
            center,
            METEOR_SHOWER_IMPACT_RADIUS_M,
            80.0,
            DamageKind::Blast,
        );
        let unarmored_loss = before - server.clients[&a].controller.health;
        // Half radius -> ~half of 80 = ~40 raw. Within rounding.
        assert!(
            (unarmored_loss - 40.0).abs() <= 2.0,
            "linear falloff at half radius should deal ~40, got {unarmored_loss}"
        );

        // Now an iron-armored player at the same distance takes strictly less.
        let b = connect_named(&mut server, "B");
        {
            let client = server.clients.get_mut(&b).unwrap();
            client.controller.position =
                Vec3Net::new(METEOR_SHOWER_IMPACT_RADIUS_M * 0.5, 0.0, 0.0);
            client.controller.health = MAX_HEALTH;
            client.inventory.equipment_slots[0] = Some(ItemStack::new(IRON_HELM_ID, 1));
            client.inventory.equipment_slots[1] = Some(ItemStack::new(IRON_CUIRASS_ID, 1));
            client.inventory.equipment_slots[2] = Some(ItemStack::new(IRON_GREAVES_ID, 1));
            client.inventory.equipment_slots[3] = Some(ItemStack::new(IRON_BOOTS_ID, 1));
        }
        server.recompute_protection(b);
        let before_b = server.clients[&b].controller.health;
        let _ = server.resolve_blast_on_players(
            center,
            METEOR_SHOWER_IMPACT_RADIUS_M,
            80.0,
            DamageKind::Blast,
        );
        let armored_loss = before_b - server.clients[&b].controller.health;
        assert!(
            armored_loss < unarmored_loss,
            "iron armor should reduce blast damage: armored {armored_loss} vs unarmored \
             {unarmored_loss}"
        );
        assert!(
            armored_loss > 0.0,
            "armor should not fully negate the blast"
        );
    }

    #[test]
    fn ground_zero_is_lethal_through_full_iron_armor() {
        let mut server = server();
        let center = Vec3Net::new(0.0, 0.0, 0.0);
        let victim = connect_named(&mut server, "Victim");
        {
            let client = server.clients.get_mut(&victim).unwrap();
            client.controller.position = center; // standing on the marker
            client.controller.health = MAX_HEALTH;
            client.inventory.equipment_slots[0] = Some(ItemStack::new(IRON_HELM_ID, 1));
            client.inventory.equipment_slots[1] = Some(ItemStack::new(IRON_CUIRASS_ID, 1));
            client.inventory.equipment_slots[2] = Some(ItemStack::new(IRON_GREAVES_ID, 1));
            client.inventory.equipment_slots[3] = Some(ItemStack::new(IRON_BOOTS_ID, 1));
        }
        server.recompute_protection(victim);
        let _ = server.resolve_blast_on_players(
            center,
            METEOR_SHOWER_IMPACT_RADIUS_M,
            crate::game_balance::METEOR_SHOWER_IMPACT_PLAYER_DAMAGE,
            DamageKind::Blast,
        );
        assert!(
            server.clients[&victim].controller.health <= 0.0,
            "ground zero must be lethal through any armor set"
        );
    }

    #[test]
    fn player_outside_the_radius_takes_no_blast() {
        let mut server = server();
        let center = Vec3Net::new(0.0, 0.0, 0.0);
        let safe = connect_named(&mut server, "Safe");
        server.clients.get_mut(&safe).unwrap().controller.position =
            Vec3Net::new(METEOR_SHOWER_IMPACT_RADIUS_M + 5.0, 0.0, 0.0);
        server.clients.get_mut(&safe).unwrap().controller.health = MAX_HEALTH;
        let _ = server.resolve_blast_on_players(
            center,
            METEOR_SHOWER_IMPACT_RADIUS_M,
            crate::game_balance::METEOR_SHOWER_IMPACT_PLAYER_DAMAGE,
            DamageKind::Blast,
        );
        assert_eq!(
            server.clients[&safe].controller.health, MAX_HEALTH,
            "a player beyond the impact radius takes no blast"
        );
    }

    #[test]
    fn impact_clears_nodes_in_radius_and_spawns_crater_nodes_in_range() {
        let mut server = server();
        let center = Vec3Net::new(200.0, 0.0, 200.0);

        // Spawn a couple of resource nodes inside the crater and one just outside.
        let kind = NodeKind::from_definition_id(crate::resource_nodes::COAL_NODE_ID).unwrap();
        let inside = server.allocate_resource_node_id();
        let inside_pos = Vec3Net::new(center.x + 3.0, 0.0, center.z);
        let node = crate::resource_nodes::spawn_resource_node(
            &crate::world::WorldResourceNodeSpawn::new(
                inside,
                crate::resource_nodes::COAL_NODE_ID,
                inside_pos,
                0.0,
            ),
            Some(server.chunk_manager.world_seed()),
        )
        .unwrap();
        server
            .chunk_manager
            .track_resource_node(inside, kind, inside_pos);
        server.insert_resource_node(inside, node);

        let outside = server.allocate_resource_node_id();
        let outside_pos = Vec3Net::new(
            center.x + METEOR_SHOWER_IMPACT_RADIUS_M + 10.0,
            0.0,
            center.z,
        );
        let node = crate::resource_nodes::spawn_resource_node(
            &crate::world::WorldResourceNodeSpawn::new(
                outside,
                crate::resource_nodes::COAL_NODE_ID,
                outside_pos,
                0.0,
            ),
            Some(server.chunk_manager.world_seed()),
        )
        .unwrap();
        server
            .chunk_manager
            .track_resource_node(outside, kind, outside_pos);
        server.insert_resource_node(outside, node);

        let now = server.tick;
        force_event(&mut server, center, now);
        let envelopes = server.resolve_meteor_shower_impact();

        // The inside node was removed; the outside node survived.
        assert!(
            server.resource_nodes.get(&inside).is_none(),
            "in-radius node cleared"
        );
        assert!(
            server.resource_nodes.get(&outside).is_some(),
            "out-of-radius node survives"
        );
        // A ResourceNodeDepleted was broadcast for the cleared node.
        assert!(
            envelopes.iter().any(|e| matches!(
                &e.message,
                ServerMessage::ResourceNodeDepleted { id } if *id == inside
            )),
            "the cleared node must broadcast a depletion so clients animate it"
        );

        // The crater cluster spawned within count bounds, all meteorite, all
        // inside the crater, and all tracked in the AoI index.
        let crater_nodes = server
            .meteor_shower
            .active
            .as_ref()
            .unwrap()
            .crater_nodes
            .clone();
        assert!(
            (crater_nodes.len() as u32) >= METEOR_SHOWER_CRATER_NODE_COUNT_MIN
                && (crater_nodes.len() as u32) <= METEOR_SHOWER_CRATER_NODE_COUNT_MAX,
            "crater-node count {} out of [{METEOR_SHOWER_CRATER_NODE_COUNT_MIN}, \
             {METEOR_SHOWER_CRATER_NODE_COUNT_MAX}]",
            crater_nodes.len()
        );
        for id in &crater_nodes {
            let node = server.resource_nodes.get(id).expect("crater node is live");
            assert_eq!(
                node.definition_id, METEORITE_NODE_ID,
                "crater nodes are meteorite"
            );
            let d = node.position.horizontal_distance_squared(center).sqrt();
            assert!(
                d <= METEOR_SHOWER_IMPACT_RADIUS_M,
                "crater node {id} inside the crater"
            );
            assert!(
                server.chunk_manager.node_chunk(*id).is_some(),
                "crater node {id} must be tracked in the chunk index (AoI visible)"
            );
        }
    }

    #[test]
    fn forced_despawn_and_cleanup_removes_crater_nodes_and_reschedules() {
        let mut server = server();
        let center = Vec3Net::new(150.0, 0.0, -150.0);
        let now = server.tick;
        force_event(&mut server, center, now);
        let _ = server.resolve_meteor_shower_impact();
        let crater_nodes = server
            .meteor_shower
            .active
            .as_ref()
            .unwrap()
            .crater_nodes
            .clone();
        assert!(!crater_nodes.is_empty());

        // Jump the clock past the despawn tick and run cleanup.
        server.tick = server.meteor_shower.active.as_ref().unwrap().despawn_tick + 1;
        let envelopes = server.cleanup_meteor_shower();

        // Every crater node was removed + untracked and a depletion broadcast.
        for id in &crater_nodes {
            assert!(
                server.resource_nodes.get(id).is_none(),
                "crater node {id} despawned"
            );
            assert!(
                server.chunk_manager.node_chunk(*id).is_none(),
                "crater node {id} untracked from the chunk index"
            );
        }
        assert!(
            crater_nodes
                .iter()
                .all(|id| envelopes.iter().any(|e| matches!(
                    &e.message,
                    ServerMessage::ResourceNodeDepleted { id: broadcast } if broadcast == id
                ))),
            "each despawned crater node must broadcast a depletion"
        );
        // The event cleared and a fresh next event was rolled.
        assert!(
            server.meteor_shower.active.is_none(),
            "event cleared after cleanup"
        );
        assert!(
            server.meteor_shower.next_announce_tick.is_some(),
            "cleanup must roll the next event"
        );
    }

    #[test]
    fn meteor_shower_here_lands_at_the_callers_position_bypassing_siting() {
        // `/meteor_shower-here` must place ground zero exactly at the admin's feet,
        // even sitting on a building (siting/clearance is bypassed by design so an
        // admin can watch it land where they stand).
        let mut server = server();
        let admin = connect_named(&mut server, "Admin");
        server.clients.get_mut(&admin).unwrap().is_admin = true;
        let stand = Vec3Net::new(137.0, 0.0, -92.0);
        server.clients.get_mut(&admin).unwrap().controller.position = stand;

        // Drop a building right where the admin stands: the placed meteor must
        // still land here, proving the clearance check is skipped.
        place_building(
            &mut server,
            crate::building::BuildingPiece::Foundation,
            stand,
            0.0,
        );

        let envelopes = server.command_meteor_shower_here(admin, &["8"]);
        let announced = envelopes.iter().find_map(|e| match &e.message {
            ServerMessage::MeteorShower {
                impact_position, ..
            } => Some(*impact_position),
            _ => None,
        });
        assert_eq!(
            announced,
            Some(Vec3Net::new(stand.x, 0.0, stand.z)),
            "the placed meteor must land at the caller's XZ, not a sited point"
        );
        let active = server
            .meteor_shower
            .active
            .as_ref()
            .expect("event is active");
        assert_eq!(active.impact_position, Vec3Net::new(stand.x, 0.0, stand.z));
    }

    #[test]
    fn meteor_shower_here_is_admin_gated() {
        let mut server = server();
        let plain = connect_named(&mut server, "Plain");
        // Force non-admin (the first loopback client can auto-admin): the command
        // must not schedule an event.
        server.clients.get_mut(&plain).unwrap().is_admin = false;
        let envelopes = server.command_meteor_shower_here(plain, &[]);
        assert!(
            server.meteor_shower.active.is_none(),
            "a non-admin must not be able to place a meteor"
        );
        assert!(
            envelopes.iter().any(|e| matches!(
                &e.message,
                ServerMessage::Toast(t) if t.text.contains("admin only")
            )),
            "a non-admin gets an admin-only warning"
        );
    }

    #[test]
    fn announce_is_resent_to_a_client_that_joins_mid_event() {
        // Start an event, then connect a fresh client through the real connection
        // path and assert its envelopes include the meteor shower announce (the
        // late-joiner resend), so a player who joins mid-meteor sees it.
        let mut server = server();
        let center = Vec3Net::new(100.0, 0.0, 100.0);
        let now = server.tick;
        force_event(&mut server, center, now + 500);

        let (_client_id, envelopes) = server
            .connect(
                crate::protocol::PROTOCOL_VERSION,
                Some(crate::protocol::GAME_VERSION.to_owned()),
                crate::protocol::AccountId(42),
                "LateJoiner".to_owned(),
                String::new(),
            )
            .expect("late joiner connects");

        let announce = envelopes.iter().find_map(|e| match &e.message {
            ServerMessage::MeteorShower {
                impact_position, ..
            } => Some(*impact_position),
            _ => None,
        });
        assert_eq!(
            announce,
            Some(center),
            "a mid-event joiner must receive the meteor shower announce resend"
        );
    }
}
