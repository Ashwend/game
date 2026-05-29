# Project: Reduce per-frame cost of resource-node rendering

**Status:** proposal — not implemented
**Estimated effort:** 2-5 days depending on approach
**Driver:** [profiling.md](../docs/profiling.md) — session 2026-05-29 capped the headroom

## Problem

At AoI-fill saturation (~1811 visible `Mesh3d` entities — trees, ore veins, crude clutter) the client hits a per-frame floor of ~3 ms median / ~7 ms p99. Trace analysis attributes most of the remaining variance to a few Bevy render-thread systems that **scale linearly with mesh-entity count**:

| span | typical cost at 1811 entities | scales with |
|---|---|---|
| `bevy_render::view::window::prepare_windows` | 5-11 ms (rare outlier) | macOS Metal swapchain — *not* entity count |
| `bevy_render::renderer::render_system` | 2-4 ms | draw call count (largely batched already) |
| `bevy_pbr::prepass::update_mesh_previous_global_transforms` | 7 ms peak | entity count (linear) |
| `bevy_transform::systems::sync_simple_transforms` | 4-5 ms peak | entity count (linear, *if* Changed) |
| `bevy_pbr::cluster::prepare_clusters` | 0.5-2 ms | per-light × per-entity |
| `bevy_render::batching::gpu_preprocessing::*` | 0.5-2.5 ms | entity count |

Earlier sessions removed all the per-frame *gameplay* cost (`apply_resource_nodes_system`, `maintain_world_grid_system`, `update_pickup_target_system` — all event-driven or fingerprint-gated now). What's left is **Bevy's render-pipeline cost of having 1811 distinct mesh entities**.

To get the median frame below ~3 ms (above ~333 fps headroom) we need to reduce that entity count.

## Options

Three approaches, with framework risk increasing top-to-bottom.

### Option A — Chunk-level mesh merging (recommended)

Combine all same-model resource nodes within a chunk into **one merged `Mesh3d` entity per (chunk, model) pair**. Worst case ~16 model types × ~50 visible chunks = ~800 entities; typical case much fewer because each chunk only has a few model types. Realistically ~50-200 visual entities replacing the current 1811.

**Architecture sketch:**

- *Logical* entity per resource node (server-replicated `ResourceNode` + `ResourceNodeStorage`, no `Mesh3d`). Stays as-is. Gather, pickup-target raycast, gameplay all read from these. Cheap.
- *Visual* entity per `(ChunkCoord, ResourceNodeModel)` (`Mesh3d` of merged geometry, single `MeshMaterial3d`). Spawned and rebuilt lazily.
- `ChunkVisualState` resource (or component on the chunk-room entity) tracks which (chunk, model) pairs have an active merged mesh and its dirty/clean state.
- A `rebuild_chunk_visual_meshes_system` consumes `Added<ResourceNode>` / `RemovedComponents<ResourceNode>` (same pattern we just used in [src/app/systems/items/resource_nodes.rs](../src/app/systems/items/resource_nodes.rs)) and rebuilds *only the affected (chunk, model) merged meshes*.

**Pros**
- Standard Bevy PBR materials, shadows, clustering all keep working unchanged. No custom shaders. No render-graph changes.
- Per-entity Bevy systems (`sync_simple_transforms`, `prepass::update_mesh_previous_global_transforms`, etc.) collapse from O(1811) → O(visible chunks × model types).
- Hit detection and gameplay logic are unaffected — they query logical entities.
- Pop-in chip bursts ([spawn_pop_in_chip_burst at src/app/systems/items/resource_nodes.rs:317](../src/app/systems/items/resource_nodes.rs)) keep working — they're standalone particle entities.

**Cons**
- Per-node pop-in animation (the "node emerges from the ground" curve in [`tick_resource_node_pop_in_system`](../src/app/systems/items/resource_nodes.rs)) won't work — the merged mesh's transform animates as a whole. **Mitigation:** spawn a temporary per-node `Mesh3d` for the duration of the pop-in (~0.42 s), then despawn it once the merged mesh contains its geometry. The chip burst already handles the "fresh node" visual cue.
- Mesh rebuild has a CPU cost. With 8 spawns/frame budget already in place and node respawns 5-15 min apart, rebuilds are rare in steady state. Worst case: initial AoI fill triggers ~50-200 rebuilds, spread over the same 226 frames the existing spawn budget already spreads them over.
- Mesh memory roughly doubles for the merged copy (the per-instance handles can be dropped from the asset cache once a node's geometry is baked into a merged mesh).

**Framework risk:** Low. Pure additive change on the client. Server, replication, Lightyear, gameplay are untouched. Bevy's mesh batching keeps doing its job.

**Effort:** 2-3 days.

### Option B — Manual GPU instancing with a custom material

One `Mesh3d` per `ResourceNodeModel` (10-16 entities total). Per-instance transform/yaw data lives in a `StorageBuffer<Vec<MeshInstance>>`. Custom WGSL shader reads instance index from `vertex_index / mesh_vert_count` and looks up its transform.

**Pros**
- Maximum entity reduction (~10-16 visual entities total).
- Per-instance data updates only when nodes added/removed.

**Cons**
- **Custom shader.** Bevy's PBR pipeline is tightly coupled to `bevy_pbr::Mesh::transform` — bypassing it means either reimplementing PBR (lights, shadows, fog, tonemapping, fog, decals) or extending `ExtendedMaterial` and hoping the extension hooks cover what we need. They probably don't cover shadow casting.
- Loses or breaks: shadow cascades for instanced nodes, clustered light culling per-instance, the prepass (motion vectors).
- Storage-buffer rebinding on every visibility/AoI change is non-trivial.
- High risk of "subtly wrong shadow / light response on resource nodes" bugs that aren't caught by tests.

**Framework risk:** High. Steps outside Bevy's standard PBR path; future Bevy upgrades may need shader rewrites.

**Effort:** 4-7 days, plus tail of "fix subtle visual regressions".

### Option C — Investigate first; maybe no refactor is needed

The trace shows `sync_simple_transforms` and `update_mesh_previous_global_transforms` taking multi-millisecond bursts. These systems are *supposed* to skip entities with unchanged transforms (`Changed<Transform>` filter). If our 1811 nodes are truly static post-spawn, these should be near-zero.

It's possible something is still tripping `Changed<Transform>` on resource nodes every frame — e.g., Lightyear writing the component on every replication update, or a forgotten `commands.entity().insert(target_transform)` somewhere. We already fixed one such site in this session (the per-frame `Transform` re-insert that started the whole investigation).

**Pros**
- If it's a bug, the fix is small.
- Cost: hours, not days.

**Cons**
- May not yield enough savings. Even if Changed<Transform> bookkeeping is correctly idle, Bevy's `extract_meshes_for_gpu` and clustering still iterate per-entity. Some Bevy 0.18 systems are unconditionally O(N) per-entity regardless of Changed.

**Framework risk:** None.

**Effort:** Half a day to investigate.

## Recommendation

**Start with C, then commit to A if needed.**

1. **C first** (half a day). Add a temporary `Ref<Transform>` query that logs whenever a `ResourceNode`-tagged entity's transform changes post-spawn. If Lightyear or another system is tripping `Changed<Transform>` every frame, the spans we see are pure waste and the fix is bounded.
2. **A if C doesn't suffice** (2-3 days). The chunk-level merge gives a large constant-factor win without touching the rendering framework. It preserves Bevy's PBR, shadows, batching, and all the existing gameplay code paths. The pop-in degradation has a clean mitigation.
3. **Avoid B.** The framework risk and ongoing maintenance burden don't justify the additional entity-count reduction over A. Custom rendering in a game prototype that already runs at 333 fps median is premature.

## Compatibility with our framework

| invariant | preserved by A | preserved by B |
|---|---|---|
| Server stays authoritative; gameplay flows through `GameServer` + `ClientMessage`/`ServerMessage`. | ✓ | ✓ |
| Per-entity replication via Lightyear (no `ServerMessage::Snapshot` revival). | ✓ | ✓ |
| Singleplayer and multiplayer use the same client code path. | ✓ | ✓ |
| Per-frame gameplay never pauses (overlays gate controls, not simulation). | ✓ | ✓ |
| Bevy PBR materials, shadows, clusters, prepass. | ✓ | ✗ (custom material required) |
| Per-node logical entity for gather/damage. | ✓ (logical/visual split) | ✓ |
| Per-node pop-in animation. | △ (temporary per-node mesh during pop-in) | △ (custom path needed) |

A keeps every existing invariant in [CLAUDE.md § Singleplayer/multiplayer invariant](../CLAUDE.md) and [CLAUDE.md § Replicated state — rules](../CLAUDE.md). B would require a significant new render-graph integration we haven't done before.

## Open questions

- How does chunk visibility (entering/leaving the AoI ring while standing on a chunk boundary) interact with the per-chunk visual mesh? The logical entities already handle this via the room-subscription system. The visual mesh should follow the same chunk-room membership — likely attach the merged-visual entity to the same chunk room so AoI gates it automatically.
- Should merged-visual entities replicate? No — they're purely client-side, derived from the replicated logical set. Avoid attaching `Replicate` to them.
- Worth verifying that disabling shadow casting on crude clutter ([`is_crude()` check at src/app/systems/items/resource_nodes.rs:294](../src/app/systems/items/resource_nodes.rs)) still applies after merging — likely needs to merge crude and shadow-casting nodes into separate visual entities even within the same chunk.

## Definition of done

- [ ] Median frame time at 1811-node AoI fill: ≤ 2.5 ms (currently 3 ms).
- [ ] p99 frame time: ≤ 5 ms (currently ~7 ms).
- [ ] `sync_simple_transforms` and `update_mesh_previous_global_transforms` no longer appear in the top-25 worst spans.
- [ ] Existing gather / pickup-target / pop-in / death-effect behavior preserved (manually verified + the 19 resource-node unit tests still green).
- [ ] No regression in the 38 deployable tests (the merge pattern may inform deployable rendering later).
