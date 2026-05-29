# Profiling and trace analysis

How to capture a Chrome-tracing JSON from the client, load it into Perfetto's `trace_processor`, and query it with SQL to find expensive frames.

Use this when:
- F2 HUD shows the `p99 frame` or `max frame` diverging meaningfully from `frame`.
- A periodic stutter is reproducible.
- You want to know *which system* is causing a frame spike, not just that one exists.

## 1. In-game perf HUD (no rebuild)

Press **F2** to toggle the perf overlay ([src/app/ui/hud.rs](../src/app/ui/hud.rs)). It exposes:

| field | what it means |
|---|---|
| `FPS` | Smoothed FPS over the last ~1s window (rolling average). Hides spikes. |
| `Frame` | The most recent single frame in ms. Bounces. |
| `p99 frame` | 99th-percentile of the last 480-sample window. If this diverges from `Frame`, you have periodic stalls even if the average looks fine. |
| `max frame` | Worst single frame in the window. |
| `Chunk`, `Loaded`, `Live nodes`, `Visible`, `Regrow queue` | Server-side counters from the periodic [`PerfStatsSnapshot`](../src/protocol.rs) broadcast. Visible-node count is what drives most per-frame client work. |

The HUD is enough for a first-pass diagnosis. If `p99` ≈ `Frame`, the game is steady — no spikes. If `p99` >> `Frame`, capture a trace.

## 2. Capturing a Chrome trace

Build with the `profile` Cargo feature. This adds:
- `bevy/trace_chrome`: writes `trace-<unix-ms>.json` to the working directory on **clean exit**.
- `LogDiagnosticsPlugin` + `EntityCountDiagnosticsPlugin` + `SystemInformationDiagnosticsPlugin`: periodic stdout logs of fps, frame time, entity count, CPU, RAM.

```bash
# Terminal 1 — local dedicated server (reproduces the multiplayer code path
# without WAN jitter; recommended over a remote test).
./cli server --bind 127.0.0.1:7777

# Terminal 2 — client with tracing
cargo run --features profile --bin game -- client --connect 127.0.0.1:7777
```

Stand still in-game for ~30 seconds, then **quit through the menu** (not Ctrl+C). The trace only flushes on clean exit. Output lands in the project root.

**Caveats**
- The trace is large (~100 MB per second of capture, ~2-6 GB for 30 s) and writing it has non-trivial overhead. If a problem only reproduces with `--features profile`, it may *be* the profile overhead. Always retest without it.
- The `./cli profile` wrapper invokes the same command but also sets `BEVY_ASSET_ROOT` and kills any existing `Game` process first. Use it if you're seeing asset-load errors when launching cargo directly.

## 3. Loading the trace into Perfetto

We use the `trace_processor` CLI — Perfetto's local SQL engine for trace files. It parses the Chrome JSON, exposes spans as a SQLite-shaped `slice` table, and runs SQL queries non-interactively.

```bash
# One-time install
curl -LO https://get.perfetto.dev/trace_processor
chmod +x ./trace_processor
mv ./trace_processor /tmp/trace_processor
```

Run a query against a trace file:

```bash
/tmp/trace_processor query -f /tmp/some_query.sql /path/to/trace.json
```

Skip the loading-progress noise:

```bash
/tmp/trace_processor query -f /tmp/some_query.sql /path/to/trace.json 2>&1 \
  | grep -v "^Loading\|^\[\|Trace health\|Data losses\|misplaced\|^column"
```

The trace loads in 10-20 s for a multi-GB file; then queries run in milliseconds.

### The `slice` table schema we use

| column | meaning |
|---|---|
| `id` | row id (unused for our queries) |
| `ts` | start timestamp, **nanoseconds** since trace start |
| `dur` | duration in **nanoseconds** |
| `name` | span label — typically `"system: name=\"…\""` for Bevy systems, `"main app: "` for the per-frame wrapper, `"schedule: name=Update "` etc. for schedule wrappers |
| `track_id` | thread / track identifier. `0` = main thread, `3` = render thread, `4-8` = task pool workers |

**Watch out for trailing spaces in span names.** Bevy's `tracing` formatting includes a trailing space: filter for `'main app: '`, `'schedule: name=Main '`, etc., not `'main app:'`.

`ts/1e6` gives milliseconds, `ts/1e9` gives seconds. `dur/1e6` for milliseconds.

## 4. Useful queries

These four queries pinpoint nearly every kind of frame-pacing problem we hit during the 2026-05-29 session. Save them under `/tmp/*.sql` and re-run as needed.

### 4a. Frame-time distribution

Bins every per-frame `main app:` duration. Look for a **bimodal** distribution — a clean fast peak and a separate slow peak — that's the signature of a periodic system adding work to ~1-in-N frames.

```sql
WITH frames AS (
  SELECT dur/1e6 AS frame_ms
  FROM slice
  WHERE name = 'main app: '
    AND track_id = 0
    AND ts > 3000000   -- skip 3 ms of startup
)
SELECT
  CAST(frame_ms AS INTEGER) || ' ms' AS bucket,
  COUNT(*) AS n_frames
FROM frames
GROUP BY CAST(frame_ms AS INTEGER)
ORDER BY CAST(frame_ms AS INTEGER);
```

### 4b. Top contributors on slow frames

Sum the cost of every Bevy/Lightyear/our-game system that ran *during* each slow frame. Bucket the slow window tightly (e.g. `BETWEEN 6000000 AND 9000000` for 6-9 ms) and the column with the biggest non-zero values is the culprit.

```sql
WITH main_frames AS (
  SELECT id, ts AS frame_start, ts + dur AS frame_end, dur
  FROM slice
  WHERE name = 'main app: ' AND track_id = 0
    AND ts > 3000000 AND dur BETWEEN 6000000 AND 9000000
)
SELECT
  ROUND(f.dur/1e6, 2) AS frame_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%lightyear%' THEN s.dur ELSE 0 END)/1e6, 2) AS lightyear_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%Replication%' THEN s.dur ELSE 0 END)/1e6, 2) AS replication_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%bevy_egui%' THEN s.dur ELSE 0 END)/1e6, 2) AS egui_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%apply_resource_nodes%' THEN s.dur ELSE 0 END)/1e6, 2) AS resnode_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%maintain_world_grid%' THEN s.dur ELSE 0 END)/1e6, 2) AS grid_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%render_system%' THEN s.dur ELSE 0 END)/1e6, 2) AS render_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%prepare_windows%' THEN s.dur ELSE 0 END)/1e6, 2) AS prep_win_ms,
  ROUND(SUM(CASE WHEN s.name LIKE '%EarlyShadowPassNode%' THEN s.dur ELSE 0 END)/1e6, 2) AS shadow_ms
FROM main_frames f
JOIN slice s ON s.ts >= f.frame_start AND s.ts + s.dur <= f.frame_end
GROUP BY f.id
ORDER BY f.dur DESC
LIMIT 20;
```

Add or remove columns to chase specific suspects. The `LIKE '%term%'` matcher is forgiving — `'%apply_resource_nodes%'` catches both the system span and its `system_commands` deferred-apply counterpart.

### 4c. Worst individual spans

Useful when the slow-frame composition is uniform and you need to find an outlier — e.g. a single Lightyear receive burst or a Metal swapchain stall.

```sql
SELECT
  SUBSTR(name, 1, 75) AS name,
  track_id,
  ROUND(dur/1e6, 2) AS dur_ms,
  ROUND(ts/1e6, 1)   AS ts_ms
FROM slice
WHERE ts > 3000000 AND dur > 4000000
  -- exclude wrappers and startup noise:
  AND name NOT LIKE 'main app:%' AND name NOT LIKE 'schedule:%'
  AND name NOT LIKE 'sub app:%'  AND name NOT LIKE 'update:%'
  AND name NOT LIKE 'multithreaded executor%'
  AND name NOT LIKE 'plugin%'    AND name NOT LIKE 'asset loading:%'
  AND name NOT LIKE '%PipelineCache%'
  AND name NOT LIKE '%upscaling::prepare_view_upscaling_pipelines%'
  AND name NOT LIKE 'run_graph%' AND name NOT LIKE 'pass:%'
  AND name NOT LIKE 'submit_graph_commands%'
  AND name NOT LIKE 'system_commands: name="game::net::client::process_pending_connect_system%'
  AND name NOT LIKE 'system_commands: name="game::analytics%'
ORDER BY dur DESC
LIMIT 30;
```

### 4d. Cadence of slow frames

Tells you whether slow frames are **isolated** (random hitches) or **clustered into bursts** (GPU pipeline stall or sustained contention). The gap is start-of-slow-frame to start-of-next-slow-frame.

```sql
WITH s AS (
  SELECT ROW_NUMBER() OVER (ORDER BY ts) AS i, ts/1e6 AS ms
  FROM slice
  WHERE name = 'main app: ' AND track_id = 0
    AND ts > 3000000 AND dur > 5000000   -- adjust threshold to your "slow"
)
SELECT
  CAST(b.ms - a.ms AS INTEGER) AS gap_ms,
  COUNT(*) AS n
FROM s a JOIN s b ON b.i = a.i + 1
WHERE b.ms - a.ms < 100
GROUP BY 1 ORDER BY 1;
```

A peak at small gap_ms (≈ slow_frame_dur) means **consecutive** slow frames — a sustained burst, very likely a GPU pipeline stall blocking the main thread's next extract phase. A peak at 50 ms means tied to the 20 Hz server tick. A flat distribution means random.

## 5. Diagnostic patterns we learned

These are the systems-level patterns we hit while debugging the 2026-05-29 frame-spike investigation. If you're chasing a similar symptom, start here before instrumenting.

**Bimodal frame distribution → a periodic system adds work to every Nth frame.**
The fast peak is the baseline; the slow peak is the baseline plus N systems' worth of work. Query 4b reveals which.

**Slow frames in clustered bursts (small gaps in query 4d) → GPU pipeline stall.**
Bevy's pipelined renderer makes the main thread wait one render frame at the extract sync. A slow render frame blocks the next main frame, which has nothing to do but wait — looks like a slow main frame in the trace too. This propagates until the GPU catches up. Common cause on macOS: `bevy_render::view::window::prepare_windows` (the `wgpu::Surface::get_current_texture()` Metal swapchain stall, [upstream issue](https://github.com/gfx-rs/wgpu/issues/2269)).

**A system iterating N replicated entities every frame to discover "nothing changed" is the most common bug shape we saw.**
Both [`apply_resource_nodes_system`](../src/app/systems/items/resource_nodes.rs) and [`maintain_world_grid_system`](../src/app/systems/deployables.rs) had this pattern. The fix is a cheap event-driven probe at the top of the system:

```rust
let added_any = !added_probe.is_empty();
let removed_count = removed_probe.read().count();
if !added_any && removed_count == 0 && other_queues_empty {
    return;   // fast path — no iteration
}
```

For systems where `Added<T>` can fire while the system early-returns (e.g. the client-connect window where `client_id == None`), the `Added` filter compares against the system's *last_run* tick and misses entities added during early-return frames. The fix is a **one-time catch-up scan** gated by an `applied_first_snapshot: bool` flag — see [src/app/systems/items/resource_nodes.rs:155](../src/app/systems/items/resource_nodes.rs) for the canonical implementation.

**Spurious change-detection on resources.**
Bevy's change detection fires on any `&mut` access via `DerefMut`, even when the new value equals the old. Compare-before-assign is the fix. We hit this in [`update_cursor_system`](../src/app/systems/input/cursor.rs) — every-frame writes to `CursorOptions.visible` tripped `bevy_winit::system::changed_cursor_options`'s slow winit path. Cost was ~684 µs mean / 16 ms outlier on the main thread.

**`commands.entity().insert()` on a maybe-despawned entity → panic in apply_buffers.**
On the server, between queuing a teardown command (e.g. `Disconnecting`) and the buffer being applied, Lightyear's own connection management can despawn the client entity. Use `try_insert` instead — it silently no-ops on a despawned target, which is the right behavior for teardown signals. Example: [src/net/host/routing.rs:113](../src/net/host/routing.rs).

**`Ref::is_changed()` is unreliable for Lightyear-replicated components.**
Lightyear's receive path uses `entity_world_mut.insert_by_ids(...)` which *always* bumps the change tick, even when the new bytes equal the old. So `Changed<T>` and `Ref<T>::is_changed()` fire on every replication tick, not just on real changes. Don't gate work behind these for replicated state; use `Added<T>` (one-shot per spawn) and `RemovedComponents<T>` (one-shot per despawn) instead, plus event-driven bookkeeping.

## 6. Limits of the workflow

- The Chrome trace JSON format **does not record CPU time vs wall time**. Span durations are wall-clock only. You can't tell from the trace whether a thread was busy computing or blocked on a syscall/mutex/GPU. Look at *which threads are running concurrently* for circumstantial evidence.
- `--features profile` itself costs a few percent. For micro-optimization measurement, prefer the F2 HUD over the trace.
- Trace files are huge. Keep at most the last 1-2 around; delete the rest (`rm trace-*.json` in the project root).
