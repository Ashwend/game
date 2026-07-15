---
title: Meteor shower event
owns: The meteor shower world event end to end; the announce-only wire contract, the shared deterministic trajectory math, the server scheduler/siting/impact/cleanup engine, and the METEOR_SHOWER_* balance family.
when_to_read: Before touching the meteor schedule, siting, blast, crater cluster, trajectory math, crater profile, or the announce payload; or when client and server disagree about where or when a meteor lands.
sources:
  - src/world/meteor_shower.rs - meteor_world_state, crater_surface_height, METEOR_FLIGHT_SECONDS
  - src/server/meteor_shower.rs - MeteorShowerState, tick_world_events, select_meteor_shower_site, resolve_blast_on_players, meteor_shower_announce_for
  - src/protocol/messages.rs - ServerMessage::MeteorShower, delivery
  - src/app/state/runtime.rs - MeteorShowerEvent, ClientRuntime::meteor_shower, tick_world_time
  - src/game_balance.rs - METEOR_SHOWER_* constants
related:
  - docs/ui-and-client.md - the client chrome, countdown HUD, danger warning, map marker, sky/crater VFX
  - docs/networking.md - ClientMessage/ServerMessage inventory, channels, handshake
  - docs/server-authority.md - GameServer tick order and ServerEnvelope routing
  - docs/pvp-combat.md - damage_after_armor and the shared apply_player_damage tail
  - docs/movement.md - the crater's analytic movement floor
---

# Meteor shower event

> When to read this: before changing the meteor schedule, siting, blast, crater cluster, or trajectory math, or when debugging where/when a meteor lands. Source of truth: `src/server/meteor_shower.rs` (authority) and `src/world/meteor_shower.rs` (shared math). The client chrome is owned by [docs/ui-and-client.md](ui-and-client.md).

The meteor shower is a recurring server-scheduled world event: a meteor is announced ten real minutes ahead, streaks in over the final 45 seconds, strikes a clearance-checked site, damages players and clears resource nodes in the blast radius, and scatters a contested cluster of rich meteorite nodes that despawn if unmined. The whole design rests on one wire message plus pure shared math; nothing about the meteor is streamed.

## Announce-only wire contract

The entire event ships as one broadcast: `ServerMessage::MeteorShower { impact_position, impact_tick, trajectory_seed }` (src/protocol/messages.rs - ServerMessage::MeteorShower), sent at T minus `METEOR_SHOWER_WARNING_SECONDS` (600 s) when the scheduled announce tick arrives. It rides the reliable channel (a dropped announce would leave a player blind to an incoming meteor, see `ServerMessage::delivery`), and `meteor_shower_announce_for` (src/server/meteor_shower.rs - meteor_shower_announce_for) resends the identical payload to any client that connects while the event is alive, announce through crater despawn, appended by the connection/welcome path (src/server/connection.rs), so late joiners see the fireball or crater immediately.

Everything downstream (fireball position, countdown, danger warning, map marker, crater, cleanup) is a deterministic function of that payload plus the client's own authoritative-clock estimate. The meteor is never per-tick replicated: like `ServerMessage::WorldTime`, where clients integrate the clock locally between sparse authoritative signals, the meteor is neither an event stream nor per-entity state, it is a function of time. See the replicated-state rules in CLAUDE.md before considering a replicated meteor entity.

## Shared trajectory math

`src/world/meteor_shower.rs` is pure and dependency-light (only `Vec2`/`Vec3` math plus the shared `splitmix64`), so the client renderer and the determinism tests call the same function.

- `METEOR_FLIGHT_SECONDS` = 45.0: the visible flight window. The committed arc spans exactly this window, from the entry point at `impact_tick - 45 s` to the impact point at `impact_tick`.
- **Entry point** is seeded off `trajectory_seed` through `splitmix64` with per-axis salts (`seeded_unit`): a compass azimuth, a horizontal distance of 5000 to 7000 m out, and an altitude of 2500 to 3500 m up, so the object enters from far beyond the world edge.
- **Descent** follows a quadratic Bezier (the control point bows `PATH_BOW_FRACTION` = 0.18 of the entry altitude off the straight chord) reparametrised by a quadratic ease (`u = p * p`), so the final approach visibly accelerates.
- **Velocity** is the analytic derivative of that path (chain rule through the ease), continuous by construction; no finite-difference jitter.
- `meteor_world_state(impact_position, impact_tick, trajectory_seed, estimated_tick)` returns `None` outside the flight window (before `remaining <= METEOR_FLIGHT_SECONDS` and at/after `impact_tick`); inside it, the object is on one committed arc that ends exactly at the impact point. `estimated_tick` is FRACTIONAL on purpose: evaluating at whole 20 Hz ticks quantises the descent into 50 ms position steps that stutter at render frame rates (`fractional_ticks_move_the_meteor_between_whole_ticks` guards this).
- The module also owns the shared crater surface profile: `crater_surface_height` plus the `CRATER_*` constants (bowl radius 6.5 m, rim end 9.5 m, skirt 14.5 m, rim height 0.85 m). The client crater mesh, the movement collider's analytic floor (`src/controller/grid.rs`, see [docs/movement.md](movement.md)), and the server's crater-node seating all sample this one function.

## Server engine

`src/server/meteor_shower.rs` owns everything authoritative. `MeteorShowerState` on `GameServer` holds `next_announce_tick: Option<u64>` plus `active: Option<ActiveMeteorShower>` and is advanced by `tick_world_events` from `GameServer::tick` (src/server/tick.rs), ordered schedule -> announce -> impact -> cleanup.

- **Scheduler.** `roll_next_announce_tick` rolls the next event uniformly in `METEOR_SHOWER_INTERVAL_DAYS_MIN..=MAX` (2 to 4 in-game days) converted to real server ticks via `REAL_SECONDS_PER_DAY` at cycle multiplier 1. The schedule is real time by design: the admin `/time-speed` cheat accelerates only the day/night visual cycle, so spinning the sun does not pull meteors closer together. The announce lands `METEOR_SHOWER_WARNING_SECONDS` before the rolled impact.
- **Siting.** `select_meteor_shower_site` samples up to `METEOR_SHOWER_SITE_CANDIDATES` = 48 seeded candidates inside `PlayableBounds` inset by `METEOR_SHOWER_SITE_BOUNDS_MARGIN_M` = 20 m, restricted to the outer ring beyond `METEOR_SHOWER_SITE_MIN_CENTER_DISTANCE_FRACTION` = 0.35 of the playable radius. A candidate is rejected within `METEOR_SHOWER_BUILDING_CLEARANCE_M` = 60 m of ANY deployed entity (building pieces included), Tool Cupboard claim footprint cell, or ruin footprint. Building safety is guaranteed by siting, never by a damage exemption, so the clearance must exceed `METEOR_SHOWER_IMPACT_RADIUS_M` = 18 m (const-asserted in `clearance_exceeds_impact_radius`). If every candidate fails on a saturated map, the selector falls back to the sampled point that maximises distance to the nearest structure.
- **Impact.** `resolve_meteor_shower_impact` resolves exactly once when the clock reaches `impact_tick`. Players inside the radius take Blast damage through `resolve_blast_on_players`: linear falloff from `METEOR_SHOWER_IMPACT_PLAYER_DAMAGE` = 250 at ground zero to 0 at the radius edge, per-player armor via `damage_after_armor`, then the shared `apply_player_damage` tail (knockback, correction, death/loot flow, see [docs/pvp-combat.md](pvp-combat.md)); ground zero is lethal through any current armor set. `deplete_nodes_in_radius` removes every resource node in the radius through the normal depletion path (regrow schedule plus a `ResourceNodeDepleted` broadcast so clients play the shatter/fell effect). `spawn_meteor_shower_crater_nodes` then scatters `METEOR_SHOWER_CRATER_NODE_COUNT_MIN..=MAX` (3 to 6) rich meteorite nodes (`METEORITE_NODE_ID`), seeded off the trajectory seed, seated on `crater_surface_height`, with `METEOR_SHOWER_CRATER_NODE_SPACING_M` = 2.5 m minimum spacing.
- **Cleanup.** At `despawn_tick` (`impact_tick` + `METEOR_SHOWER_DESPAWN_SECONDS` = 600 s) `cleanup_meteor_shower` force-despawns any still-unmined crater nodes (broadcasting `ResourceNodeDepleted`, untracking with NO regrow; these were event spawns, not world nodes), clears the event, and rolls the next announce.
- **Deliberately not persisted.** `MeteorShowerState` is transient: on world load the scheduler rolls a fresh next event and an in-flight event does not survive a restart. This keeps the save format untouched (no version bump).
- **Admin commands.** `/meteor_shower [warning_seconds]` (default 30) clears any live event and forces a fresh sited one; `/meteor_shower-here [warning_seconds]` (default 8) drops the impact exactly at the caller's position, bypassing siting and the clearance check by design. Both are admin-gated.

## Client presentation

The announce is stored on `runtime.meteor_shower` as an `MeteorShowerEvent` (src/app/state/runtime.rs; the receive path is in src/app/systems/network.rs, which also distinguishes a genuinely new announce from a late-join resend by comparing `impact_tick`). `ClientRuntime::tick_world_time` drops the event once the crater window closes on the local clock and installs/clears the crater's analytic movement floor on the block grid. Every consumer reads only `runtime.meteor_shower` plus the clock estimate:

- **Fireball.** `MeteorVisual` body layers plus the segmented trail, driven by `update_meteor_sky_system` (src/app/scene/meteor_sky.rs - MeteorVisual, update_meteor_sky_system) evaluating `meteor_world_state` at the fractional `runtime.server_tick_precise()`. The fireball is a true world-anchored object, not a disc on the sky dome; while beyond `METEOR_PROXY_DISTANCE` it renders on a far-plane proxy along the true direction.
- **Impact site.** Crater mesh, site fires, the one-time rock blast, pre-armed boom/flyby audio, and the distance-scaled rumble and camera kick (src/app/scene/meteor_shower.rs).
- **HUD.** Countdown pill plus the escalating evacuation warning while the player is inside `METEOR_SHOWER_DANGER_RADIUS_M` = 60 m (src/app/ui/hud/meteor_shower.rs - meteor_shower_hud).
- **Map.** Temporary pulsing impact marker (src/app/ui/world_map.rs - draw_meteor_shower_marker).

Per-surface detail (draw order, crater mesh construction, audio cue timing) lives in the "Meteor shower: countdown HUD, map marker, and sky/ground VFX" section of [docs/ui-and-client.md](ui-and-client.md); do not duplicate it here.

## Balance

The whole `METEOR_SHOWER_*` family lives in the "meteor shower event" block of `src/game_balance.rs`: interval days 2 to 4, warning 600 s, building clearance 60 m, impact radius 18 m, danger radius 60 m, ground-zero player damage 250, crater-node count 3 to 6, spacing 2.5 m, despawn 600 s, site fires 100 s burn + 30 s fade, 48 site candidates, center-distance fraction 0.35, bounds margin 20 m. Blast knockback shares `EXPLOSION_KNOCKBACK_SPEED` with the explosives family. Per invariant 4 in CLAUDE.md, new meteor tuning goes there, never inline.
