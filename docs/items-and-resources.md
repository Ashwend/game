---
title: Items, tools, resources, and gather rules
owns: The two compile-time registries (REGISTERED_ITEMS, RESOURCE_NODE_DEFINITIONS), tool/material effectiveness, and the gather payout rule.
when_to_read: Before adding or editing an item, tool, ore, resource node, or gather rule. The step-by-step lives in docs/playbooks/add-content.md.
sources:
  - src/items.rs - ItemDefinition, REGISTERED_ITEMS, tool_effectiveness_pct, DoorVariant, ItemId/intern_item_id
  - src/resources.rs - ResourceNodeDefinition, RESOURCE_NODE_DEFINITIONS, ToolRequirement, next_payout_from_storage
  - src/game_balance.rs - tool durability/PvP-damage constants
  - src/crafting.rs - RecipeStation
  - src/server/chunk_manager/regrow.rs - node respawn timing
related:
  - docs/crafting-and-deployables.md - recipes, the furnace/smelt path, deployable placement and damage
  - docs/base-building-and-claims.md - the authoritative HP/colliders for placed building blocks
  - docs/chunks-and-aoi.md - where node regrow timing lives and how nodes anchor to chunks
  - docs/replication.md - how node/item state replicates and the event-driven client reconciliation pattern
  - docs/toon-shading.md - the cel ToonMaterial the ore boulders (and other props) share
  - docs/playbooks/art-pipeline.md - authoring the held tool glbs and inventory icons
---

# Items, tools, resources, and gather rules

> When to read this: before adding or editing an item, tool, ore, resource node, or gather rule. Source of truth: `src/items.rs`, `src/resources.rs`. Canonical invariants (no-em-dashes, balance-in-game_balance.rs, replicated-state rules) live in CLAUDE.md.

Two compile-time `&'static` registries drive the economy:

- `REGISTERED_ITEMS` in `src/items.rs` is every player-holdable thing: raw materials, four tools, the hammer, the building plan, the deployables, plus six hidden building-block definitions.
- `RESOURCE_NODE_DEFINITIONS` in `src/resources.rs` is every world-spawnable node: three ores, a stone vein, six tree variants, and three crude E-pickup scatter nodes.

Both are slices baked into the binary. Adding an entry means editing the file and recompiling; there is no dynamic loading. This is intentional: the registries are tiny, and tying them to the binary version means a save file's string ids always resolve against a stable set on load.

## ItemDefinition shape

`src/items.rs - ItemDefinition` has exactly these fields. The two an agent most often forgets are `description` and `equipable`; both are required on every entry or the slice fails to compile.

| field | type | role |
| --- | --- | --- |
| `id` | `&'static str` | stable string id (e.g. `"iron_ore"`). Lives forever in saves. |
| `name` | `&'static str` | display name. |
| `description` | `&'static str` | tooltip copy, present on every entry. |
| `stack_size` | `u16` | declared slot limit. |
| `equipable` | `bool` | gates whether the item can be held in hand (read in `held.rs`, `tool_swap.rs`, `inventory/slot.rs`, `server/queries.rs`). Raw materials and the hidden building blocks are `false`. |
| `model` | `ItemModel` | first-person animation archetype: `Bag`, `Hatchet`, `Pickaxe`, `Deployable`. Same-kind tools share an archetype (an iron hatchet swings exactly like a stone one). The hammer uses `Hatchet`. |
| `held_mesh` | `HeldMesh` | first-person mesh: `Bag`, `StoneHatchet`, `IronHatchet`, `StonePickaxe`, `IronPickaxe`, `Hammer`, `BuildingPlan`. Decoupled from `model` so look and animation vary independently. Replicated as a 1-byte selector on `PlayerHeldItem` so peers render the right mesh without shipping the string id. |
| `tint` | `ItemTint` | `ItemTint::new(r, g, b)`, the placeholder/UI tint. |
| `tool` | `Option<ToolProfile>` | present only for tools. |
| `deployable` | `Option<DeployableProfile>` | present only for placeables. |

`effective_stack_size()` short-circuits to `1` when `tool.is_some()`, regardless of the declared `stack_size`, because per-item durability rides on `ItemStack`, so two tools can never share a slot. Everything else stacks to its `stack_size`. The torch is an equipable deployable that is *not* a tool, so it stacks normally (`stack_size: 10`).

Lookup goes through a build-once `OnceLock<HashMap<&'static str, &'static ItemDefinition>>`:

- `item_definition(id) -> Option<&'static ItemDefinition>`
- `stack_limit(id) -> Option<u16>`
- `normalize_stack(stack)` clamps quantity into `[1, stack_limit]` and returns `None` for unknown ids (this is also the choke point that rejects malformed wire input). It **clones** the stack rather than rebuilding via `ItemStack::new`, on purpose, so a worn tool's `durability` survives normalization instead of resetting to factory-fresh.

### ItemId interning

`src/items.rs - ItemId` is `Arc<str>`, not `String`. `intern_item_id(id)` returns the interned `Arc` for an id: registry constants resolve without allocating via an `RwLock<HashMap<Box<str>, Arc<str>>>` cache seeded from `REGISTERED_ITEMS`; unknown ids fall through to a fresh `Arc` that is then cached so subsequent hits also avoid allocating. Clones of an `ItemId` are a refcount bump. Deserialized ids reuse the cached `Arc` on a hit. `RecipeId` in `src/crafting.rs` is the same `Arc<str>` story.

## ToolProfile and tier scaling

`src/items.rs - ToolProfile` carries `kind` (`ToolKind`), `tier: u8`, `gather_amount: u16`, `cooldown_ticks: u64`, `max_durability: Option<u32>`, and `player_damage: u32`.

`ToolKind` is `Hands`, `Axe`, `Pickaxe`, `Hammer`. `Hands` is the `Default` and is synthesized via `HANDS_TOOL` when no tool is held, so the gather pipeline always has a profile to read; it is never a valid gather tool (see the crude-node rule below). The hammer never gathers and never damages (`gather_amount: 0`, `player_damage: 0`); its swing repairs and its wheel upgrades/demolishes.

Tier is how progression scales, with zero per-tool branching:

- Stone tools are `tier: 1`, `gather_amount: 6`. Iron tools are the same `kind` at `tier: 2`, `gather_amount: 12`.
- `ToolRequirement::allows` (`src/resources.rs`) checks `tool.kind == req.kind && tool.tier >= req.min_tier`, so a higher tier automatically satisfies a lower-tier node.
- `max_durability` is the impact budget. Only swings that connect (gather payout, player hit, structure hit) cost a point; whiffs are free. The single wear path is `consume_active_tool_durability` in `src/server/tool_wear.rs`, and the remaining count rides on `ItemStack::durability`. `None` means the tool never wears (`HANDS_TOOL`).
- `player_damage` is per-swing PvP damage before armor; `0` means the swing is rejected rather than landing a zero-damage hit.

`tier`, `gather_amount`, and `cooldown_ticks` are literals inline in `REGISTERED_ITEMS`. Only `max_durability` and `player_damage` pull from `src/game_balance.rs` (`STONE_TOOL_DURABILITY = 200`, `IRON_TOOL_DURABILITY = 600`, `HAMMER_DURABILITY = 600`, and the per-tool `*_PVP_DAMAGE` constants). Do not move tier/gather/cooldown into game_balance; they are intentionally inline.

> The tier-2 gate is currently latent: no node in `RESOURCE_NODE_DEFINITIONS` requires `min_tier 2` (every ore and tree is `min_tier 1`). The iron upgrade is felt only through bigger `gather_amount`/`durability`/PvP, not through gated nodes. If you add a tier-2-only node, that is a new gameplay gate, call it out.

## Gather payout: yield comes from the tool, not the node

There is no `yield_per_swing` field on the node. A node's `storage` is just its finite reservoir; the per-swing quantity scales with the *tool*.

`src/resources.rs - next_payout_from_storage` is the one rule, shared by the server's `next_resource_payout` and the client-side gather prediction:

```
quantity = tool.gather_amount.max(1)            // 6 stone, 12 iron, 1 hands
// walk storage, take the first non-empty stack, grant min(stack.quantity, quantity)
```

Server and client run the identical function so optimistic client gain matches the authoritative payout (test `client_storage_payout_matches_server_node_payout` in `src/resources.rs` checks this for all storage+tool combos). `storage` is a `&[ResourceMaterial]` list even though every current node has exactly one entry; the payout walks the first non-empty stack, do not assume one-item-per-node is enforced.

Reach is one knob per category: `PICKUP_RANGE = 3.4` (`src/items.rs`) for dropped items, `RESOURCE_GATHER_RANGE = 2.75` (`src/resources.rs`) for nodes. The server validates with lenient distance-only checks (`within_pickup_reach` / `within_gather_reach`) plus `PICKUP_SERVER_REACH_SLACK_M = 1.5` (`src/game_balance.rs`) so it doesn't false-reject a target the client already locked.

## ResourceNodeDefinition shape

`src/resources.rs - ResourceNodeDefinition` has exactly these fields. There is **no** `capacity`, **no** `yield_per_swing`, **no** `yields_item_id`, and **no** regrow field.

| field | type | role |
| --- | --- | --- |
| `id` | `&'static str` | stable string id. |
| `name` | `&'static str` | display name. |
| `model` | `ResourceNodeModel` | which mesh family (ore/vein/tree-by-size/crude). Drives the render path and collider. |
| `required_tool` | `ToolRequirement` | `{ kind, min_tier }`, not a bare `ToolKind`. |
| `storage` | `&'static [ResourceMaterial]` | the finite reservoir; each `ResourceMaterial` is `{ item_id, quantity }`. A node spawns full from this. |
| `anchor_height` | `f32` | vertical offset for the targeting anchor. |
| `ray_radius` | `f32` | focus-cylinder radius for look-at targeting. |

Authoritative node state lives in `GameServer::resource_nodes` as `HashMap<ResourceNodeId, ResourceNodeState>`; the ECS mirror in `src/server/resource_node_ecs.rs` carries the replicated component split. See [docs/replication.md](replication.md).

### Crude E-pickup nodes

`SurfaceStone`, `BranchPile`, `HayGrass` carry `ToolRequirement::new(ToolKind::Hands, 0)`. `ToolRequirement::allows` returns `false` for any `Hands` requirement, so **no swing gathers them, even with a tool equipped**: they are E-pickup-only. A `Hands` requirement is the crude-pickup marker, not "gatherable by punching". `ResourceNodeModel::is_crude` drives both the gather-path tool skip and a smaller render scale, and crude nodes have no collider (walk-through). Each holds exactly one item (`stone`/`wood`/`fiber`), the no-tool bootstrap so a fresh player can craft their first crude tools.

### Regrow timing lives in chunk_manager, not on the node

A mined-out node respawns 5 to 15 minutes later (jittered) at a noise-valid chunk position. The window is `MIN_REGROW_TICKS = 5 * 60 * SERVER_TICK_RATE_HZ` and `MAX_REGROW_TICKS = 15 * 60 * SERVER_TICK_RATE_HZ` in `src/server/chunk_manager/mod.rs`; the jitter is applied in `src/server/chunk_manager/regrow.rs`. It is keyed off `SERVER_TICK_RATE_HZ`, not a hardcoded 20. See [docs/chunks-and-aoi.md](chunks-and-aoi.md).

### Tree dead-snag state is server-authoritative

`spawn_resource_node` decides `dead` from `world_seed + position` via `tree_is_dead` (forest-growth noise channel, smoothstep alive-chance, deterministic per-node hash). It is frozen on the `ResourceNodeState` (replicated and saved), not re-derived per client. A `world_seed` of `None` (the client menu backdrop, which neither replicates nor saves) leaves trees alive.

## The single tool-vs-material table

`src/items.rs - tool_effectiveness_pct(ToolKind, DestructibleMaterial) -> u32` is the one place that answers "how well does tool X bite material Y", as an integer percentage multiplier. Every destructible-entity damage path reads through it instead of branching on entity type, so balancing a matchup is a one-line edit and a new material is one new arm.

| tool \ material | Wood | Stone | Sticks | WoodBuilding | StoneBuilding | MetalBuilding | Cloth |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Axe | 150 | 50 | 300 | 15 | 0 | 0 | 300 |
| Pickaxe | 50 | 150 | 200 | 5 | 0 | 0 | 300 |
| Hammer | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Hands | 50 | 50 | 50 | 50 | 50 | 50 | 50 |

Matched proper tool is roughly 1.5x, mismatched proper tool roughly 0.5x. `StoneBuilding` and `MetalBuilding` return `0` for every tool, so those entities are tool-proof by construction (only a future explosives path breaches them). The hammer returns `0` for everything, which closes the "hammer as a free raid tool" hole. `Hands` is a worst-case catch-all (bare hands are normally rejected upstream before reaching here).

**Make a deployable tool-immune by giving it a 0-arm material, never by special-casing the damage handler.** The iron door is the worked example.

## DoorVariant: the immutable-identity pattern

`src/items.rs - DoorVariant` (`HewnLog`, `Iron`) is the canonical example of immutable spawn identity. The variant travels on the replicated `Deployable` component and in the save, and is the single lookup for the door's item id, HP, raid material, and name:

| variant | item id | `max_hp()` | `material()` |
| --- | --- | --- | --- |
| `HewnLog` | `hewn_log_door` | `DOOR_MAX_HP = 1500` | `WoodBuilding` (raidable with tools, slowly) |
| `Iron` | `iron_door` | `IRON_DOOR_MAX_HP = 3000` | `MetalBuilding` (tool-immune, exactly double the HP) |

All accessors are `const fn`. Adding a door is one arm here plus a recipe and a model; nothing in the damage/placement/persistence paths changes.

`DeployableKind` (the broader enum) has eight variants: `Workbench`, `Furnace`, `Building`, `Door`, `SleepingBag`, `StorageBox`, `Torch`, `ToolCupboard`. Its `material()` is the source of truth for each kind's `DestructibleMaterial`; `raidable()` flags which kinds anyone may damage. Placement, damage, and ownership for these live in [docs/crafting-and-deployables.md](crafting-and-deployables.md) and [docs/base-building-and-claims.md](base-building-and-claims.md).

## Full roster

**Raw materials / refined (`Bag` mesh, not equipable):** `wood`, `stone`, `coal`, `iron_ore`, `sulfur_ore`, `fiber`, `plant_twine`, `iron_bar`, `hewn_log`.

**Tools (four, all equipable, all stack to 1):** `wood_stone_hatchet` (Axe t1), `wood_stone_pickaxe` (Pickaxe t1), `iron_hatchet` (Axe t2), `iron_pickaxe` (Pickaxe t2). Every tool is an authored Blender glb matched to its icon; see [docs/playbooks/art-pipeline.md](playbooks/art-pipeline.md).

**Build/utility holdables:** `hammer` (Hammer tool, `Hatchet` animation, `Hammer` mesh), `building_plan` (`BuildingPlan` mesh, no tool/deployable).

**Deployables (equipable, render as the bag in hand):** `workbench_t1`, `crude_furnace`, `hewn_log_door`, `iron_door`, `sleeping_bag`, `torch` (stacks to 10), `storage_box_small`, `storage_box_large`, `tool_cupboard`.

**Six hidden building-block defs** built via `building_piece_item`: foundation, wall, window wall, doorway, ceiling, stairs. Never craftable and never in an inventory; they exist so `DeployedEntity::item_id` resolves through the registry (saves, mirror views, colliders). Their `max_health` and `collider_half_width` are **placeholders**: the authoritative HP and colliders for placed building blocks come from `src/building.rs`, not the registry entry. Do not trust those numbers.

**Resource nodes (13 in `RESOURCE_NODE_DEFINITIONS`):**

- Ores (Pickaxe t1): `coal_node`, `iron_node`, `sulfur_node` (72 each), `stone_node` (Stone Vein, 96 stone).
- Trees (Axe t1, six size variants): `pine_tree_small` / `pine_tree` / `pine_tree_large`, `birch_tree_small` / `birch_tree` / `birch_tree_large` (wood, scaling 18 to 84 by size). The un-suffixed ids (`pine_tree`, `birch_tree`) are the **medium** variants on purpose, so pre-size-variant saves load as medium without migration.
- Crude (Hands, E-pickup): `surface_stone` (1 stone), `branch_pile` (1 wood), `hay_grass` (1 fiber).

## Ore depletion stages (cosmetic, client-side)

Ore and stone-vein nodes step through `ORE_NODE_STAGE_COUNT = 3` meshes while being mined (untouched, worn, gutted), defined in `src/app/scene/mesh/ore.rs`. The stage meshes are authored glbs (`assets/ore/<type>/stage_<n>.glb`, built by `art/ore/build_ore.py`) loaded into `ResourceVisualAssets` in `src/app/scene/assets.rs`.

The ore boulders are cel-shaded via the shared `ToonMaterial` (`src/app/scene/toon.rs`), the same material the deployable props and trees use; ores are no longer the only cel family. The spawn path (`src/app/systems/items/resource_nodes/spawn.rs`) imports `ToonMaterial`, not the old `OreToonMaterial`. See [docs/toon-shading.md](toon-shading.md).

Staging is purely cosmetic and entirely client-side. `apply_resource_node_stage_system` (`src/app/systems/items/resource_nodes/stages.rs`) watches `Changed<ResourceNodeStorage>`, maps remaining fraction to a stage (`ORE_STAGE_WORN_BELOW = 0.70`, `ORE_STAGE_GUTTED_BELOW = 0.35`), and on a real crossing swaps the mirror's `Mesh3d`, fires a half-magnitude ore-shatter burst, and plays `OreStageCrumble`. Full depletion plays the full shatter plus `OreNodeBreak`. Gather rules, colliders, and targeting are untouched by stage. Part-mined nodes from replication or a save spawn directly at the correct stage mesh.

Admin helper: `/drain [remaining-fraction]` (function `command_drain` in `src/server/commands/world.rs`) sets the looked-at node's storage absolutely (default 0.5; accepts `0..=1` or a percentage like `/drain 40`; `0` removes the node through the regular depletion path), exercising the full replication chain without forty pickaxe swings.

## Client reconciliation is event-driven

The client mirrors replicated nodes into local `NetworkResourceNode` visuals via `apply_resource_nodes_system` (`src/app/systems/items/resource_nodes/mod.rs`), reacting to `Added<ResourceNode>` and `RemovedComponents<ResourceNode>`, never iterating the full replicated set per frame (that costs 1 to 4 ms at AoI scale). The `ResourceNodeEntities` resource holds a forward `id -> Entity` map, a reverse `Entity -> id` map (so `RemovedComponents` can find the local mirror), and a `pending_spawns: VecDeque` that drains a per-frame spawn budget across frames. A one-time catch-up scan on the first run after connect handles entities that arrived during early-return `client_id == None` frames. `Ref::is_changed()` lies for Lightyear-touched components, so do not gate work behind it. This is the canonical pattern for any new `apply_*` system; the full rationale is in [docs/replication.md](replication.md) and CLAUDE.md.

## Smelting is not in the recipe registry

`src/crafting.rs - RecipeStation` has only two variants: `None` (hand-craftable) and `Workbench { min_tier }`. A workbench tier 2 satisfies a tier-1 requirement, mirroring tool tiers. **There is no `Furnace` variant.** Smelting happens inside the furnace's own UI, not the recipe registry: `smelt_result` in `src/server/furnace/state.rs` today maps only `iron_ore -> iron_bar`, and extending it is a one-line change. The recipe queue, furnace state machine, loot bags, and deployable damage all live in [docs/crafting-and-deployables.md](crafting-and-deployables.md).

## ID hygiene

- Never rename a shipped item or node id; saves embed the string id. Tree ids are deliberately un-suffixed for medium so old saves load without migration.
- Removing an item is allowed, but existing saves carrying that id fail to load (intentional, caught by the version bump in `src/save/format.rs`).
- New tools slot into the `tier` hierarchy rather than introducing a per-tool damage table. The stone to iron jump (tier 1 to 2, gather 6 to 12) is the canonical pure-data example.
- A node's yield item must exist in the registry; an unresolved id silently drops the yield at gather time.

## Related docs

- [docs/crafting-and-deployables.md](crafting-and-deployables.md) - recipes, the furnace/smelt path, deployable placement and damage, loot bags.
- [docs/base-building-and-claims.md](base-building-and-claims.md) - authoritative HP/colliders for placed building blocks, stability, doors, Tool Cupboard claims.
- [docs/chunks-and-aoi.md](chunks-and-aoi.md) - node regrow scheduling and how nodes anchor to chunks.
- [docs/replication.md](replication.md) - how node/item state replicates and the event-driven client reconciliation pattern.
- [docs/toon-shading.md](toon-shading.md) - the cel `ToonMaterial` the ore boulders and other props share.
- [docs/playbooks/art-pipeline.md](playbooks/art-pipeline.md) - authoring held tool glbs and inventory icons.
- [docs/pvp-combat.md](pvp-combat.md) - how `player_damage` feeds combat and the durability-on-hit path.
</content>
