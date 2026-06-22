---
title: Profiling and performance discipline
owns: The performance workflow: F2 perf HUD, ./cli profile Chrome-trace capture, the Perfetto trace_processor SQL recipe, named server spans, and the canonical perf-bug fixes.
when_to_read: Before optimizing, when chasing a frame spike, or before adding any O(live-entities)-per-tick work.
sources:
  - src/app/ui/hud.rs - perf overlay render (perf_stats_ui, frame_time_stats)
  - src/app/systems/input/menu_toggles.rs - toggle_perf_stats_system (F2 toggle)
  - src/protocol/messages.rs - PerfStatsSnapshot, ServerMessage::PerfStats
  - src/server/tick.rs - server_tick / chunk_manager_tick spans, PerfStats broadcast
  - src/server.rs - PERF_STATS_BROADCAST_INTERVAL_TICKS
  - src/app/systems/items/resource_nodes/mod.rs - apply_resource_nodes_system (event-driven reconciliation pattern)
  - src/app/systems/deployables/mod.rs - maintain_world_grid_system (event-gated grid)
  - src/app/systems/input/cursor.rs - compare-before-assign fix
  - src/app/systems/display.rs - target_limiter (framepace Manual vs Auto)
  - src/net/host/mirror.rs - named sync_* spans
  - Cargo.toml - profile / dev-fast features, opt-level overrides
related:
  - docs/replication.md - the Changed<T>-lies rule and the event-driven reconciliation pattern this doc points at
  - docs/build-and-dev.md - ./cli subcommands (profile, dev, dev-fast)
  - docs/chunks-and-aoi.md - the AoI visible-node floor that drives per-frame client cost
  - projects/resource-node-instancing.md - unimplemented proposal to cut the render-pipeline floor
---

# Profiling and performance discipline

> When to read this: before optimizing, when chasing a frame spike, or before adding any O(live-entities)-per-tick work. Source of truth: `src/app/ui/hud.rs`, `src/app/systems/input/menu_toggles.rs`, the `profile` Cargo feature in `Cargo.toml`. Canonical invariants live in CLAUDE.md.

The binary/package is `ashwend` (`Cargo.toml` `[package] name`). Several paths in the older profiling notes are stale: `protocol`, `items/resource_nodes`, and `deployables` are all directories now, not single files. The current paths are in the `sources` front-matter above; verify a path exists before trusting any file:line you read elsewhere.

Workflow tiers, cheapest first:
1. F2 perf HUD: no rebuild, always available, good enough for a first-pass yes/no on "is there a spike".
2. `./cli profile` Chrome trace + `trace_processor` SQL: attributes a spike to a specific system or render pass.
3. The canonical fixes below: the bug shapes a trace usually points at, each already fixed once in this codebase.

## F2 perf HUD

The overlay toggles on **F2** in `src/app/systems/input/menu_toggles.rs` (`toggle_perf_stats_system`), which flips `settings.hud.show_perf_stats`. F2 is hardcoded and deliberately not rebindable (it sits in the debug-toggle bucket; see the comment on the system). The toggle no-ops unless the screen is `InGame` with no pause/chat/dialog modal open.

The overlay is *rendered* in `src/app/ui/hud.rs` (`perf_stats_ui`), gated by `settings.hud.show_perf_stats`. If F2 does not toggle, the binding is the menu_toggles file; if the panel is blank or stale, the consumer is hud.rs. Do not look for the toggle in hud.rs.

Fields rendered (`perf_stats_ui`, `frame_time_stats`):

| field | meaning |
|---|---|
| `FPS` | Smoothed FPS over the diagnostic history window. Hides spikes. |
| `Frame` | Most recent single frame in ms. Bounces frame to frame. |
| `p99 frame` | 99th percentile over the frame-time history window. Diverging from `Frame` means periodic stalls even when the average looks fine. |
| `max frame` | Worst single frame in the window. |
| `Chunk` | Player's chunk `(x, z)` plus its biome classification label. From the server. |
| `Loaded` | `loaded_chunks` (server). |
| `Live nodes` | `live_nodes`, total live resource nodes server-side. |
| `Visible` | `aoi_visible_nodes`, nodes in the player's AoI ring. This is what drives most per-frame client work. |
| `Regrow queue` | `pending_regrows`, scheduled node respawns. |

`FPS`, `Frame`, `p99 frame`, and `max frame` come from `FrameTimeDiagnosticsPlugin::new(480)`, which is **always on** (`src/app.rs`, not behind the `profile` feature). The 480-sample window is ~1 s at 500 FPS, ~4 s at 120 FPS. `frame_time_stats` full-sorts the samples and reads `p99_index = ((n as f64 * 0.99) as usize).min(n - 1)`; full sort is fine at ~480 samples.

The `Chunk`/`Loaded`/`Live nodes`/`Visible`/`Regrow queue` block comes from the server in `PerfStatsSnapshot` (`src/protocol/messages.rs`), delivered as `ServerMessage::PerfStats`. Until the first snapshot arrives the block reads "waiting for server…". The struct has seven fields: `loaded_chunks`, `live_nodes`, `pending_regrows`, `aoi_visible_nodes` (all `u32`), plus `player_chunk_x`, `player_chunk_z` (`i32`), and `player_classification` (`PerfClassificationId`).

Rule of thumb: if `p99 frame` is approximately `Frame`, the game is steady, stop here. If `p99 frame` >> `Frame`, capture a trace.

## ./cli profile (Chrome trace)

`./cli profile` runs `close_existing_game` (a `pkill` of `Ashwend` / `ashwend` / `target/debug/ashwend`) then `cargo run --features profile --bin ashwend -- client "$@"`. `BEVY_ASSET_ROOT` is exported globally at the top of `cli` for **every** subcommand, not by `profile` specifically, so launching `cargo run` directly works too as long as that env var is set.

The `profile` feature (`Cargo.toml`) is just `["bevy/trace_chrome"]`. The extra diagnostics (`LogDiagnosticsPlugin`, `EntityCountDiagnosticsPlugin { max_history_length: 480 }`, `SystemInformationDiagnosticsPlugin`) are added separately in `src/app.rs` behind `#[cfg(feature = "profile")]`, so enabling the feature does pull them in. `bevy/trace_chrome` writes `trace-<unix-ms>.json` to the working directory **on clean exit only**.

```bash
# Terminal 1: local dedicated server (reproduces the multiplayer code path
# without WAN jitter; preferred over a remote test).
./cli server --bind 127.0.0.1:7777

# Terminal 2: client with tracing.
./cli profile --connect 127.0.0.1:7777
```

Stand still for ~30 s, then **quit through the menu**, not Ctrl+C. The trace flushes only on clean exit. Output lands in the project root.

Caveats:
- The trace is large (~100 MB per second of capture, multi-GB for 30 s) and writing it has real overhead. If a problem only reproduces under `--features profile`, the profile overhead may *be* the problem; always retest without it.
- `rm trace-*.json` from the project root after; keep at most the last one or two.
- The Chrome trace format records **wall time only**, not CPU time. You cannot tell from a span whether a thread was computing or blocked on a syscall/mutex/GPU. Use which threads run concurrently as circumstantial evidence.
- For micro-optimization measurement, prefer the F2 HUD; `--features profile` itself costs a few percent.

## Loading and querying the trace (trace_processor)

Use Perfetto's local SQL engine. It parses the Chrome JSON and exposes spans as a SQLite-shaped `slice` table.

```bash
# One-time install.
curl -LO https://get.perfetto.dev/trace_processor
chmod +x ./trace_processor && mv ./trace_processor /tmp/trace_processor

# Run a query file against a trace.
/tmp/trace_processor query -f /tmp/some_query.sql /path/to/trace.json

# Strip the loading-progress noise.
/tmp/trace_processor query -f /tmp/some_query.sql /path/to/trace.json 2>&1 \
  | grep -v "^Loading\|^\[\|Trace health\|Data losses\|misplaced\|^column"
```

A multi-GB trace loads in 10-20 s; queries then run in milliseconds.

`slice` columns we use:

| column | meaning |
|---|---|
| `id` | row id (unused in our queries) |
| `ts` | start timestamp, **nanoseconds** since trace start |
| `dur` | duration, **nanoseconds** |
| `name` | span label: `"system: name=\"…\""` for Bevy systems, `"main app: "` for the per-frame wrapper, `"schedule: name=… "` for schedule wrappers, and our named server spans below |
| `track_id` | thread/track. `0` = main thread, `3` = render thread, `4-8` = task-pool workers |

`ts/1e6` and `dur/1e6` give milliseconds; `/1e9` gives seconds. **Bevy's `tracing` format leaves a trailing space in span names**: filter for `'main app: '` and `'schedule: name=Main '`, not `'main app:'`.

### Named server spans

The server hot path carries `info_span!` markers; grep the trace for these to attribute server-side cost cleanly instead of pattern-matching system names:

- `server_tick`, `chunk_manager_tick`: `src/server/tick.rs` (`GameServer::tick`).
- `host_fixed_tick`, `route_envelopes`: `src/net/host.rs`.
- `sync_resource_node_entities`, `sync_dropped_item_entities`, `sync_deployable_entities`, `sync_player_entities`, `sync_loot_bag_entities`: the mirror-sync systems in `src/net/host/mirror.rs`. These are the per-component replication mirror writes; a fat one usually means an O(N) walk that should be event-driven.

### Useful queries

Save under `/tmp/*.sql`. These four pinpoint nearly every frame-pacing problem this codebase has hit.

**Frame-time distribution.** Bins every per-frame `main app:` duration. A **bimodal** shape (clean fast peak + separate slow peak) is the signature of a periodic system adding work to ~1-in-N frames.

```sql
WITH frames AS (
  SELECT dur/1e6 AS frame_ms
  FROM slice
  WHERE name = 'main app: ' AND track_id = 0
    AND ts > 3000000   -- skip ~3 ms of startup
)
SELECT CAST(frame_ms AS INTEGER) || ' ms' AS bucket, COUNT(*) AS n_frames
FROM frames
GROUP BY CAST(frame_ms AS INTEGER)
ORDER BY CAST(frame_ms AS INTEGER);
```

**Top contributors on slow frames.** Sum each suspect's cost across the slow window. Bucket tightly (e.g. 6-9 ms) and the biggest non-zero column is the culprit. `LIKE '%term%'` is forgiving and catches a system's `system_commands` deferred-apply counterpart too.

```sql
WITH main_frames AS (
  SELECT id, ts AS frame_start, ts + dur AS frame_end, dur
  FROM slice
  WHERE name = 'main app: ' AND track_id = 0
    AND ts > 3000000 AND dur BETWEEN 6000000 AND 9000000
)
SELECT
  ROUND(f.dur/1e6, 2) AS frame_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%lightyear%'            THEN s.dur ELSE 0 END)/1e6, 2) AS lightyear_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%Replication%'          THEN s.dur ELSE 0 END)/1e6, 2) AS replication_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%bevy_egui%'            THEN s.dur ELSE 0 END)/1e6, 2) AS egui_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%apply_resource_nodes%' THEN s.dur ELSE 0 END)/1e6, 2) AS resnode_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%maintain_world_grid%'  THEN s.dur ELSE 0 END)/1e6, 2) AS grid_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%render_system%'        THEN s.dur ELSE 0 END)/1e6, 2) AS render_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%prepare_windows%'      THEN s.dur ELSE 0 END)/1e6, 2) AS prep_win_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%EarlyShadowPassNode%'  THEN s.dur ELSE 0 END)/1e6, 2) AS shadow_ms
FROM main_frames f
JOIN slice s ON s.ts >= f.frame_start AND s.ts + s.dur <= f.frame_end
GROUP BY f.id
ORDER BY f.dur DESC
LIMIT 20;
```

**Worst individual spans.** For when the slow-frame composition is uniform and you need the single outlier (a Lightyear receive burst, a Metal swapchain stall). Excludes wrappers and startup noise.

```sql
SELECT SUBSTR(name, 1, 75) AS name, track_id,
       ROUND(dur/1e6, 2) AS dur_ms, ROUND(ts/1e6, 1) AS ts_ms
FROM slice
WHERE ts > 3000000 AND dur > 4000000
  AND name NOT LIKE 'main app:%' AND name NOT LIKE 'schedule:%'
  AND name NOT LIKE 'sub app:%'  AND name NOT LIKE 'update:%'
  AND name NOT LIKE 'multithreaded executor%'
  AND name NOT LIKE 'plugin%'    AND name NOT LIKE 'asset loading:%'
  AND name NOT LIKE '%PipelineCache%'
  AND name NOT LIKE '%upscaling::prepare_view_upscaling_pipelines%'
  AND name NOT LIKE 'run_graph%' AND name NOT LIKE 'pass:%'
  AND name NOT LIKE 'submit_graph_commands%'
ORDER BY dur DESC
LIMIT 30;
```

**Cadence of slow frames.** Tells you whether slow frames are isolated (random hitches) or clustered into bursts (GPU stall / sustained contention). The gap is start-of-slow-frame to start-of-next-slow-frame.

```sql
WITH s AS (
  SELECT ROW_NUMBER() OVER (ORDER BY ts) AS i, ts/1e6 AS ms
  FROM slice
  WHERE name = 'main app: ' AND track_id = 0
    AND ts > 3000000 AND dur > 5000000   -- tune to your "slow"
)
SELECT CAST(b.ms - a.ms AS INTEGER) AS gap_ms, COUNT(*) AS n
FROM s a JOIN s b ON b.i = a.i + 1
WHERE b.ms - a.ms < 100
GROUP BY 1 ORDER BY 1;
```

A peak at small `gap_ms` (about the slow-frame duration) means **consecutive** slow frames, a sustained burst, usually a GPU pipeline stall blocking the main thread's extract phase. A peak at 50 ms means tied to the 20 Hz server tick. A flat spread means random.

## Canonical fixes (the bug shapes a trace points at)

Each of these was found via the workflow above and is already fixed once in the codebase. When a trace reproduces the symptom, the fix is usually the same shape.

**A system iterating N replicated entities every frame to discover "nothing changed".** The most common bug shape here. `apply_resource_nodes_system` (`src/app/systems/items/resource_nodes/mod.rs`) and `maintain_world_grid_system` (`src/app/systems/deployables/mod.rs`) both had it. The fix is event-driven: react to `Added<T>` and `RemovedComponents<T>` plus your own bookkeeping, with a cheap probe at the top that early-returns when no events fired. Iterating the full AoI query every frame costs 1-4 ms for the noop case alone at AoI scale (the in-code comment on `maintain_world_grid_system` calls the old fingerprint-every-frame approach "a lie at scale", 1-2 ms/frame with ~1811 nodes). This is **the** reconciliation pattern; copy it from `apply_resource_nodes_system` exactly: pending-spawn `VecDeque` to carry the per-frame budget across frames, reverse `Entity -> Id` map so `RemovedComponents` can find the local mirror, and a one-time `applied_first_snapshot` catch-up scan because the `Added` filter's `last_run` tick misses entities that arrived during early-return `client_id == None` frames. Full write-up in [docs/replication.md](replication.md).

**Per-frame spawn budgets, not time throttles.** Both reconciliation systems cap how many entities they instantiate per frame and carry the remainder forward, rather than doing all the work in one frame or gating on a timer:
- Resource nodes: `MAX_RESOURCE_NODE_SPAWNS_PER_FRAME = 8`, queued in `pending_spawns: VecDeque`. The initial ~1811-node AoI fill takes ~226 frames at 8/frame (see the in-code comment).
- Grass: `MAX_GRASS_TILE_SPAWNS_PER_FRAME = 12` (`src/app/scene/grass/mod.rs`), with `fill_pending` carry-over and an early-out when the camera tile is unchanged and nothing is pending. This is the "grass throttle"; it is a spawn budget, not a time throttle. The mental model matters when tuning: raise the cap to fill faster at the cost of fatter frames during the fill.

**Spurious change-detection on resources.** Bevy fires change detection on any `&mut` deref, even when the new value equals the old. Compare-before-assign. Hit in `src/app/systems/input/cursor.rs`: every-frame writes to `CursorOptions.visible` / `grab_mode` tripped `bevy_winit`'s `changed_cursor_options` slow winit path, ~684 us mean / 16 ms outlier on the main thread. The fix only assigns when the value actually flips.

**Framepace `Auto` re-queries winit every frame.** `src/app/systems/display.rs` (`target_limiter`) resolves vsync to a fixed `Limiter::Manual(frametime)` computed from the primary monitor's refresh rate, falling back to `Auto` only before the refresh rate is known. The two cap identically, but `bevy_framepace`'s `Auto` runs `current_monitor().refresh_rate_millihertz()` on the main thread every frame (~37 us/frame). Unit tests cover the resolution. Never leave `Auto` set in steady state.

**`commands.entity().insert()` on a maybe-despawned entity panics in apply_buffers.** On the server, between queuing a teardown command and the buffer applying, Lightyear's connection management can despawn the client entity. Use `try_insert`; it silently no-ops on a despawned target, which is correct for teardown signals. See `src/net/host/routing.rs` (the `Disconnecting` insert).

## Don't gate work behind Changed<T> for replicated components

Lightyear's receive path uses `insert_by_ids(...)`, which **always** bumps the change tick, even when the new bytes equal the old. So `Changed<T>` and `Ref<T>::is_changed()` fire on every replication tick for any replicated component (`ResourceNode`, `Deployable`, `DeployableActive`, dropped items, players), not just on real changes. Do not gate work behind them for replicated state. Use `Added<T>` (one-shot per spawn) and `RemovedComponents<T>` (one-shot per despawn) plus event-driven bookkeeping. This is a CLAUDE.md replicated-state rule and is documented in full, with the canonical pattern, in [docs/replication.md](replication.md).

## Diagnostic patterns

- **Bimodal frame distribution** means a periodic system adds work every Nth frame. The fast peak is the baseline; the slow peak is baseline plus that work. The "top contributors" query reveals which.
- **Clustered slow frames (small `gap_ms`)** mean a GPU pipeline stall. Bevy's pipelined renderer makes the main thread wait one render frame at the extract sync; a slow render frame blocks the next main frame, which then has nothing to do but wait and shows up as a slow main frame too. Common on macOS: `bevy_render::view::window::prepare_windows`, the `wgpu::Surface::get_current_texture()` Metal swapchain stall ([upstream](https://github.com/gfx-rs/wgpu/issues/2269)).
- **Flat slow-frame spread** means random hitches; look for an outlier with the "worst individual spans" query rather than a periodic system.

## Build levers

- `./cli profile`: dev build with `bevy/trace_chrome` + the diagnostics plugins. Use for trace capture only.
- `./cli dev`: plain dev build. `[profile.dev.package."*"] opt-level = 3` (`Cargo.toml`) builds all dependencies (Bevy, rapier3d, Lightyear) at opt-level 3 even in dev, so a plain dev build is already representative for HUD-level measurement; only our own crate stays at the default opt level for fast incremental rebuilds.
- `./cli dev-fast`: adds `bevy/dynamic_linking` (`dev-fast` feature) for faster link times during iteration; not for measurement.
- `PERF_STATS_BROADCAST_INTERVAL_TICKS` (`src/server.rs`) is the server-to-client perf-stats cadence; it equals `SERVER_TICK_RATE_HZ` (20), so the HUD's server block refreshes once per second. Raising it lowers that bandwidth at the cost of a staler HUD. (Gameplay tuning constants live in `src/game_balance.rs` per CLAUDE.md, but this broadcast cadence is a server-internal knob and lives with the other broadcast intervals in `src/server.rs`.)

## The remaining floor (unimplemented)

The one large optimization not yet built is reducing the per-frame render-pipeline cost of the ~1800-visible-entity AoI floor (real PBR meshes with shadows, all visible at once). It is captured as a design proposal in [projects/resource-node-instancing.md](../projects/resource-node-instancing.md), **status: proposal, not implemented**. Option A (chunk-level mesh merge, keeps Bevy PBR/shadows/batching) is the recommended path; nothing is built. The cost figures in that doc date to the 2026-05-29 capture session and predate later art passes (trees, grass, toon) that changed the visible-entity mix, so treat them as historical, not current. Do not assume any merged-mesh / chunk-visual system exists in `src/`.

## Related docs

- [docs/replication.md](replication.md) - the `Changed<T>`-lies rule and the full event-driven reconciliation pattern this doc points at.
- [docs/build-and-dev.md](build-and-dev.md) - the `./cli` surface (`profile`, `dev`, `dev-fast`).
- [docs/chunks-and-aoi.md](chunks-and-aoi.md) - the AoI ring and visible-node floor that drives per-frame client cost.
- [projects/resource-node-instancing.md](../projects/resource-node-instancing.md) - unimplemented proposal to cut the render-pipeline floor.
