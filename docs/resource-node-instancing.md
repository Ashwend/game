---
title: "Proposal: resource-node render-floor reduction"
owns: The unimplemented design intent for cutting the per-frame render cost of the AoI-saturated resource-node entity floor.
when_to_read: Only when asked to reduce the per-frame render cost of the ~1800-visible-entity floor.
status: proposal
sources:
  - src/app/systems/items/resource_nodes.rs - apply_resource_nodes_system, MAX_RESOURCE_NODE_SPAWNS_PER_FRAME, pending_spawns, applied_first_snapshot
  - src/app/systems/items/resource_nodes/spawn.rs - spawn_resource_node_entity, spawn_pop_in_chip_burst, is_crude shadow gate
  - src/app/systems/items/resource_nodes/pop_in.rs - tick_resource_node_pop_in_system, POP_IN_DURATION_SECS
  - src/resource_nodes.rs - ResourceNodeModel, is_crude, is_tree, is_ore
  - src/server/resource_node_ecs.rs - ResourceNode, ResourceNodeStorage, ResourceNodeChunk
related:
  - docs/profiling.md - the trace investigation that defined this headroom and the spawn-budget / event-driven patterns this proposal reuses
  - docs/chunks-and-aoi.md - chunk-room AoI gating that Option A's visual entities would ride on
  - docs/replication.md - why the logical entities stay replicated and the visual entities must not be
  - docs/rendering-materials.md - the PBR material path Option A preserves
---

# Proposal: resource-node render-floor reduction

> When to read this: only when asked to reduce the per-frame render cost of the ~1800-visible-entity floor. Source of truth for shipped behavior: `src/app/systems/items/resource_nodes/`. Canonical invariants live in CLAUDE.md. This doc is a design proposal, nothing here is built.

## STATUS

**Proposal only. Nothing in this doc is implemented.**

- None of Options A, B, or C exist in the tree. `grep` for `ChunkVisualState`, `rebuild_chunk_visual_meshes_system`, or any merged-mesh code returns nothing. The client still spawns one `Mesh3d` mirror entity per resource node.
- All performance figures below are **historical**, captured in the 2026-05-29 trace session that defined this headroom. They predate the 2026-06 art passes (trees toon remodel, grass upgrade, ore + deployables cel shading), which changed the visible-entity mix. Treat the numbers as a pre-art-pass baseline, not current measurements. Re-capture a trace (see [docs/profiling.md](../docs/profiling.md)) before acting on any threshold here.
- Recommendation stands: investigate (C) first, then commit to chunk-level mesh merging (A) if needed. Avoid the custom-instancing path (B).

## Problem

At AoI-fill saturation (~1811 visible `Mesh3d` mirror entities: trees, ore veins, crude clutter) the client hit a per-frame floor of ~3 ms median / ~7 ms p99 in the 2026-05-29 trace. Trace analysis attributed most of the remaining variance to Bevy render-thread systems that **scale linearly with mesh-entity count**:

| span | typical cost at 1811 entities | scales with |
|---|---|---|
| `bevy_render::view::window::prepare_windows` | 5-11 ms (rare outlier) | macOS Metal swapchain, *not* entity count |
| `bevy_render::renderer::render_system` | 2-4 ms | draw-call count (largely batched already) |
| `bevy_pbr::prepass::update_mesh_previous_global_transforms` | 7 ms peak | entity count (linear) |
| `bevy_transform::systems::sync_simple_transforms` | 4-5 ms peak | entity count (linear, *if* `Changed`) |
| `bevy_pbr::cluster::prepare_clusters` | 0.5-2 ms | per-light x per-entity |
| `bevy_render::batching::gpu_preprocessing::*` | 0.5-2.5 ms | entity count |

The per-frame *gameplay* cost was already removed in earlier sessions: `apply_resource_nodes_system` (`src/app/systems/items/resource_nodes.rs - apply_resource_nodes_system`) is event-driven, and `maintain_world_grid_system` is event-gated (see [docs/profiling.md](../docs/profiling.md)). What remained was **Bevy's render-pipeline cost of having 1811 distinct mesh entities**.

To push the median frame below ~3 ms (above ~333 fps headroom) the entity count itself has to drop.

Bevy version at proposal time: `0.18.1` (`Cargo.toml - bevy`). Some Bevy 0.18 render systems are unconditionally O(N) per-entity regardless of `Changed` state, which is why C alone may not suffice.

## Current shipped shape (what a refactor starts from)

These paths are live and correct as of this writing; verify against the tree before relying on a line number.

- **Logical / authoritative state**: `ResourceNode` + `ResourceNodeStorage` mirror entity, replicated by Lightyear (`src/server/resource_node_ecs.rs - ResourceNode`, `ResourceNodeStorage`). `ResourceNodeChunk(ChunkCoord)` also rides the same server entity, but it is a server-only component (it mirrors `ChunkManager::node_chunks` and is NOT replicated to clients; only `ResourceNode` and `ResourceNodeStorage` are registered with Lightyear). Gather, pickup-target raycast, and all gameplay read these.
- **Visual state today**: the client spawns a separate client-only visual entity carrying a `Mesh3d` (the replicated `ResourceNode` entity does not carry `Mesh3d`); the two are linked by the `ResourceNodeEntities` id maps. One mesh entity per node. This is the entity count the proposal targets.
- **Client reconciliation**: `apply_resource_nodes_system` (`src/app/systems/items/resource_nodes.rs`) is event-driven, reacting to `Added<ResourceNode>` and `RemovedComponents<ResourceNode>`, with a `pending_spawns: VecDeque` and a per-frame budget `MAX_RESOURCE_NODE_SPAWNS_PER_FRAME = 8`. The one-time `applied_first_snapshot` catch-up scan handles entities that arrived during early-return `client_id == None` frames. The initial 1811-node fill takes ~226 frames at budget 8/frame (comment in `resource_nodes.rs` near the budget loop). **Any new visual-mesh rebuild system must copy this pattern exactly**, see [docs/replication.md](../docs/replication.md) and CLAUDE.md replicated-state rule #6 (do not gate on `Changed<T>` / `Ref::is_changed()` for Lightyear-touched components).
- **Pop-in animation**: `tick_resource_node_pop_in_system` (`src/app/systems/items/resource_nodes/pop_in.rs`) eases each node up out of the ground over `POP_IN_DURATION_SECS = 0.42` (with a small overshoot). Per-node transform animation.
- **Pop-in chip burst**: `spawn_pop_in_chip_burst` (`src/app/systems/items/resource_nodes/spawn.rs`) spawns standalone particle entities when a node appears. Independent of the node mesh.
- **Shadow gate**: crude clutter skips the shadow pass. `spawn_resource_node_entity` inserts `NotShadowCaster` when `model.is_crude()` is true (`src/app/systems/items/resource_nodes/spawn.rs`; `is_crude` is defined in `src/resource_nodes.rs - ResourceNodeModel::is_crude` and covers `SurfaceStone`, `BranchPile`, `HayGrass`). Tree canopies additionally opt out of shadow *reception* (`NotShadowReceiver`) to avoid self-shadow acne.
- **Model taxonomy**: `ResourceNodeModel` (`src/resource_nodes.rs - ResourceNodeModel`) has **13 variants**: `CoalOre`, `IronOre`, `SulfurOre`, `StoneVein`, `PineTree{Small,Medium,Large}`, `BirchTree{Small,Medium,Large}`, `SurfaceStone`, `BranchPile`, `HayGrass`. Of these, `is_ore()` is the three ores, `is_tree()` is the six trees, `is_crude()` is the three E-pickup tufts. This 13 is the upper bound on "distinct visual model types" any merge or instancing scheme must handle.

## Options

Three approaches, framework risk increasing top-to-bottom.

### Option A, chunk-level mesh merging (recommended)

Combine all same-model resource nodes within a chunk into **one merged `Mesh3d` entity per (chunk, model) pair**. Worst case ~13 model types x ~50 visible chunks, but each chunk only holds a few model types, so the realistic visual-entity count is ~50-200 replacing the current ~1811.

**Architecture sketch**

- *Logical* entity per resource node (server-replicated `ResourceNode` + `ResourceNodeStorage` + `ResourceNodeChunk`, **no `Mesh3d`**). Stays as-is. Gather, pickup-target raycast, all gameplay read from these. Cheap.
- *Visual* entity per `(ChunkCoord, ResourceNodeModel)`: a `Mesh3d` of merged geometry with a single `MeshMaterial3d`. Spawned and rebuilt lazily.
- A `ChunkVisualState` resource (or a component on the chunk-room entity) tracks which (chunk, model) pairs have an active merged mesh and its dirty/clean state.
- A `rebuild_chunk_visual_meshes_system` consumes `Added<ResourceNode>` / `RemovedComponents<ResourceNode>` (the same event-driven pattern in `src/app/systems/items/resource_nodes.rs`) and rebuilds *only the affected (chunk, model) merged meshes*. The client cannot read the chunk key from `ResourceNodeChunk` (that component is server-only and never replicated), so it would have to derive the chunk from the node's replicated position, or a new replicated chunk component would have to be added.

**Pros**

- Standard Bevy PBR materials, shadows, clustering all keep working unchanged. No custom shaders, no render-graph changes. See [docs/rendering-materials.md](../docs/rendering-materials.md).
- Per-entity Bevy systems (`sync_simple_transforms`, `prepass::update_mesh_previous_global_transforms`, etc.) collapse from O(1811) to O(visible chunks x model types).
- Hit detection and gameplay are unaffected; they query the logical entities.
- Pop-in chip bursts (`spawn_pop_in_chip_burst` in `spawn.rs`) keep working; they are standalone particle entities.

**Cons**

- Per-node pop-in animation (`tick_resource_node_pop_in_system` in `pop_in.rs`) cannot animate one node inside a merged mesh; the merged mesh's transform animates as a whole. **Mitigation:** spawn a temporary per-node `Mesh3d` for the pop-in duration (`POP_IN_DURATION_SECS = 0.42`), then despawn it once the merged mesh contains its geometry. The chip burst already provides the "fresh node" cue.
- Mesh rebuild has a CPU cost. With the 8-spawns/frame budget already in place and node respawns 5-15 min apart, rebuilds are rare in steady state. Worst case: initial AoI fill triggers ~50-200 rebuilds, spread over the same ~226 frames the existing spawn budget already spreads them over.
- Mesh memory roughly doubles for the merged copy. The per-instance handles can be dropped from the asset cache once a node's geometry is baked into a merged mesh.

**Framework risk:** Low. Purely additive on the client. Server, replication, Lightyear, gameplay untouched. Bevy's mesh batching keeps doing its job.

**Effort:** 2-3 days.

### Option B, manual GPU instancing with a custom material

One `Mesh3d` per `ResourceNodeModel` (up to 13 entities total). Per-instance transform/yaw data lives in a `StorageBuffer<Vec<MeshInstance>>`. A custom WGSL shader reads the instance index from `vertex_index / mesh_vert_count` and looks up its transform.

**Pros**

- Maximum entity reduction (up to ~13 visual entities total).
- Per-instance data updates only when nodes are added/removed.

**Cons**

- **Custom shader.** Bevy's PBR pipeline is tightly coupled to `bevy_pbr::Mesh::transform`. Bypassing it means either reimplementing PBR (lights, shadows, fog, tonemapping, decals) or extending `ExtendedMaterial` and hoping the extension hooks cover what is needed. They likely do not cover shadow casting.
- Loses or breaks: shadow cascades for instanced nodes, clustered light culling per-instance, the prepass (motion vectors).
- Storage-buffer rebinding on every visibility/AoI change is non-trivial.
- High risk of "subtly wrong shadow / light response on resource nodes" bugs that tests will not catch.

**Framework risk:** High. Steps outside Bevy's standard PBR path; future Bevy upgrades may force shader rewrites.

**Effort:** 4-7 days, plus a tail of "fix subtle visual regressions".

### Option C, investigate first; maybe no refactor is needed

The trace showed `sync_simple_transforms` and `update_mesh_previous_global_transforms` taking multi-millisecond bursts. These systems are *supposed* to skip entities with unchanged transforms (`Changed<Transform>` filter). If the 1811 nodes are truly static post-spawn, these should be near-zero.

Something may still be tripping `Changed<Transform>` on resource nodes every frame, e.g. Lightyear writing the component on every replication update, or a stray `commands.entity().insert(target_transform)`. One such site was fixed in the 2026-05-29 session (the per-frame `Transform` re-insert that started the investigation). Note: per CLAUDE.md replicated-state rule #6 and [docs/profiling.md](../docs/profiling.md), Lightyear's receive path bumps the change tick on every replication tick even when bytes are identical, so `Changed`/`Ref::is_changed()` is an unreliable signal here; confirm via a logged `Ref<Transform>` probe, not a `Changed` filter.

**Pros**

- If it is a bug, the fix is small.
- Cost: hours, not days.

**Cons**

- May not yield enough savings. Even if `Changed<Transform>` bookkeeping is correctly idle, Bevy's `extract_meshes_for_gpu` and clustering still iterate per-entity. Some Bevy 0.18 systems are unconditionally O(N) per-entity regardless of `Changed`.

**Framework risk:** None.

**Effort:** half a day to investigate.

## Recommendation

**Start with C, then commit to A if needed.**

1. **C first** (half a day). Add a temporary `Ref<Transform>` query that logs whenever a `ResourceNode`-tagged entity's transform changes post-spawn. If Lightyear or another system trips it every frame, the spans are pure waste and the fix is bounded.
2. **A if C does not suffice** (2-3 days). The chunk-level merge gives a large constant-factor win without touching the rendering framework. It preserves Bevy PBR, shadows, batching, and every existing gameplay path. The pop-in degradation has a clean mitigation.
3. **Avoid B.** The framework risk and ongoing maintenance burden do not justify the extra entity-count reduction over A. Custom rendering in a prototype that already ran at 333 fps median is premature.

## Compatibility with the framework

| invariant | preserved by A | preserved by B |
|---|---|---|
| Server stays authoritative; gameplay flows through `GameServer` + `ClientMessage`/`ServerMessage`. | yes | yes |
| Per-entity replication via Lightyear (no full-state snapshot revival). | yes | yes |
| Singleplayer and multiplayer use the same client code path. | yes | yes |
| Gameplay never pauses (overlays gate controls, not simulation). | yes | yes |
| Bevy PBR materials, shadows, clusters, prepass. | yes | no (custom material required) |
| Per-node logical entity for gather/damage. | yes (logical/visual split) | yes |
| Per-node pop-in animation. | partial (temporary per-node mesh during pop-in) | partial (custom path needed) |

A keeps every invariant in [CLAUDE.md](../CLAUDE.md) (singleplayer == multiplayer, gameplay-never-pauses, the replicated-state rules). B would require a new render-graph integration the project has not done before.

## Open questions

- How does chunk visibility (entering/leaving the AoI ring on a chunk boundary) interact with the per-chunk visual mesh? The logical entities already handle this via the room-subscription system ([docs/chunks-and-aoi.md](../docs/chunks-and-aoi.md)). The visual mesh should follow the same chunk-room membership: attach the merged-visual entity to the same chunk's local membership so AoI gates it for free.
- Should merged-visual entities replicate? No. They are purely client-side, derived from the replicated logical set. Do **not** attach `Replicate` to them (see [docs/replication.md](../docs/replication.md)).
- Verify the shadow gate survives the merge. Crude clutter skips shadow casting via `NotShadowCaster` (`is_crude()` check in `spawn.rs`), and tree canopies skip shadow *reception*. Merging likely needs to split shadow-casting and non-casting nodes into separate visual entities even within the same (chunk, model) bucket.

## Definition of done (targets are pre-art-pass, re-baseline first)

- [ ] Median frame time at the saturated AoI fill: <= 2.5 ms (was ~3 ms in 2026-05-29).
- [ ] p99 frame time: <= 5 ms (was ~7 ms in 2026-05-29).
- [ ] `sync_simple_transforms` and `update_mesh_previous_global_transforms` no longer appear in the top-25 worst spans.
- [ ] Existing gather / pickup-target / pop-in / death-effect behavior preserved (manually verified, and the resource-node unit tests in `src/app/systems/items/resource_nodes/tests.rs` plus `stages.rs` still green).
- [ ] No regression in the deployable tests (`src/app/systems/deployables/tests.rs`); the merge pattern may inform deployable rendering later.

## Related docs

- [docs/profiling.md](../docs/profiling.md) - the trace workflow that produced these figures and the spawn-budget / event-driven reconciliation patterns Option A reuses.
- [docs/chunks-and-aoi.md](../docs/chunks-and-aoi.md) - chunk membership and room-based AoI gating the per-chunk visual entities would ride on.
- [docs/replication.md](../docs/replication.md) - why the logical entities stay replicated and the visual entities must stay client-only.
- [docs/rendering-materials.md](../docs/rendering-materials.md) - the standard PBR material path Option A keeps intact.
- [docs/items-and-resources.md](../docs/items-and-resources.md) - the `ResourceNodeModel` taxonomy and node spawn rules.
