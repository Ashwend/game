# Items and Resources

Two static registries underpin the game's economy:

- **Items** in [`src/items.rs`](../src/items.rs), every player-holdable thing (raw materials, tools, deployables-in-hand) with stack limits, tool profiles, display metadata, and (for deployables) placement profile.
- **Resources** in [`src/resources.rs`](../src/resources.rs), every world-spawnable resource node (trees, ore veins, surface scatter) with a base capacity, regrow window, and the items it yields.

Both registries are compile-time. Adding entries means editing these files, recompiling, and shipping. There is no dynamic loading; this is intentional, the registries are tiny and tying them to the binary version means a save file's items always resolve against a stable set on load.

## Item shape

An item is identified by a stable string id (`&'static str`) like `"basic_pickaxe"` or `"iron_ore"`. The full definition is an `ItemDefinition` with fields covering:

- `id`, `name`, `tint`: identity + UI presentation.
- `stack_size` / `effective_stack_size()`: how many units fit in one slot.
- `model`: `ItemModel`, the first-person **animation archetype** (`Hatchet`, `Pickaxe`, `Bag`, `Deployable`), drives the swing pose + tool-swap cadence. Same-kind tools share an archetype (an iron hatchet swings exactly like a stone one).
- `held_mesh`: `HeldMesh`, the first-person **mesh** the renderer puts in hand (`StoneHatchet`, `IronHatchet`, `StonePickaxe`, `IronPickaxe`, `Bag`). Decoupled from `model` so a tool's look (stone vs iron head) is independent of how it animates. Adding a new tool material is a new `HeldMesh` variant plus one mesh handle in `src/app/scene/assets.rs`, no pose or gameplay change.
- `tool`: `Option<ToolProfile>`, present only for tools. Carries `kind` (`Axe`, `Pickaxe`, `Hands`), `gather_amount`, `cooldown_ticks`, and `tier`. Tier is how progression scales: an iron tool is the same `kind` at `tier: 2` with a bigger `gather_amount`, so it satisfies every tier-1 node automatically and yields more per swing without any per-item branch.
- `deployable`: `Option<DeployableProfile>`, present only for placeable structures (workbench, furnace). Carries `kind`, collider half-extents, max health, station radius (for crafting gating), and material classification.

The active registry is constructed once via `item_definitions_by_id()` (a `LazyLock<HashMap<&str, &'static ItemDefinition>>`) and queried via:

- `item_definition(id) -> Option<&'static ItemDefinition>`, full lookup.
- `stack_limit(id) -> Option<u16>`, convenience for inventory math.
- `normalize_stack(stack)`, clamps quantity into `[1, stack_limit]`; returns `None` for unknown items, which is also the choke point that rejects malformed wire input.

## How to add a new item

1. **Pick a stable id**. Snake case, lowercase, no version suffix. Once shipped it lives forever in player saves.
2. **Add a `pub const X_ID: &str = "x";`** at the top of [`src/items.rs`](../src/items.rs) so call sites reference it symbolically.
3. **Append an entry to the `REGISTERED_ITEMS` array** with the full `ItemDefinition`. Fields:
   - `id: X_ID`, `name: "Display Name"`, `tint: ItemTint::new(r, g, b)`.
   - `stack_size`, clamp this to the real limit. Tools default to 1, raw materials to higher (50–200).
   - `model: ItemModel::...` (animation archetype) and `held_mesh: HeldMesh::...` (in-hand mesh). Raw materials + deployables use `Bag`.
   - `tool: Some(ToolProfile { ... })` only if it's a tool.
   - `deployable: Some(DeployableProfile { ... })` only if it's placeable.
4. **If it's a tool**, set the right `ToolKind` and tune `gather_amount`/`cooldown_ticks`/`tier` against the existing tiers (stone = tier 1, iron = tier 2). A higher tier satisfies every lower-tier node requirement automatically; the bigger `gather_amount` is what makes the upgrade felt. Tools also drive destructible-entity damage via `tool_effectiveness_pct` (the central tool-vs-material table) and `DEPLOYABLE_DAMAGE_PER_GATHER_POINT` in [`src/game_balance.rs`](../src/game_balance.rs).
5. **If it's a deployable**, the placement reach and damage range come from `DEPLOYABLE_PLACEMENT_REACH_M` / `DEPLOYABLE_DAMAGE_RANGE_M` (see [`game_balance.rs`](../src/game_balance.rs)). The collider half-extents and `station_radius` are per-item.
6. **If the item is a recipe output**, add the recipe to [`src/crafting.rs`](../src/crafting.rs), see "Crafting" below.
7. **If the item should drop from a resource node**, reference it from the appropriate `ResourceNodeDefinition` in [`src/resources.rs`](../src/resources.rs).
8. **Add the item's mesh/material** in the client scene module (`src/app/scene/`). Materials follow the conventions in [docs/materials.md](materials.md).

## Resource nodes

A resource node is a static placeable thing the world spawns at generation time. Defined in [`src/resources.rs`](../src/resources.rs) with:

- `id`: stable string id.
- `kind`: `Tree { species, size }` or `OreVein { ore }` or `Scatter { kind }`.
- `capacity`: how many units the node holds at full.
- `yield_per_swing`: how many units a successful gather grants (clamped by remaining capacity).
- `required_tool_kind`: which `ToolKind` is needed (e.g. `Pickaxe` for ore, `Hatchet` for trees, `Hands` for surface scatter).
- `yields_item_id`: what the gather grants (resolved against the items registry).
- `regrow_after_ticks_range`: jittered respawn window after depletion (typically `(5*60*20, 15*60*20)` for 5–15 min).

Authoritative state lives in `GameServer::resource_nodes` as a `HashMap<ResourceNodeId, ResourceNodeState>`. The ECS mirror in `src/server/resource_node_ecs.rs` carries the replicated component split, see [Networking § Replication](networking.md#replication).

**Client reconciliation.** The client mirrors replicated nodes into local `NetworkResourceNode` visual entities via [`apply_resource_nodes_system`](../src/app/systems/items/resource_nodes.rs). It is **event-driven**, reacts to `Added<ResourceNode>` and `RemovedComponents<ResourceNode>` rather than iterating the full replicated set every frame. The `ResourceNodeEntities` resource carries three state pieces that make this work:

1. `entities: HashMap<ResourceNodeId, Entity>`, forward map id → local mirror entity.
2. `replicated_to_id: HashMap<Entity, ResourceNodeId>`, reverse map so `RemovedComponents` events can find the id of a despawned replicated entity.
3. `pending_spawns: VecDeque<PendingSpawn>`, per-frame spawn budget (`MAX_RESOURCE_NODE_SPAWNS_PER_FRAME = 8`) drains across frames. Persisting the queue across frames is load-bearing: `Added<T>` only fires *once* per entity, so the budget-deferred remainder of an initial AoI fill would be lost without it.

A one-time catch-up scan runs on the first real run after connect (`!applied_first_snapshot`). This handles entities that arrived during early-return frames while `client_id == None` (Bevy's `Added` filter's `last_run` tick advanced past them). After the catch-up, steady-state cost is ~50 µs per frame instead of 1.4-4 ms. The same pattern applies to `maintain_world_grid_system` in [src/app/systems/deployables.rs](../src/app/systems/deployables.rs) and should be applied to any new `apply_*` system that iterates replicated state. See [docs/profiling.md](profiling.md) for the trace-analysis workflow that surfaces this kind of bug.

## How to add a new resource node type

1. **Add a `pub const X_NODE_ID: &str = "x_node";`** at the top of [`src/resources.rs`](../src/resources.rs).
2. **Append an entry to the `RESOURCE_NODE_DEFINITIONS` array** with the full definition.
3. **Decide the spawn rule.** Resource nodes are populated by the chunk generator in [`src/world/chunk/`](../src/world/chunk/), specifically the Poisson-disk spawn pass. If the new node type is biome-specific, extend the chunk-classification → spawn-list mapping there.
4. **Pick a yield item.** It must exist in the items registry; the registry lookup at gather time will silently drop the yield if the id resolves to nothing.
5. **Pick a tool gate.** A `Hands` requirement means anyone can pick it up; a `Pickaxe`/`Hatchet` requirement gates it behind owning the right tool.
6. **Add a render path.** The client side reads the node type from the replicated `ResourceNode` component and dispatches to the appropriate scene system in `src/app/systems/items/resource_nodes.rs`.

## Crafting (preview)

Recipes live in [`src/crafting.rs`](../src/crafting.rs), input list, output item, batch limits, category for the UI filter, and `station` gating (none / `Workbench { min_tier }` / `Furnace`). The crafting modal in [`src/app/ui/crafting.rs`](../src/app/ui/crafting.rs) renders them; the always-on queue HUD in [`src/app/ui/crafting_queue.rs`](../src/app/ui/crafting_queue.rs) shows in-flight jobs. Server-side queue processing lives in [`src/server/crafting.rs`](../src/server/crafting.rs).

Furnaces are a separate path: their state machine lives in [`src/server/furnace/`](../src/server/furnace/) (split into `state.rs`, `tick.rs`, `commands.rs`). Smelt recipes are hardcoded in `state.rs::smelt_result`; today only `iron_ore → iron_bar`. Extending the smelt table is a one-line change.

## ID hygiene

- Never rename a shipped item or node id. Saves embed the string id; rename = corrupted save (rejected at load via the version bump in [`src/save/format.rs`](../src/save/format.rs)).
- Removing an item is allowed but be aware that existing saves carrying that id will fail to load (intentional, the version bump catches it).
- Tool tiers (`tier: u8` on `ToolProfile`) are how the gather/damage system scales; new tools should slot into the tier hierarchy rather than introducing a per-tool damage table. The stone → iron jump (tier 1 → 2, `gather_amount` 6 → 12) is the canonical example: pure data, zero new branches.
- Tool-vs-material effectiveness lives in **one** function, `tool_effectiveness_pct(ToolKind, DestructibleMaterial)` in [`src/items.rs`](../src/items.rs). Every destructible-entity damage path reads through it instead of branching on entity type, so balancing a matchup (hatchet→wood, pickaxe→stone, …) is a one-line edit and a new material (`metal`, `concrete`, …) is one new arm.

## Where to look next

- [docs/networking.md](networking.md) for how item state replicates.
- [docs/materials.md](materials.md) for the PBR material conventions used by item meshes.
- [src/game_balance.rs](../src/game_balance.rs) for the tuning knobs that affect items at runtime (combat damage scalar, placement ranges, furnace timings).
