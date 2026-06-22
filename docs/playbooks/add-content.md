---
title: "Playbook: add a tool, ore, recipe, smeltable, or deployable"
owns: The compile-accurate step-by-step for the common content additions (tool, resource node, recipe, smelt recipe, deployable kind).
when_to_read: When the task is "add a new tool / ore / tree node / recipe / smeltable / deployable", or when you need the exact field checklist for one of those registries.
sources:
  - src/items.rs - ItemDefinition, REGISTERED_ITEMS, ToolProfile, DeployableKind, tool_effectiveness_pct
  - src/resources.rs - ResourceNodeDefinition, RESOURCE_NODE_DEFINITIONS, ToolRequirement, next_payout_from_storage
  - src/crafting.rs - RecipeDefinition, REGISTERED_RECIPES, RecipeStation
  - src/server/furnace/state.rs - smelt_result, fuel_burn_ticks_for
  - src/server/deployables.rs - DeployedEntity, apply_place_deployable_command, restore_deployed_entities, spill_container_contents
  - src/world/chunk/mod.rs - NodeKind, definition_id, from_definition_id
related:
  - docs/items-and-resources.md - the registry reference this playbook operates on
  - docs/crafting-and-deployables.md - the crafting/furnace/deployable system reference
  - docs/base-building-and-claims.md - building piece HP/cost tables and claims
  - docs/playbooks/add-replicated-entity.md - the wire/replication contract a brand-new networked entity must follow
  - docs/playbooks/art-pipeline.md - authoring the icon, held mesh, or prop glb
  - docs/worlds-and-saves.md - save format and the version-bump rule
---

# Playbook: add a tool, ore, recipe, smeltable, or deployable

> When to read this: you are adding a new tool, ore/tree node, recipe, smelt recipe, or deployable kind. Source of truth: `src/items.rs`, `src/resources.rs`, `src/crafting.rs`, `src/server/furnace/state.rs`, `src/server/deployables.rs`. Canonical invariants live in CLAUDE.md.

Every addition here is data appended to a compile-time `&'static` registry plus, for some, one server extension point. The economy is driven by two registries (`REGISTERED_ITEMS`, `RESOURCE_NODE_DEFINITIONS`) and one recipe table (`REGISTERED_RECIPES`); smelting and deployables each have a small set of named extension points. Pick the right section, fill every field, then read [Append-only rules](#append-only-enum-stable-ids-save-format-bump) before you ship.

The reference docs ([items-and-resources.md](../items-and-resources.md), [crafting-and-deployables.md](../crafting-and-deployables.md)) explain the systems. This doc is only the procedures.

## Add a tool

A tool is an `ItemDefinition` (`src/items.rs` - `ItemDefinition`) with a `Some(ToolProfile)`. Append to `REGISTERED_ITEMS` (`src/items.rs` - `REGISTERED_ITEMS`).

1. Pick a stable id: snake_case, lowercase, no version suffix. Add `pub const X_ID: &str = "x";` near the top of `src/items.rs` so call sites reference it symbolically. The id lives forever in saves (see [Append-only rules](#append-only-enum-stable-ids-save-format-bump)).
2. Append the full `ItemDefinition`. Every field is required and the struct does not compile if you omit one:
   - `id: X_ID`, `name: "Display Name"`, `description: "..."` (tooltip copy, present on every entry), `tint: ItemTint::new(r, g, b)`.
   - `stack_size: u16`. Tools declare `1`, but note `effective_stack_size()` (`src/items.rs` - `ItemDefinition::effective_stack_size`) forces `1` for anything with `tool.is_some()` regardless, because per-item durability rides on the `ItemStack`. Two tools can never share a slot.
   - `equipable: true` (a tool must be holdable; `equipable` gates whether the item can be put in hand).
   - `model: ItemModel::Hatchet` or `ItemModel::Pickaxe` (`src/items.rs` - `ItemModel`). This is the swing-pose archetype, not the mesh. Iron and stone of the same kind share an archetype. The hammer animates as `ItemModel::Hatchet`. New tool materials of an existing kind reuse the existing archetype.
   - `held_mesh: HeldMesh::...` (`src/items.rs` - `HeldMesh`). Existing variants: `StoneHatchet`, `IronHatchet`, `StonePickaxe`, `IronPickaxe`, `Hammer`, `BuildingPlan`, `Bag`. A brand-new tool look is a new `HeldMesh` variant plus the mesh wiring in `src/app/scene/assets.rs`; the four tools are authored glbs (see [art-pipeline.md](art-pipeline.md)).
   - `tool: Some(ToolProfile { ... })` (`src/items.rs` - `ToolProfile`):
     - `kind: ToolKind::Axe | ToolKind::Pickaxe | ToolKind::Hammer | ToolKind::Hands` (`src/items.rs` - `ToolKind`). `Hammer` never gathers and never damages (it repairs/upgrades/demolishes via its own commands). `Hands` is the synthesized empty-hand profile, not a real item.
     - `tier: u8`. Stone tools are tier 1, iron tier 2. A higher tier satisfies every lower-tier node requirement automatically (`ToolRequirement::allows` checks `tool.tier >= req.min_tier`). `tier`, `gather_amount`, and `cooldown_ticks` are inline literals in `REGISTERED_ITEMS`, not in `game_balance.rs`.
     - `gather_amount: u16`. Units granted per successful swing, clamped to remaining node storage. This is what makes a tier upgrade felt: stone tools gather 6, iron 12. The node does not define a per-swing yield; the tool does.
     - `cooldown_ticks: u64`. Swing cadence in practice is gated by the swing animation, not this value (see `src/server` gather path), so cadence barely changes between tiers.
     - `max_durability: Option<u32>`. `None` means it never wears (only `HANDS_TOOL`). Only connecting swings (gather, player hit, structure hit) consume durability; whiffs are free. Pull the value from `game_balance.rs` (`STONE_TOOL_DURABILITY` for stone tools, `IRON_TOOL_DURABILITY` for iron; the hammer uses its own `HAMMER_DURABILITY`).
     - `player_damage: u32`. Raw per-swing PvP damage before armor; `0` means the tool cannot hit players (bare hands, hammer). Pull from `game_balance.rs` (e.g. `STONE_HATCHET_PVP_DAMAGE`).
   - `deployable: None`.
3. If the tool is craftable, add its recipe (see [Add a recipe](#add-a-recipe)).
4. Add the inventory icon and held mesh per [art-pipeline.md](art-pipeline.md). A missing icon falls back to a tinted rectangle, so it is optional for a working item, expected for a shipped one.

Notes:
- Tools never reach `tool_effectiveness_pct` for gathering; that table is the destructible-entity damage path. To make a tool that bites a material differently, edit the matchup arm in `tool_effectiveness_pct` (`src/items.rs` - `tool_effectiveness_pct`), do not special-case the damage handler.
- The tier-2 gate is currently latent: no resource node requires `min_tier 2`, so iron tools are felt only through bigger `gather_amount`, durability, and PvP, not through gated nodes.

## Add an ore or tree node

A resource node is a static thing the world spawns at generation time. Definition is a `ResourceNodeDefinition` (`src/resources.rs` - `ResourceNodeDefinition`) appended to `RESOURCE_NODE_DEFINITIONS` (`src/resources.rs` - `RESOURCE_NODE_DEFINITIONS`). There are 13 today: 3 ores, 1 stone vein, 6 tree variants (small/medium/large pine and birch), 3 crude E-pickup scatter nodes.

1. Add `pub const X_NODE_ID: &str = "x_node";` near the top of `src/resources.rs`.
2. Append the `ResourceNodeDefinition`. Fields (note: there is no `capacity`, no per-swing yield, and no regrow field on the node):
   - `id: X_NODE_ID`, `name: "Display Name"`.
   - `model: ResourceNodeModel::...` (`src/resources.rs` - `ResourceNodeModel`). Drives the client mesh and the `is_tree`/`is_ore`/`is_crude` classification (collider shape, render scale, gather-skip).
   - `required_tool: ToolRequirement::new(kind, min_tier)` (`src/resources.rs` - `ToolRequirement`). `ToolRequirement::allows` rejects a `Hands` requirement for any swing, so `ToolRequirement::new(ToolKind::Hands, 0)` marks the node E-pickup-only. Ore/stone use `Pickaxe`, trees use `Axe`. `min_tier` 1 means any pickaxe/hatchet works.
   - `storage: &[ResourceMaterial::new(item_id, quantity)]`. This is the node's finite reservoir, not a per-swing yield. It is a list, but every current node has exactly one entry; the payout walks the first non-empty stack. Per-swing yield is `tool.gather_amount` clamped to remaining storage (`src/resources.rs` - `next_payout_from_storage`, shared by server and client prediction so they cannot disagree). The yield `item_id` must exist in `REGISTERED_ITEMS` or the payout is silently dropped.
   - `anchor_height: f32`, `ray_radius: f32`. The targeting focus box. Set the anchor to the middle of the visible model and the radius wide enough that looking at the mesh focuses it.
3. Wire it into chunk generation so the world actually spawns it. Nodes are placed by the chunk generator's Poisson-disk pass (`src/world/chunk/generator.rs`). The generator works in terms of `NodeKind` (`src/world/chunk/mod.rs` - `NodeKind`), not definition-id strings. The definition-id <-> kind mapping must agree in both directions:
   - `NodeKind::definition_id` (and `variant_definition_id` for trees) maps a kind to the registry id the generator spawns.
   - `NodeKind::from_definition_id` is the reverse, used for chunk membership and the regrow scheduler. If a new id is not in both, the generator either never spawns it or the regrow/capacity bookkeeping disagrees.
   - Per-kind density and spacing live in `chunk_kind_target` / `kind_target` (`src/world/chunk/generator.rs`) and `min_spacing_m` / `base_capacity` (`src/world/chunk/mod.rs`, `classification.rs`). The same `kind_target` formula is shared by generation and the regrow ceiling; they must stay one function or the world over- or under-fills.
4. Add the client render path. The client reads the node's model from the replicated component and dispatches in `src/app/systems/items/resource_nodes/spawn.rs` (the `resource_nodes` module is a directory; reconciliation lives in its `mod.rs`). Reconciliation is event-driven (`Added`/`RemovedComponents`), not per-frame iteration; see CLAUDE.md's replicated-state rules and [add-replicated-entity.md](add-replicated-entity.md) before touching it. You do not add new replicated components for a new node type; the existing `ResourceNode` split already carries definition id, position, and storage.

Notes:
- Tree dead-snag state is decided server-side from `world_seed + position` (`src/resources.rs` - `spawn_resource_node`, `tree_is_dead`), frozen on the node, and replicated. It is not re-derived per client.
- Ore/stone-vein depletion swaps through stage meshes (thresholds at 70% and 35% remaining, `src/app/systems/items/resource_nodes/stages.rs`). This is purely cosmetic and client-side; gather/collider/targeting are untouched. To exercise depletion without swinging, the admin command is `/drain [remaining-fraction]` (`src/server/commands/world.rs` - `command_drain`), e.g. `/drain 0.4`; `/drain 0` removes the node through the normal depletion path.
- Crude nodes (`is_crude`) are walk-through (no collider), render smaller, and are E-pickup-only.

## Add a recipe

A recipe is a `RecipeDefinition` (`src/crafting.rs` - `RecipeDefinition`) appended to `REGISTERED_RECIPES` (`src/crafting.rs` - `REGISTERED_RECIPES`). The output `item_id` must already exist in `REGISTERED_ITEMS`.

1. Add `pub const X_RECIPE_ID: &str = "x";` near the top of `src/crafting.rs`. Recipe ids are stable: queued jobs and saves reference them.
2. Append the `RecipeDefinition`:
   - `id`, `name`, `description`.
   - `category: RecipeCategory::Materials | Tools | Building | Misc` (`src/crafting.rs` - `RecipeCategory`). Drives the browser filter only.
   - `inputs: &[CraftingInput::new(item_id, quantity)]`. Consumed at enqueue, not on completion.
   - `output_item`, `output_quantity`.
   - `craft_seconds: f32`. Server converts to ticks via `SERVER_TICK_RATE_HZ` (20.0).
   - `tier: u8`. Sort order in the browser only (higher surfaces first).
   - `station: RecipeStation::None | RecipeStation::Workbench { min_tier }` (`src/crafting.rs` - `RecipeStation`). Those are the only two variants. A `Workbench { tier: 2 }` deployable in range satisfies a `Workbench { min_tier: 1 }` requirement, mirroring tool tiers. There is no `Furnace` station: smelting is not a recipe (see next section).
3. That is the whole change. The id index, category iteration, and the browser/queue UI follow automatically. Crafting is strictly serial per player; queue and batch caps live in [crafting-and-deployables.md](../crafting-and-deployables.md).

## Add a smelt recipe or fuel

Smelting is deliberately not in the recipe registry. It runs inside the furnace's own UI and state machine (`src/server/furnace/`). There are exactly two extension points, both in `src/server/furnace/state.rs`:

- New smeltable: add an arm to `smelt_result` (`src/server/furnace/state.rs` - `smelt_result`). Today it maps only `iron_ore -> iron_bar`. Return `Some(ItemStack::new(OUTPUT_ID, qty))` for the new input id. Both input and output items must exist in `REGISTERED_ITEMS`.
- New fuel: add an arm to `fuel_burn_ticks_for` (`src/server/furnace/state.rs` - `fuel_burn_ticks_for`). Today it maps `wood -> WOOD_BURN_TICKS` and `coal -> COAL_BURN_TICKS`. Return `Some(burn_ticks)` for the new fuel id. The smelt timing constants (`FURNACE_SMELT_TICKS_PER_OUTPUT`, `FURNACE_WOOD_BURN_TICKS`, `FURNACE_COAL_BURN_TICKS`) live in `game_balance.rs`.

Both are one-line edits. Furnace slot layout (`FURNACE_ITEM_SLOT_COUNT` smelt slots + 1 fuel slot at index 0) and the owner-private replication path (`OpenFurnaceView`) are documented in [crafting-and-deployables.md](../crafting-and-deployables.md).

## Add a deployable kind

Every placed object (workbench, furnace, building block, door, sleeping bag, storage box, torch, tool cupboard) is one `DeployedEntity` struct (`src/server/deployables.rs` - `DeployedEntity`) carrying an `Option<>` per kind-specific sub-state. `DeployableKind` (`src/items.rs` - `DeployableKind`) has 8 variants today. A new kind is a coordinated change across the item registry, the kind enum, and the server lifecycle.

1. Item entry: add `pub const X_ID: &str = "x";`, append an `ItemDefinition` to `REGISTERED_ITEMS` with `equipable: true`, `model: ItemModel::Deployable`, `held_mesh: HeldMesh::Bag`, `tool: None`, and `deployable: Some(DeployableProfile { ... })` (`src/items.rs` - `DeployableProfile`): `kind`, `max_health`, `collider_half_width`, `collider_half_height`, `station_radius` (0.0 if it is not a crafting station).
2. Kind enum: add a `DeployableKind` variant. The variant is immutable spawn identity: it rides the replicated `Deployable` component and the save, and feeds `material()` (the tool-vs-material lever) and `raidable()` (whether non-owners may damage it). Update both:
   - `DeployableKind::material` -> the `DestructibleMaterial` it is built from. Give it a tool-immune material (`StoneBuilding` or `MetalBuilding`, both return 0 for every tool in `tool_effectiveness_pct`) to make it raid-proof; never special-case the damage handler.
   - `DeployableKind::raidable` -> `true` only if non-owners may damage it (building blocks, doors, sleeping bags, tool cupboard). Non-raidable player-placed kinds (workbench, furnace) are owner-damageable only; admins bypass.
3. Sub-state (only if the kind needs mutable per-entity state): add an `Option<SubState>` field to `DeployedEntity` and initialise it in five places, or the state is lost:
   - Init on place: set it in `apply_place_deployable_command` (`src/server/deployables.rs` - `apply_place_deployable_command`), the way furnaces get `FurnaceState::default()`, storage boxes get a fresh grid, and cupboards get the placer's authorized list. Free deployables not on the standard surface/overlap path take their own placer (torches use `place_torch`).
   - Restore on load: map it in `restore_deployed_entities` (`src/server/deployables.rs` - `restore_deployed_entities`) from the persisted record.
   - Persist on save: map it in `persisted_deployed_entities` and add the `Option<PersistedXState>` field to `PersistedDeployedEntity` (`src/save/types.rs`).
   - Spill on destroy (only if it is a container): extend `spill_container_contents` (`src/server/deployables.rs` - `spill_container_contents`) so breaking it drops the contents as a loot bag, the way storage boxes and furnaces spill.
   - Mutate through the dirty flag: when changing a replicated field post-spawn, go through `deployed_entity_mut` / `mark_deployable_dirty` so the mirror re-syncs. A bare `deployed_entities.get_mut` bypasses the dirty flag and silently drops the diff.
4. Replication: the deployable mirror sync (`src/net/host/mirror.rs` - `sync_deployable_entities`) spawns the mirror entity via `attach_room_gated_replication` (`src/net/host/rooms.rs`), which attaches `ReplicationGroup::new_from_entity()`. You do not write a new spawn site; reuse the existing one. Read CLAUDE.md's replicated-state rules and [add-replicated-entity.md](add-replicated-entity.md) before adding any new replicated component for a kind. Identity (including the kind) is immutable post-spawn: a tier upgrade respawns the mirror entity rather than mutating the kind in place.
5. Client: add placement preview/snapping (`src/app/systems/deployables/placement.rs`) and the structure mesh/material in `src/app/scene/`.

Notes:
- Stability and claim footprints are derived, never persisted; `restore_deployed_entities` seeds `stability = 100` and a post-load `refresh_structural_stability` recomputes them. Do not save them.
- Doors are not a separate deployable lifecycle: a new door is one `DoorVariant` arm (`src/items.rs` - `DoorVariant`: item id, HP, material, label) plus a recipe and a model. Nothing in placement, damage, replication, or persistence changes. The iron door is the worked example (`MetalBuilding` material = tool-immune, double HP).

## Append-only enum, stable ids, save-format bump

Saves are positional postcard (`src/save/format.rs`), so layout is load-bearing. The save format is currently at `SAVE_FORMAT_VERSION = 17` (`src/save/format.rs` - `SAVE_FORMAT_VERSION`).

- Never rename or reorder a shipped item id, node id, or recipe id. Saves embed the string id; a rename is a corrupted save. Tree ids are deliberately un-suffixed for the medium variant (`pine_tree`, `birch_tree`) so pre-size-variant saves load as medium without migration.
- Enum variants that travel in saves or on the wire (`DeployableKind`, `DoorVariant`, `BuildingPiece`, `BuildingTier`, `NodeKind`, `RecipeCategory`) are positional. New variants MUST be appended at the end. Reordering silently reinterprets old saves.
- Any change to a persisted struct's field layout, including adding a field to a previously fieldless enum variant (such as `Door` gaining `variant`), changes the byte layout. That requires a `SAVE_FORMAT_VERSION` bump. Old saves are then rejected at load (intentional). See [worlds-and-saves.md](../worlds-and-saves.md) for the bump procedure and the golden-hash test that enforces it.
- Appending an item, node, or recipe definition (no struct-layout change) does NOT require a version bump; only changes to a persisted struct's shape do.
- Removing an item or node id is allowed, but existing saves carrying it fail to load. `restore_deployed_entities` already drops persisted entries whose item id no longer resolves, so a retired deployable type degrades gracefully rather than crashing.
- Item ids are interned to `Arc<str>` (`src/items.rs` - `intern_item_id`): known ids resolve without allocating, deserialized ids reuse the cached `Arc`. When normalizing stacks, `normalize_stack` clones rather than rebuilds so a worn tool's durability survives; never replace a stack with a fresh `ItemStack::new` (it resets durability to factory-fresh).

## Related docs

- [docs/items-and-resources.md](../items-and-resources.md) - the item/tool/resource registry reference these procedures edit.
- [docs/crafting-and-deployables.md](../crafting-and-deployables.md) - the crafting queue, furnace state machine, and unified deployable system.
- [docs/base-building-and-claims.md](../base-building-and-claims.md) - building piece HP/cost tables, doors, stability, and Tool Cupboard claims.
- [docs/playbooks/add-replicated-entity.md](add-replicated-entity.md) - the wire/replication contract for any new networked entity.
- [docs/playbooks/art-pipeline.md](art-pipeline.md) - authoring the icon, held mesh, or prop glb.
- [docs/worlds-and-saves.md](../worlds-and-saves.md) - save format, the version-bump rule, and the golden-hash test.
