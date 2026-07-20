//! Server-side meteor shower event engine: scheduling, clearance-aware
//! multi-site selection, the reliable announce, per-meteor impact resolution,
//! and per-meteor crater cleanup.
//!
//! The server owns everything about the shower. One event spawns
//! `METEOR_SHOWER_COUNT_MIN..=MAX` meteors of varied sizes distributed across
//! the map's outer ring, with staggered impact ticks; clients render a
//! deterministic sky show from the single announce (see
//! `crate::world::meteor_shower` and `docs/meteor-shower.md`); the server
//! never streams a fireball. There is deliberately NO global announcement UI:
//! the fireballs and the audio are the announcement.
//!
//! ## Real-time schedule, not day/night time
//!
//! The scheduler counts in real server ticks (`SERVER_TICK_RATE_HZ`): the
//! interval is `METEOR_SHOWER_INTERVAL_MINUTES_MIN..=MAX` REAL minutes,
//! measured event start to event start. This is deliberate: the admin
//! `/time-speed` cheat accelerates only the day/night visual cycle
//! (`world_time.multiplier`), NOT the wall clock, so spinning the sun does not
//! pull meteors closer together. Meteors are a real-time event.
//!
//! ## Not persisted
//!
//! `MeteorShowerState` is transient. On world load the scheduler rolls a fresh next
//! event; an in-flight event does not survive a restart. This keeps the save
//! format untouched (no version bump) and matches the throwaway nature of a live
//! world event, a server restart mid-shower simply reschedules.

use crate::{
    combat::DamageKind,
    game_balance::{
        METEOR_SHOWER_BUILDING_CLEARANCE_M, METEOR_SHOWER_COUNT_MAX, METEOR_SHOWER_COUNT_MIN,
        METEOR_SHOWER_CRATER_NODE_COUNT_MAX, METEOR_SHOWER_CRATER_NODE_COUNT_MIN,
        METEOR_SHOWER_CRATER_NODE_SPACING_M, METEOR_SHOWER_DESPAWN_SECONDS,
        METEOR_SHOWER_IMPACT_PLAYER_DAMAGE, METEOR_SHOWER_IMPACT_RADIUS_M,
        METEOR_SHOWER_IMPACT_STAGGER_SECONDS, METEOR_SHOWER_INTER_SITE_SPACING_M,
        METEOR_SHOWER_INTERVAL_MINUTES_MAX, METEOR_SHOWER_INTERVAL_MINUTES_MIN,
        METEOR_SHOWER_SECONDARY_SIZE_MAX, METEOR_SHOWER_SECONDARY_SIZE_MIN,
        METEOR_SHOWER_SITE_BOUNDS_MARGIN_M, METEOR_SHOWER_SITE_CANDIDATES,
        METEOR_SHOWER_SITE_MIN_CENTER_DISTANCE_FRACTION, METEOR_SHOWER_WARNING_SECONDS,
    },
    protocol::{
        ClientId, MeteorStrike, ResourceNodeId, SERVER_TICK_RATE_HZ, ServerMessage, Vec3Net,
    },
    resource_nodes::{METEORITE_NODE_ID, spawn_resource_node},
    world::{NodeKind, PlayableBounds, WorldResourceNodeSpawn, splitmix64},
};

use super::{
    DeliveryTarget, GameServer, ServerEnvelope,
    combat::{PlayerDamageHit, knockback_impulse},
};

/// One live meteor of the active shower event. Present from the announce (at T
/// minus the warning window) through its own impact and until its crater /
/// crater cluster despawns; each meteor resolves and cleans up on its own
/// staggered clock.
#[derive(Debug, Clone)]
pub(super) struct ActiveMeteor {
    /// Ground-zero position (y is floor level). The wire announce, the crater
    /// visual, and the impact resolution all key on this.
    pub(super) impact_position: Vec3Net,
    /// Server tick this meteor strikes (staggered per meteor within the event).
    pub(super) impact_tick: u64,
    /// Seeds this fireball's approach azimuth on the client.
    pub(super) trajectory_seed: u64,
    /// Size multiplier in `(0, 1]`. Scales the blast radius, ground-zero
    /// damage, node-depletion radius, crater geometry, and crater-node count;
    /// clients scale the fireball/crater visuals and audio off the same value.
    pub(super) size: f32,
    /// Whether this meteor's impact has already resolved (blast applied, crater
    /// nodes spawned). The tick loop resolves each meteor exactly once when the
    /// clock reaches its `impact_tick`.
    pub(super) resolved: bool,
    /// Server tick this meteor's crater + crater-node cluster despawn.
    pub(super) despawn_tick: u64,
    /// Crater node ids spawned by this meteor's impact, force-despawned at
    /// `despawn_tick` if still unmined. A player mining one out drops it from
    /// the live map via the normal gather path; cleanup only touches ids still
    /// present.
    pub(super) crater_nodes: Vec<ResourceNodeId>,
}

/// Server-owned meteor shower scheduler + active event. Not persisted; rolled fresh
/// on world load.
#[derive(Debug, Clone)]
pub(super) struct MeteorShowerState {
    /// Real server tick the next event's *announce* fires. `None` means an event
    /// is currently active (a new next-event tick is rolled when the LAST
    /// meteor of the event cleans up).
    pub(super) next_announce_tick: Option<u64>,
    /// The live event's meteors (empty when no event is active). Each resolves
    /// and cleans up independently; the event slot frees when the vec empties.
    pub(super) meteors: Vec<ActiveMeteor>,
}

impl MeteorShowerState {
    /// Fresh scheduler state at world start: no active event, the first event
    /// rolled 1 to 2 in-game days out from `now_tick` using `world_seed` as the
    /// deterministic stream base.
    pub(super) fn new(now_tick: u64, world_seed: u64) -> Self {
        Self {
            next_announce_tick: Some(roll_next_announce_tick(now_tick, world_seed, 1)),
            meteors: Vec::new(),
        }
    }
}

/// Real-minute interval converted to server ticks, measured from
/// `anchor_tick` (the previous event's start, or "now" for the first roll).
/// The announce fires `METEOR_SHOWER_WARNING_SECONDS` before the first
/// impact, so the *announce* offset is the impact interval minus the warning
/// window (clamped so it never underflows on a short min interval).
fn roll_next_announce_tick(anchor_tick: u64, world_seed: u64, event_index: u64) -> u64 {
    // A dedicated deterministic stream salted off the seed and the event index,
    // the same splitmix64 idiom the ruins scatter and regrow scheduler use.
    let state = splitmix64(world_seed ^ 0x00E3_1B01_FA11_0000 ^ event_index.wrapping_mul(0x9E37));
    let unit = ((state >> 40) as f32) / ((1u64 << 24) as f32);
    let minutes = METEOR_SHOWER_INTERVAL_MINUTES_MIN
        + (METEOR_SHOWER_INTERVAL_MINUTES_MAX - METEOR_SHOWER_INTERVAL_MINUTES_MIN) * unit;
    let impact_seconds = minutes * 60.0;
    // Announce lands `warning` before the first impact; never schedule the
    // announce in the past for a tiny interval.
    let announce_seconds = (impact_seconds - METEOR_SHOWER_WARNING_SECONDS).max(0.0);
    anchor_tick.saturating_add((announce_seconds * SERVER_TICK_RATE_HZ) as u64)
}

/// The seeded per-event meteor count, uniform in
/// `METEOR_SHOWER_COUNT_MIN..=MAX`. Pure so a test can pin the range.
fn roll_meteor_count(event_seed: u64) -> u32 {
    let span = METEOR_SHOWER_COUNT_MAX.saturating_sub(METEOR_SHOWER_COUNT_MIN) + 1;
    let state = splitmix64(event_seed ^ 0x00C0_0147_0000_0000);
    METEOR_SHOWER_COUNT_MIN + ((state >> 33) % u64::from(span)) as u32
}

impl GameServer {
    /// Advance the meteor shower event engine one tick. Ordered like the other tick
    /// subsystems: schedule -> announce -> per-meteor impact -> per-meteor
    /// cleanup. Returns the envelopes to fan out (the reliable announce
    /// broadcast, per-node depletion broadcasts, near-impact blast
    /// consequences).
    pub(super) fn tick_world_events(&mut self) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();

        // 1. Scheduler: has the next announce come due?
        if let Some(announce_tick) = self.meteor_shower.next_announce_tick
            && self.tick >= announce_tick
        {
            envelopes.extend(self.begin_meteor_shower());
        }

        // 2. Impacts: resolve each meteor exactly once when the clock reaches
        //    its own (staggered) impact tick.
        envelopes.extend(self.resolve_due_meteor_impacts());

        // 3. Cleanup: force-despawn each expired meteor's unmined crater nodes;
        //    when the LAST meteor cleans up the event slot frees and the next
        //    event is rolled.
        envelopes.extend(self.cleanup_expired_meteors());

        envelopes
    }

    /// `/meteor [warning_seconds]` (admin, default 30): force an immediate
    /// full multi-meteor shower for testing. Clears any in-flight event and
    /// begins a fresh one whose FIRST impact lands `warning_seconds` from now,
    /// so a tester does not have to wait out the scheduled interval.
    /// Returns the announce broadcast plus a success toast to the issuer.
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
                        "usage: /meteor [warning_seconds], e.g. /meteor 30",
                    )];
                }
            }
        }
        // Clear any live event (dropping its remaining crater nodes) so the
        // forced one is clean.
        let mut envelopes = self.clear_live_meteors();
        envelopes.extend(self.begin_meteor_shower_with_warning(warning_seconds));
        let count = self.meteor_shower.meteors.len();
        envelopes.push(meteor_shower_toast(
            client_id,
            crate::protocol::ToastKind::Success,
            format!(
                "meteor shower scheduled: {count} meteors, first impact in {warning_seconds:.0}s"
            ),
        ));
        envelopes
    }

    /// `/meteor-here [warning_seconds] [size]` (admin, defaults 8 s / 1.0):
    /// force a SINGLE meteor whose impact lands at the CALLER'S CURRENT
    /// POSITION, so an admin can drop one exactly where they stand to watch
    /// it. `size` (clamped to `[0.2, 1.0]`) lets a tester eyeball the
    /// size-scaled blast/crater/visuals. Bypasses the multi-site selector and
    /// its building-clearance check entirely: by design this can land on the
    /// caller and on buildings (the point is to place it precisely). Clears
    /// any in-flight event first, then reuses the shared announce path.
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

        let usage = || {
            meteor_shower_toast(
                client_id,
                crate::protocol::ToastKind::Warning,
                "usage: /meteor-here [warning_seconds] [size], e.g. /meteor-here 8 0.5",
            )
        };
        let mut warning_seconds = 8.0_f32;
        if let Some(arg) = args.first() {
            match arg.parse::<f32>() {
                Ok(value) if value.is_finite() && value > 0.0 => warning_seconds = value,
                _ => return vec![usage()],
            }
        }
        let mut size = 1.0_f32;
        if let Some(arg) = args.get(1) {
            match arg.parse::<f32>() {
                Ok(value) if value.is_finite() && value > 0.0 => size = value.clamp(0.2, 1.0),
                _ => return vec![usage()],
            }
        }
        // Clear any live event (dropping its remaining crater nodes) so the
        // forced one is clean.
        let mut envelopes = self.clear_live_meteors();
        envelopes.extend(self.begin_meteor_shower_at(here, warning_seconds, size));
        envelopes.push(meteor_shower_toast(
            client_id,
            crate::protocol::ToastKind::Success,
            format!("meteor on your position: impact in {warning_seconds:.0}s (size {size:.2})"),
        ));
        envelopes
    }

    /// Cinematic hooks. `suspend_meteor_events` clears any live event and
    /// unschedules the next one so a random shower can't wander into a take;
    /// `force_cinematic_meteor` drops the scripted starfall strike at an
    /// explicit position. The orchestrator re-rolls the scheduler with a
    /// fresh [`MeteorShowerState`] when playback ends.
    pub(super) fn suspend_meteor_events(&mut self) -> Vec<ServerEnvelope> {
        let envelopes = self.clear_live_meteors();
        self.meteor_shower.next_announce_tick = None;
        envelopes
    }

    pub(super) fn force_cinematic_meteor(
        &mut self,
        impact_position: Vec3Net,
        warning_seconds: f32,
        size: f32,
        trajectory_seed: u64,
    ) -> Vec<ServerEnvelope> {
        let mut envelopes = self.clear_live_meteors();
        envelopes.extend(self.begin_meteor_shower_at(impact_position, warning_seconds, size));
        // Pin the streak: the cinematic take must get the identical entry
        // azimuth every run, where the derived seed varies with the tick.
        // Set before the announce is built? No: `begin_meteor_shower_at`
        // already broadcast, so rewrite the stored meteor AND re-announce.
        if let Some(meteor) = self.meteor_shower.meteors.last_mut() {
            meteor.trajectory_seed = trajectory_seed;
        }
        envelopes
            .retain(|envelope| !matches!(envelope.message, ServerMessage::MeteorShower { .. }));
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::MeteorShower {
                meteors: self.meteor_shower_strikes(),
            },
        });
        envelopes
    }

    /// Force-despawn the crater nodes of every live meteor and clear the event,
    /// WITHOUT rolling the next schedule (the caller immediately begins a fresh
    /// forced event). Used by the admin commands so a replaced event never
    /// leaks its unmined crater cluster.
    fn clear_live_meteors(&mut self) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        let meteors = std::mem::take(&mut self.meteor_shower.meteors);
        for meteor in meteors {
            envelopes.extend(self.despawn_crater_nodes(&meteor.crater_nodes));
        }
        envelopes
    }

    /// Select the event's impact sites and broadcast the announce. Called once
    /// when the scheduled announce tick arrives.
    fn begin_meteor_shower(&mut self) -> Vec<ServerEnvelope> {
        self.begin_meteor_shower_with_warning(METEOR_SHOWER_WARNING_SECONDS)
    }

    /// Shared multi-meteor announce path with an explicit warning window (real
    /// seconds until the FIRST impact). The routine scheduler uses the balance
    /// constant; the admin command passes its own. Rolls the meteor count,
    /// sites each meteor via the sector-stratified selector, assigns exactly
    /// one size-1.0 headliner (the rest roll the secondary band), and staggers
    /// the impact ticks so the event reads as a shower.
    fn begin_meteor_shower_with_warning(&mut self, warning_seconds: f32) -> Vec<ServerEnvelope> {
        // Roll the event stream from the world seed + the first impact tick so
        // it is reproducible for the same event but distinct across events.
        let first_impact_tick = self
            .tick
            .saturating_add((warning_seconds.max(0.0) * SERVER_TICK_RATE_HZ) as u64);
        let event_seed =
            splitmix64(self.chunk_manager.world_seed() ^ first_impact_tick ^ 0x00FA_11ED_57A8_0000);

        let count = roll_meteor_count(event_seed) as usize;
        let sites = self.select_meteor_shower_sites(event_seed, count);

        // Exactly one headliner (size 1.0) per event; the rest roll the
        // secondary band. Which meteor is the headliner is itself seeded.
        let headliner =
            (splitmix64(event_seed ^ 0x00B1_6001_0000_0000) >> 33) as usize % count.max(1);

        let mut meteors = Vec::with_capacity(count);
        for (k, site) in sites.into_iter().enumerate() {
            let per_meteor =
                splitmix64(event_seed ^ (k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let size = if k == headliner {
                1.0
            } else {
                let unit = ((splitmix64(per_meteor ^ 0x0051_2E00_0000_0000) >> 40) as f32)
                    / ((1u64 << 24) as f32);
                METEOR_SHOWER_SECONDARY_SIZE_MIN
                    + (METEOR_SHOWER_SECONDARY_SIZE_MAX - METEOR_SHOWER_SECONDARY_SIZE_MIN) * unit
            };
            // Meteor 0 lands exactly at the end of the warning window (the
            // announce-to-first-impact lead); the rest land at seeded staggers
            // in [0, METEOR_SHOWER_IMPACT_STAGGER_SECONDS] past it.
            let stagger_seconds = if k == 0 {
                0.0
            } else {
                let unit = ((splitmix64(per_meteor ^ 0x0057_A660_0000_0000) >> 40) as f32)
                    / ((1u64 << 24) as f32);
                unit * METEOR_SHOWER_IMPACT_STAGGER_SECONDS
            };
            let impact_tick =
                first_impact_tick.saturating_add((stagger_seconds * SERVER_TICK_RATE_HZ) as u64);
            meteors.push(ActiveMeteor {
                impact_position: site,
                impact_tick,
                trajectory_seed: splitmix64(per_meteor ^ 0x0074_2A6E_C702_0000),
                size,
                resolved: false,
                despawn_tick: impact_tick
                    .saturating_add((METEOR_SHOWER_DESPAWN_SECONDS * SERVER_TICK_RATE_HZ) as u64),
                crater_nodes: Vec::new(),
            });
        }
        self.begin_meteor_shower_event(meteors, warning_seconds)
    }

    /// Announce a SINGLE meteor at an EXPLICIT impact position, bypassing
    /// siting and the building-clearance check. Used only by `/meteor-here`,
    /// which drops the meteor exactly where the admin stands (so it can, by
    /// design, land on the caller and on buildings).
    fn begin_meteor_shower_at(
        &mut self,
        impact_position: Vec3Net,
        warning_seconds: f32,
        size: f32,
    ) -> Vec<ServerEnvelope> {
        let impact_tick = self
            .tick
            .saturating_add((warning_seconds.max(0.0) * SERVER_TICK_RATE_HZ) as u64);
        let trajectory_seed =
            splitmix64(self.chunk_manager.world_seed() ^ impact_tick ^ 0x00FA_11ED_57A8_0000);
        let meteors = vec![ActiveMeteor {
            impact_position,
            impact_tick,
            trajectory_seed,
            size,
            resolved: false,
            despawn_tick: impact_tick
                .saturating_add((METEOR_SHOWER_DESPAWN_SECONDS * SERVER_TICK_RATE_HZ) as u64),
            crater_nodes: Vec::new(),
        }];
        self.begin_meteor_shower_event(meteors, warning_seconds)
    }

    /// Register the active event's meteors and emit the single reliable
    /// announce broadcast carrying all of them. Shared by the sited path
    /// ([`Self::begin_meteor_shower_with_warning`]) and the placed path
    /// ([`Self::begin_meteor_shower_at`]).
    fn begin_meteor_shower_event(
        &mut self,
        meteors: Vec<ActiveMeteor>,
        warning_seconds: f32,
    ) -> Vec<ServerEnvelope> {
        bevy::log::info!(
            "meteor shower announced: {} meteors, first impact in {:.0}s ({})",
            meteors.len(),
            warning_seconds,
            meteors
                .iter()
                .map(|m| format!(
                    "({:.0}, {:.0}) size {:.2} tick {}",
                    m.impact_position.x, m.impact_position.z, m.size, m.impact_tick
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );

        self.meteor_shower.next_announce_tick = None;
        self.meteor_shower.meteors = meteors;

        vec![ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::MeteorShower {
                meteors: self.meteor_shower_strikes(),
            },
        }]
    }

    /// The wire payload for the current event: one [`MeteorStrike`] per live
    /// meteor (announce through crater despawn).
    fn meteor_shower_strikes(&self) -> Vec<MeteorStrike> {
        self.meteor_shower
            .meteors
            .iter()
            .map(|meteor| MeteorStrike {
                impact_position: meteor.impact_position,
                impact_tick: meteor.impact_tick,
                trajectory_seed: meteor.trajectory_seed,
                size: meteor.size,
            })
            .collect()
    }

    /// The announce envelope for a client that just connected while an event is
    /// live (announce through crater despawn), carrying every still-live
    /// meteor, or `None` if no event is active. Appended by the
    /// connection/welcome path so late joiners see the fireballs or craters
    /// immediately. The client keys the sky/craters on this one payload.
    pub(super) fn meteor_shower_announce_for(&self, client_id: ClientId) -> Option<ServerEnvelope> {
        if self.meteor_shower.meteors.is_empty() {
            return None;
        }
        Some(ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::MeteorShower {
                meteors: self.meteor_shower_strikes(),
            },
        })
    }

    /// Choose `count` clearance-safe impact points, evenly distributed by
    /// ANGULAR STRATIFICATION: the outer ring is split into `count` equal
    /// angular sectors (rotated by a seeded offset so events differ), and each
    /// meteor's site is sampled within its own sector. Building safety is
    /// guaranteed by SITING, not by a damage exemption: candidates are rejected
    /// within `METEOR_SHOWER_BUILDING_CLEARANCE_M` of ANY deployed entity
    /// (building pieces are deployed entities too), Tool Cupboard claim
    /// footprint cell, or ruin footprint, and additionally within
    /// `METEOR_SHOWER_INTER_SITE_SPACING_M` of an already-accepted site of this
    /// event. On a densely-built map where every candidate in a sector fails,
    /// fall back to that sector's candidate maximising the worst of the two
    /// clearances, so the meteor still lands as far from bases (and its
    /// siblings) as the map allows.
    ///
    /// Deterministic in `event_seed` so a test can assert the chosen sites.
    pub(super) fn select_meteor_shower_sites(&self, event_seed: u64, count: usize) -> Vec<Vec3Net> {
        let dims = self.save.map.chunk_dims();
        let bounds = PlayableBounds::from_dims(dims);
        // PlayableBounds is symmetric around the origin; inset by the margin.
        let max_x = bounds.max_x - METEOR_SHOWER_SITE_BOUNDS_MARGIN_M;
        let max_z = bounds.max_z - METEOR_SHOWER_SITE_BOUNDS_MARGIN_M;

        let playable_radius = bounds.max_x.max(1.0);
        let ring = playable_radius * METEOR_SHOWER_SITE_MIN_CENTER_DISTANCE_FRACTION;

        // Degenerate on a tiny world: no ring room. Fall back to the world edge.
        if count == 0 {
            return Vec::new();
        }
        if max_x <= 0.0 || max_z <= 0.0 || ring >= max_x.min(max_z) {
            return vec![Vec3Net::new(max_x.max(0.0), 0.0, 0.0); count];
        }

        // Precompute the structure obstacle set once: deployed-entity positions
        // (covers building pieces and placed deployables), claim footprint cells,
        // and ruin footprints.
        let obstacles = self.meteor_shower_obstacle_positions();

        let mut state = splitmix64(event_seed ^ 0x0053_1737_0000_0000);
        let mut next = || {
            state = splitmix64(state);
            ((state >> 40) as f32) / ((1u64 << 24) as f32)
        };

        let clearance_sq = METEOR_SHOWER_BUILDING_CLEARANCE_M * METEOR_SHOWER_BUILDING_CLEARANCE_M;
        let spacing_sq = METEOR_SHOWER_INTER_SITE_SPACING_M * METEOR_SHOWER_INTER_SITE_SPACING_M;

        // Seeded rotation so the sector split lands differently each event.
        let rotation = next() * std::f32::consts::TAU;
        let sector = std::f32::consts::TAU / count as f32;

        let mut sites: Vec<Vec3Net> = Vec::with_capacity(count);
        for k in 0..count {
            let sector_start = rotation + sector * k as f32;
            let mut chosen: Option<Vec3Net> = None;
            let mut best_fallback: Option<(Vec3Net, f32)> = None;
            for _ in 0..METEOR_SHOWER_SITE_CANDIDATES {
                let angle = sector_start + next() * sector;
                let (sin, cos) = angle.sin_cos();
                // Furthest radius along this bearing that stays inside the
                // inset square bounds.
                let rx = if cos.abs() > 1e-4 {
                    max_x / cos.abs()
                } else {
                    f32::MAX
                };
                let rz = if sin.abs() > 1e-4 {
                    max_z / sin.abs()
                } else {
                    f32::MAX
                };
                let r_max = rx.min(rz);
                if r_max <= ring {
                    continue;
                }
                // Area-uniform radius between the ring and the bound, so sites
                // do not clump against the inner ring.
                let r = (ring * ring + (r_max * r_max - ring * ring) * next()).sqrt();
                let (x, z) = (r * cos, r * sin);

                let nearest_structure_sq = nearest_obstacle_distance_sq(x, z, &obstacles);
                let nearest_site_sq = sites
                    .iter()
                    .map(|site| {
                        let dx = x - site.x;
                        let dz = z - site.z;
                        dx * dx + dz * dz
                    })
                    .fold(f32::MAX, f32::min);

                // Fallback quality: the worst of the two clearances, each
                // normalised to its own requirement, so the fallback trades
                // them off evenly.
                let quality =
                    (nearest_structure_sq / clearance_sq).min(nearest_site_sq / spacing_sq);
                if best_fallback.is_none_or(|(_, best)| quality > best) {
                    best_fallback = Some((Vec3Net::new(x, 0.0, z), quality));
                }
                // Accept the first candidate clear of every structure AND every
                // already-accepted sibling site.
                if nearest_structure_sq >= clearance_sq && nearest_site_sq >= spacing_sq {
                    chosen = Some(Vec3Net::new(x, 0.0, z));
                    break;
                }
            }
            sites.push(
                chosen
                    .or(best_fallback.map(|(position, _)| position))
                    .unwrap_or(Vec3Net::new(max_x.max(0.0), 0.0, 0.0)),
            );
        }
        sites
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

    /// Resolve every meteor whose (staggered) impact tick has arrived: apply
    /// the size-scaled Blast to players in the radius, deplete resource nodes
    /// in the radius, and scatter the meteorite crater cluster. Marks each
    /// meteor resolved so it never fires twice.
    pub(super) fn resolve_due_meteor_impacts(&mut self) -> Vec<ServerEnvelope> {
        let due: Vec<(usize, Vec3Net, u64, f32)> = self
            .meteor_shower
            .meteors
            .iter()
            .enumerate()
            .filter(|(_, meteor)| !meteor.resolved && self.tick >= meteor.impact_tick)
            .map(|(index, meteor)| {
                (
                    index,
                    meteor.impact_position,
                    meteor.trajectory_seed,
                    meteor.size,
                )
            })
            .collect();

        let mut envelopes = Vec::new();
        for (index, center, trajectory_seed, size) in due {
            let radius = METEOR_SHOWER_IMPACT_RADIUS_M * size;

            // Players inside the radius take Blast damage (armor applies; the
            // headliner's ground zero is lethal through any set). Routed
            // through the shared damage helper so Correction / knockback /
            // death all flow correctly.
            envelopes.extend(self.resolve_blast_on_players(
                center,
                radius,
                METEOR_SHOWER_IMPACT_PLAYER_DAMAGE * size,
                DamageKind::Blast,
            ));

            // Deplete every resource node inside the crater. Same depletion
            // path a final gather swing takes (removal + regrow schedule +
            // depleted broadcast) so clients play the shatter/fell death
            // effect.
            envelopes.extend(self.deplete_nodes_in_radius(center, radius));

            // Scatter the rich meteorite crater cluster inside the radius.
            let crater_nodes = self.spawn_meteor_shower_crater_nodes(center, trajectory_seed, size);

            if let Some(meteor) = self.meteor_shower.meteors.get_mut(index) {
                meteor.resolved = true;
                meteor.crater_nodes = crater_nodes;
            }
        }
        envelopes
    }

    /// Apply a spherical Blast to players only: linear falloff from `center` to
    /// zero at `radius`, per-player armor via `damage_after_armor`, then the
    /// shared post-hit tail (`apply_player_damage`). Ground zero at the
    /// headliner's `max_damage` is lethal through any current armor (the blast
    /// column caps well under 100%). Structure-damaging explosions are Phase 6;
    /// this is deliberately players-only and written so that generalisation is
    /// additive (Phase 6 adds structure/deployable passes beside this, reusing
    /// the same falloff math).
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
            // 1.0) at the headliner's max stays lethal through any set because
            // the blast column caps well below 100%.
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

    /// Scatter the rich meteorite crater cluster inside the size-scaled impact
    /// radius via the P5a runtime-spawn recipe. The size-1.0 headliner keeps
    /// the full `METEOR_SHOWER_CRATER_NODE_COUNT_MIN..=MAX` roll; smaller
    /// meteors scale the rolled count by `size` (minimum 1). Placement is
    /// seeded from `trajectory_seed` with a minimum spacing so crater nodes do
    /// not overlap. Returns the spawned node ids so cleanup can force-despawn
    /// any unmined ones later.
    fn spawn_meteor_shower_crater_nodes(
        &mut self,
        center: Vec3Net,
        trajectory_seed: u64,
        size: f32,
    ) -> Vec<ResourceNodeId> {
        let mut state = splitmix64(trajectory_seed ^ 0x0053_4841_5244_0000);
        let mut next = || {
            state = splitmix64(state);
            ((state >> 40) as f32) / ((1u64 << 24) as f32)
        };

        let span =
            METEOR_SHOWER_CRATER_NODE_COUNT_MAX.saturating_sub(METEOR_SHOWER_CRATER_NODE_COUNT_MIN);
        let rolled = METEOR_SHOWER_CRATER_NODE_COUNT_MIN
            + if span == 0 {
                0
            } else {
                (next() * (span + 1) as f32) as u32 % (span + 1)
            };
        // The headliner (size 1.0) keeps the full roll; smaller meteors carry
        // proportionally smaller windfalls, never zero.
        let count = ((rolled as f32 * size.clamp(0.0, 1.0)).round() as u32).max(1);

        let Some(kind) = NodeKind::from_definition_id(METEORITE_NODE_ID) else {
            return Vec::new();
        };

        // Keep crater nodes inside a slightly-inset ring so they sit within the
        // (size-scaled) crater, not on its rim.
        let scatter_radius =
            (METEOR_SHOWER_IMPACT_RADIUS_M * size * 0.8).max(METEOR_SHOWER_CRATER_NODE_SPACING_M);
        let world_seed = self.chunk_manager.world_seed();

        let mut placed: Vec<Vec3Net> = Vec::new();
        let mut spawned: Vec<ResourceNodeId> = Vec::new();
        // Give each crater node a bounded number of placement attempts so a tight
        // spacing can't loop forever.
        let mut attempts = 0u32;
        while (spawned.len() as u32) < count && attempts < count * 12 {
            attempts += 1;
            // Uniform disc sample: sqrt on the radius fraction so crater nodes don't
            // clump at the centre. Seated ON the size-scaled crater surface (the
            // client renders the raised-rim mound from the same shared profile),
            // sunk a touch so the boulder beds into the jittered mesh instead of
            // hovering where the visual surface dips below the analytic one.
            let r = scatter_radius * next().sqrt();
            let theta = next() * std::f32::consts::TAU;
            let position = Vec3Net::new(
                center.x + r * theta.cos(),
                (crate::world::crater_surface_height(r, size) - 0.15).max(0.0),
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

    /// Force-despawn each expired meteor's unmined crater nodes and drop it
    /// from the event. When the LAST meteor cleans up, the event slot frees
    /// and the next event is rolled. Broadcasts `ResourceNodeDepleted` per
    /// remaining crater node so clients remove the visual; untracks each
    /// cleanly (NO regrow, these were event spawns, not world nodes) via
    /// `untrack_resource_node`.
    pub(super) fn cleanup_expired_meteors(&mut self) -> Vec<ServerEnvelope> {
        let had_meteors = !self.meteor_shower.meteors.is_empty();
        let expired: Vec<ActiveMeteor> = {
            let tick = self.tick;
            let (expired, live): (Vec<ActiveMeteor>, Vec<ActiveMeteor>) = self
                .meteor_shower
                .meteors
                .drain(..)
                .partition(|meteor| meteor.resolved && tick >= meteor.despawn_tick);
            self.meteor_shower.meteors = live;
            expired
        };

        let mut envelopes = Vec::new();
        for meteor in &expired {
            envelopes.extend(self.despawn_crater_nodes(&meteor.crater_nodes));
        }

        // The event slot frees only when the LAST meteor has cleaned up; roll
        // the next event off a fresh index so it differs from the
        // just-finished event's window. The roll ANCHORS at the finished
        // event's own start (its impact minus the warning lead; this batch's
        // earliest impact is within one stagger of the event's first, close
        // enough on a minutes-scale window), NOT at this cleanup tick: the
        // interval constants mean "minutes between events", and anchoring at
        // cleanup would silently pad every gap by the ~10 minute crater
        // window. A minimum-interval roll can land before this tick; the
        // clamp keeps the next announce out of the past (the next event then
        // opens right after this cleanup).
        if had_meteors
            && self.meteor_shower.meteors.is_empty()
            && self.meteor_shower.next_announce_tick.is_none()
        {
            let warning_ticks = (METEOR_SHOWER_WARNING_SECONDS * SERVER_TICK_RATE_HZ) as u64;
            let event_start = expired
                .iter()
                .map(|meteor| meteor.impact_tick)
                .min()
                .unwrap_or(self.tick)
                .saturating_sub(warning_ticks);
            let next = roll_next_announce_tick(
                event_start,
                self.chunk_manager.world_seed(),
                self.tick.wrapping_add(1),
            )
            .max(self.tick);
            self.meteor_shower.next_announce_tick = Some(next);
        }
        envelopes
    }

    /// Remove + untrack any of `crater_nodes` still in the live map,
    /// broadcasting a depletion per removed node. A player may have mined one
    /// out already (which removed + untracked it via the gather path).
    fn despawn_crater_nodes(&mut self, crater_nodes: &[ResourceNodeId]) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        for id in crater_nodes {
            if self.remove_resource_node(*id).is_some() {
                self.chunk_manager.untrack_resource_node(*id);
                envelopes.push(ServerEnvelope {
                    target: DeliveryTarget::Broadcast,
                    message: ServerMessage::ResourceNodeDepleted { id: *id },
                });
            }
        }
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
        // larger than the blast that would otherwise reach it. The headliner's
        // size 1.0 is the maximum, so smaller meteors are strictly safer.
        // Enforced at compile time so a future balance edit that inverts them
        // fails the build, not just this test.
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
    fn shower_shape_constants_are_sane() {
        const {
            assert!(crate::game_balance::METEOR_SHOWER_COUNT_MIN >= 1);
            assert!(
                crate::game_balance::METEOR_SHOWER_COUNT_MAX
                    >= crate::game_balance::METEOR_SHOWER_COUNT_MIN
            );
            assert!(crate::game_balance::METEOR_SHOWER_SECONDARY_SIZE_MIN > 0.0);
            assert!(
                crate::game_balance::METEOR_SHOWER_SECONDARY_SIZE_MAX
                    >= crate::game_balance::METEOR_SHOWER_SECONDARY_SIZE_MIN
            );
            assert!(crate::game_balance::METEOR_SHOWER_SECONDARY_SIZE_MAX < 1.0);
            // Two headliner-size danger zones must fit inside the inter-site
            // spacing so sibling craters never merge.
            assert!(
                crate::game_balance::METEOR_SHOWER_INTER_SITE_SPACING_M
                    >= 2.0 * METEOR_SHOWER_IMPACT_RADIUS_M
            );
        }
    }

    #[test]
    fn scheduler_rolls_within_the_configured_window() {
        // The announce lands (interval - warning) out; the first impact lands
        // `interval` out. Check the derived announce tick falls inside the
        // min/max window.
        let now = 1_000u64;
        for seed in [1u64, 2, 3, 100, 9999] {
            let announce = roll_next_announce_tick(now, seed, 1);
            let announce_offset_secs = (announce - now) as f32 / SERVER_TICK_RATE_HZ;
            let impact_secs = announce_offset_secs + METEOR_SHOWER_WARNING_SECONDS;
            let minutes = impact_secs / 60.0;
            assert!(
                (METEOR_SHOWER_INTERVAL_MINUTES_MIN - 0.01
                    ..=METEOR_SHOWER_INTERVAL_MINUTES_MAX + 0.01)
                    .contains(&minutes),
                "seed {seed}: rolled {minutes} real minutes, outside \
                 [{METEOR_SHOWER_INTERVAL_MINUTES_MIN}, {METEOR_SHOWER_INTERVAL_MINUTES_MAX}]"
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
    fn meteor_count_stays_in_the_configured_range() {
        for seed in 0..256u64 {
            let count = roll_meteor_count(seed);
            assert!(
                (crate::game_balance::METEOR_SHOWER_COUNT_MIN
                    ..=crate::game_balance::METEOR_SHOWER_COUNT_MAX)
                    .contains(&count),
                "seed {seed} rolled {count}"
            );
        }
        // Both ends of the range are actually reachable.
        let counts: std::collections::HashSet<u32> = (0..256u64).map(roll_meteor_count).collect();
        assert!(counts.contains(&crate::game_balance::METEOR_SHOWER_COUNT_MIN));
        assert!(counts.contains(&crate::game_balance::METEOR_SHOWER_COUNT_MAX));
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

/// Integration tests against a live `GameServer`: multi-site selection with
/// real structures, per-meteor impact resolution (falloff, armor, lethality,
/// size scaling, node clearing, crater-node spawns), the announce resend to a
/// mid-event joiner, and the staggered force-despawn + cleanup lifecycle.
#[cfg(test)]
mod server_tests {
    use crate::{
        combat::DamageKind,
        game_balance::{
            METEOR_SHOWER_BUILDING_CLEARANCE_M, METEOR_SHOWER_COUNT_MAX, METEOR_SHOWER_COUNT_MIN,
            METEOR_SHOWER_CRATER_NODE_COUNT_MAX, METEOR_SHOWER_CRATER_NODE_COUNT_MIN,
            METEOR_SHOWER_IMPACT_RADIUS_M, METEOR_SHOWER_IMPACT_STAGGER_SECONDS,
            METEOR_SHOWER_INTER_SITE_SPACING_M, METEOR_SHOWER_SECONDARY_SIZE_MAX,
            METEOR_SHOWER_SECONDARY_SIZE_MIN,
        },
        items::{IRON_BOOTS_ID, IRON_CUIRASS_ID, IRON_GREAVES_ID, IRON_HELM_ID},
        protocol::{ItemStack, MAX_HEALTH, SERVER_TICK_RATE_HZ, ServerMessage, Vec3Net},
        resource_nodes::METEORITE_NODE_ID,
        server::test_support::{connect_named, place_building, server},
        world::{NodeKind, PlayableBounds},
    };

    /// Force a single meteor onto the server with an explicit impact site,
    /// tick, and size, so a test can drive the impact deterministically
    /// without waiting on the scheduler.
    fn force_meteor(
        server: &mut crate::server::GameServer,
        impact_position: Vec3Net,
        impact_tick: u64,
        size: f32,
    ) {
        server.meteor_shower.meteors.push(super::ActiveMeteor {
            impact_position,
            impact_tick,
            trajectory_seed: 0xABCD_1234 ^ impact_tick,
            size,
            resolved: false,
            despawn_tick: impact_tick + 1000,
            crater_nodes: Vec::new(),
        });
    }

    /// Spawn a coal node at `position` through the real spawn/track path.
    fn spawn_coal_node(
        server: &mut crate::server::GameServer,
        position: Vec3Net,
    ) -> crate::protocol::ResourceNodeId {
        let kind = NodeKind::from_definition_id(crate::resource_nodes::COAL_NODE_ID).unwrap();
        let id = server.allocate_resource_node_id();
        let node = crate::resource_nodes::spawn_resource_node(
            &crate::world::WorldResourceNodeSpawn::new(
                id,
                crate::resource_nodes::COAL_NODE_ID,
                position,
                0.0,
            ),
            Some(server.chunk_manager.world_seed()),
        )
        .unwrap();
        server.chunk_manager.track_resource_node(id, kind, position);
        server.insert_resource_node(id, node);
        id
    }

    #[test]
    fn event_spawns_a_full_shower_of_spaced_clear_sites() {
        // A moderately built world: the forced event must roll 4 or 5 meteors,
        // every site inside bounds, pairwise at least the inter-site spacing
        // apart, and each clear of every structure.
        let mut server = server();
        let _host = connect_named(&mut server, "Host");

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
        let obstacles = server.meteor_shower_obstacle_positions();
        assert!(!obstacles.is_empty(), "the test world must have structures");

        let _ = server.begin_meteor_shower_with_warning(30.0);

        let meteors = &server.meteor_shower.meteors;
        assert!(
            (METEOR_SHOWER_COUNT_MIN..=METEOR_SHOWER_COUNT_MAX).contains(&(meteors.len() as u32)),
            "rolled {} meteors",
            meteors.len()
        );

        let dims = server.save.map.chunk_dims();
        let bounds = PlayableBounds::from_dims(dims);
        let clearance_sq = METEOR_SHOWER_BUILDING_CLEARANCE_M * METEOR_SHOWER_BUILDING_CLEARANCE_M;
        let spacing_sq = METEOR_SHOWER_INTER_SITE_SPACING_M * METEOR_SHOWER_INTER_SITE_SPACING_M;
        for (i, meteor) in meteors.iter().enumerate() {
            let site = meteor.impact_position;
            assert!(
                bounds.contains(site.x, site.z),
                "meteor {i} site ({:.1}, {:.1}) out of bounds",
                site.x,
                site.z
            );
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
                "meteor {i} site within clearance of a structure (nearest {:.1} m)",
                nearest_sq.sqrt()
            );
            for (j, other) in meteors.iter().enumerate().skip(i + 1) {
                let d_sq = site.horizontal_distance_squared(other.impact_position);
                assert!(
                    d_sq >= spacing_sq,
                    "meteors {i} and {j} only {:.1} m apart (need {METEOR_SHOWER_INTER_SITE_SPACING_M})",
                    d_sq.sqrt()
                );
            }
        }
    }

    #[test]
    fn exactly_one_headliner_and_secondaries_stay_in_band() {
        let mut server = server();
        // A handful of distinct events (distinct ticks roll distinct event
        // seeds): each must carry exactly one size-1.0 meteor, the rest in the
        // secondary band, and every impact stagger within the window.
        for offset in [0u64, 1000, 2000, 5000, 12_345] {
            server.tick = offset;
            server.meteor_shower.meteors.clear();
            let _ = server.begin_meteor_shower_with_warning(30.0);
            let meteors = &server.meteor_shower.meteors;

            let headliners = meteors.iter().filter(|m| m.size == 1.0).count();
            assert_eq!(
                headliners, 1,
                "event at tick {offset} rolled {headliners} headliners"
            );
            for meteor in meteors {
                if meteor.size != 1.0 {
                    assert!(
                        (METEOR_SHOWER_SECONDARY_SIZE_MIN..=METEOR_SHOWER_SECONDARY_SIZE_MAX)
                            .contains(&meteor.size),
                        "secondary size {} out of band",
                        meteor.size
                    );
                }
            }

            // Staggering: the first impact lands exactly at the warning window,
            // every other within the stagger window past it.
            let first_impact = offset + (30.0 * SERVER_TICK_RATE_HZ) as u64;
            let max_stagger = (METEOR_SHOWER_IMPACT_STAGGER_SECONDS * SERVER_TICK_RATE_HZ) as u64;
            assert!(
                meteors.iter().any(|m| m.impact_tick == first_impact),
                "one meteor lands exactly at the end of the warning window"
            );
            for meteor in meteors {
                assert!(
                    (first_impact..=first_impact + max_stagger).contains(&meteor.impact_tick),
                    "impact tick {} outside the stagger window",
                    meteor.impact_tick
                );
            }
        }
    }

    #[test]
    fn site_selection_falls_back_when_saturated() {
        // Blanket the outer ring with structures so NO candidate clears; the
        // selector must still return in-bounds points (never panic, never
        // fail to site the event).
        let mut server = server();
        let dims = server.save.map.chunk_dims();
        let bounds = PlayableBounds::from_dims(dims);
        let radius = bounds.max_x;

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

        let sites = server.select_meteor_shower_sites(7, 5);
        assert_eq!(sites.len(), 5);
        for site in sites {
            assert!(site.x.is_finite() && site.z.is_finite());
            assert!(bounds.contains(site.x, site.z), "fallback stays in bounds");
        }
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
            client.controller.position = center; // standing at ground zero
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
            "the headliner's ground zero must be lethal through any armor set"
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

        // A couple of resource nodes inside the crater and one just outside.
        let inside = spawn_coal_node(&mut server, Vec3Net::new(center.x + 3.0, 0.0, center.z));
        let outside = spawn_coal_node(
            &mut server,
            Vec3Net::new(
                center.x + METEOR_SHOWER_IMPACT_RADIUS_M + 10.0,
                0.0,
                center.z,
            ),
        );

        let now = server.tick;
        force_meteor(&mut server, center, now, 1.0);
        let envelopes = server.resolve_due_meteor_impacts();

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
        let crater_nodes = server.meteor_shower.meteors[0].crater_nodes.clone();
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
    fn small_meteor_scales_down_blast_and_loot() {
        // The same event geometry at size 0.4 vs the headliner: the small
        // meteor's blast at a fixed distance deals less, misses players the
        // headliner would hit, and its crater cluster is smaller.
        let size = 0.4_f32;

        // Damage at ground zero scales with size.
        let mut server = server();
        let center = Vec3Net::new(0.0, 0.0, 0.0);
        let victim = connect_named(&mut server, "Victim");
        server.clients.get_mut(&victim).unwrap().controller.position = center;
        server.clients.get_mut(&victim).unwrap().controller.health = MAX_HEALTH;
        let now = server.tick;
        force_meteor(&mut server, center, now, size);
        let _ = server.resolve_due_meteor_impacts();
        let small_loss = MAX_HEALTH - server.clients[&victim].controller.health;
        assert!(
            small_loss > 0.0,
            "ground zero of a small meteor still hurts"
        );
        assert!(
            small_loss < crate::game_balance::METEOR_SHOWER_IMPACT_PLAYER_DAMAGE * size + 1.0,
            "small-meteor ground zero caps at the size-scaled damage, got {small_loss}"
        );

        // A player inside the headliner radius but outside the scaled radius is
        // untouched by the small meteor.
        let mut server = crate::server::test_support::server();
        let bystander = connect_named(&mut server, "Bystander");
        let between = METEOR_SHOWER_IMPACT_RADIUS_M * (size + 1.0) * 0.5;
        server
            .clients
            .get_mut(&bystander)
            .unwrap()
            .controller
            .position = Vec3Net::new(between, 0.0, 0.0);
        server
            .clients
            .get_mut(&bystander)
            .unwrap()
            .controller
            .health = MAX_HEALTH;
        let now = server.tick;
        force_meteor(&mut server, Vec3Net::new(0.0, 0.0, 0.0), now, size);
        let _ = server.resolve_due_meteor_impacts();
        assert_eq!(
            server.clients[&bystander].controller.health, MAX_HEALTH,
            "outside the size-scaled radius the small blast does not reach"
        );

        // Crater loot: with the SAME trajectory seed the rolled base count is
        // identical, so the size-scaled count is no larger and the scatter
        // stays inside the scaled radius.
        let mut server = crate::server::test_support::server();
        let center = Vec3Net::new(150.0, 0.0, -150.0);
        let now = server.tick;
        server.meteor_shower.meteors.push(super::ActiveMeteor {
            impact_position: center,
            impact_tick: now,
            trajectory_seed: 0xFEED_BEEF,
            size,
            resolved: false,
            despawn_tick: now + 1000,
            crater_nodes: Vec::new(),
        });
        let _ = server.resolve_due_meteor_impacts();
        let small_nodes = server.meteor_shower.meteors[0].crater_nodes.clone();

        let mut server = crate::server::test_support::server();
        let now = server.tick;
        server.meteor_shower.meteors.push(super::ActiveMeteor {
            impact_position: center,
            impact_tick: now,
            trajectory_seed: 0xFEED_BEEF,
            size: 1.0,
            resolved: false,
            despawn_tick: now + 1000,
            crater_nodes: Vec::new(),
        });
        let _ = server.resolve_due_meteor_impacts();
        let full_nodes = server.meteor_shower.meteors[0].crater_nodes.clone();

        assert!(
            !small_nodes.is_empty(),
            "a small meteor still drops at least one crater node"
        );
        assert!(
            small_nodes.len() < full_nodes.len(),
            "size 0.4 must scale the cluster down: {} vs {}",
            small_nodes.len(),
            full_nodes.len()
        );
    }

    #[test]
    fn meteors_resolve_at_their_own_staggered_ticks() {
        let mut server = server();
        let now = server.tick;
        let early = Vec3Net::new(200.0, 0.0, 0.0);
        let late = Vec3Net::new(-200.0, 0.0, 0.0);
        force_meteor(&mut server, early, now + 10, 1.0);
        force_meteor(&mut server, late, now + 500, 0.6);

        // At the first impact tick only the first meteor resolves.
        server.tick = now + 10;
        let _ = server.resolve_due_meteor_impacts();
        assert!(server.meteor_shower.meteors[0].resolved);
        assert!(!server.meteor_shower.meteors[1].resolved);

        // The second resolves once its own tick arrives.
        server.tick = now + 500;
        let _ = server.resolve_due_meteor_impacts();
        assert!(server.meteor_shower.meteors[1].resolved);
    }

    #[test]
    fn event_slot_rerolls_only_after_the_last_meteor_cleans_up() {
        let mut server = server();
        server.meteor_shower.next_announce_tick = None;
        let now = server.tick;
        force_meteor(&mut server, Vec3Net::new(200.0, 0.0, 0.0), now, 1.0);
        force_meteor(&mut server, Vec3Net::new(-200.0, 0.0, 0.0), now + 300, 0.5);
        let _ = server.resolve_due_meteor_impacts();
        server.tick = now + 300;
        let _ = server.resolve_due_meteor_impacts();
        let first_nodes = server.meteor_shower.meteors[0].crater_nodes.clone();
        assert!(!first_nodes.is_empty());

        // Past the FIRST meteor's despawn: it cleans up (nodes despawned),
        // but the event slot stays occupied and no new event is rolled.
        server.tick = server.meteor_shower.meteors[0].despawn_tick;
        let envelopes = server.cleanup_expired_meteors();
        for id in &first_nodes {
            assert!(
                server.resource_nodes.get(id).is_none(),
                "crater node {id} despawned with its meteor"
            );
            assert!(
                server.chunk_manager.node_chunk(*id).is_none(),
                "crater node {id} untracked from the chunk index"
            );
            assert!(
                envelopes.iter().any(|e| matches!(
                    &e.message,
                    ServerMessage::ResourceNodeDepleted { id: broadcast } if broadcast == id
                )),
                "each despawned crater node must broadcast a depletion"
            );
        }
        assert_eq!(
            server.meteor_shower.meteors.len(),
            1,
            "the later meteor is still live"
        );
        assert!(
            server.meteor_shower.next_announce_tick.is_none(),
            "no reroll while a meteor is still live"
        );

        // Past the LAST meteor's despawn: the event slot frees and the next
        // event is rolled.
        server.tick = server.meteor_shower.meteors[0].despawn_tick;
        let _ = server.cleanup_expired_meteors();
        assert!(
            server.meteor_shower.meteors.is_empty(),
            "event cleared after the last cleanup"
        );
        assert!(
            server.meteor_shower.next_announce_tick.is_some(),
            "the last cleanup must roll the next event"
        );
    }

    #[test]
    fn meteor_shower_here_lands_at_the_callers_position_bypassing_siting() {
        // `/meteor-here` must place ground zero exactly at the admin's feet,
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

        let envelopes = server.command_meteor_shower_here(admin, &["8", "0.5"]);
        let announced = envelopes.iter().find_map(|e| match &e.message {
            ServerMessage::MeteorShower { meteors } => Some(meteors.clone()),
            _ => None,
        });
        let announced = announced.expect("the placed meteor is announced");
        assert_eq!(announced.len(), 1, "/meteor-here stays a single meteor");
        assert_eq!(
            announced[0].impact_position,
            Vec3Net::new(stand.x, 0.0, stand.z),
            "the placed meteor must land at the caller's XZ, not a sited point"
        );
        assert_eq!(announced[0].size, 0.5, "the size argument threads through");
        assert_eq!(server.meteor_shower.meteors.len(), 1);
        assert_eq!(
            server.meteor_shower.meteors[0].impact_position,
            Vec3Net::new(stand.x, 0.0, stand.z)
        );
    }

    #[test]
    fn meteor_shower_here_clamps_the_size_argument() {
        let mut server = server();
        let admin = connect_named(&mut server, "Admin");
        server.clients.get_mut(&admin).unwrap().is_admin = true;
        let _ = server.command_meteor_shower_here(admin, &["8", "7.5"]);
        assert_eq!(
            server.meteor_shower.meteors[0].size, 1.0,
            "oversize clamps to 1.0"
        );
        server.meteor_shower.meteors.clear();
        let _ = server.command_meteor_shower_here(admin, &["8", "0.01"]);
        assert_eq!(
            server.meteor_shower.meteors[0].size, 0.2,
            "undersize clamps to 0.2"
        );
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
            server.meteor_shower.meteors.is_empty(),
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
        // late-joiner resend) with EVERY live meteor, so a player who joins
        // mid-shower sees the whole thing.
        let mut server = server();
        let now = server.tick;
        let a = Vec3Net::new(100.0, 0.0, 100.0);
        let b = Vec3Net::new(-180.0, 0.0, 40.0);
        force_meteor(&mut server, a, now + 500, 1.0);
        force_meteor(&mut server, b, now + 700, 0.6);

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
            ServerMessage::MeteorShower { meteors } => Some(meteors.clone()),
            _ => None,
        });
        let announce = announce.expect("a mid-event joiner receives the resend");
        assert_eq!(announce.len(), 2, "the resend carries every live meteor");
        assert!(announce.iter().any(|m| m.impact_position == a));
        assert!(
            announce
                .iter()
                .any(|m| m.impact_position == b && m.size == 0.6)
        );
    }
}
