---
title: Items, tools, resources, and gather rules
owns: The two compile-time registries (REGISTERED_ITEMS, RESOURCE_NODE_DEFINITIONS), tool/material effectiveness, and the gather payout rule.
when_to_read: Before adding or editing an item, tool, ore, resource node, or gather rule. The step-by-step lives in docs/playbooks/add-content.md.
sources:
  - src/items/registry.rs - ItemDefinition, REGISTERED_ITEMS, the item rows
  - src/items/materials.rs - tool_effectiveness_pct, explosive_effectiveness_pct, DestructibleMaterial
  - src/items/tools.rs, weapons.rs, armor.rs, ranged.rs, explosives.rs, consumables.rs - the per-category profile structs and enums
  - src/server/heal.rs - apply_player_heal, tick_consumable_uses, tick_heal_over_time
  - src/items/deployables.rs, upgrades.rs - DeployableKind, DoorVariant, DEPLOYABLE_UPGRADES
  - src/items/ids.rs - id string consts, intern_item_id/ItemId
  - src/resource_nodes.rs - ResourceNodeDefinition, RESOURCE_NODE_DEFINITIONS, ToolRequirement, next_payout_from_storage
  - src/game_balance.rs - tool durability/PvP-damage constants, weapon/armor/explosive constants
  - src/crafting/types.rs - RecipeStation
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

> When to read this: before adding or editing an item, tool, ore, resource node, or gather rule. Source of truth: `src/items/`, `src/resource_nodes.rs`. Canonical invariants (no-em-dashes, balance-in-game_balance.rs, replicated-state rules) live in CLAUDE.md.

Two compile-time `&'static` registries drive the economy:

- `REGISTERED_ITEMS` (in `src/items/registry.rs`) is every player-holdable thing: raw materials and refined intermediates, the tools, the hammer, the building plan, the weapons / armor / ranged weapons / explosives, the deployables, plus six hidden building-block definitions.
- `RESOURCE_NODE_DEFINITIONS` in `src/resource_nodes.rs` is every world-spawnable node: three ores, a stone vein, the meteorite node, six tree variants, and three crude E-pickup scatter nodes.

Both are slices baked into the binary. Adding an entry means editing the file and recompiling; there is no dynamic loading. This is intentional: the registries are tiny, and tying them to the binary version means a save file's string ids always resolve against a stable set on load.

`src/items.rs` is now a thin front that re-exports the `src/items/` directory (`registry.rs` for the rows, `materials.rs` for the effectiveness tables and `DestructibleMaterial`, `tools.rs`/`weapons.rs`/`armor.rs`/`ranged.rs`/`explosives.rs`/`consumables.rs` for the per-category profile structs and their enums, `deployables.rs`/`upgrades.rs` for `DeployableKind` and the upgrade table, `ids.rs` for the id consts, `visual.rs` for the mesh/model enums, `pickup.rs`). Call sites still say `crate::items::X`; the flat re-export means the split did not churn them.

## ItemDefinition shape

`src/items/registry.rs - ItemDefinition` has exactly these fields. The two an agent most often forgets are `description` and `equipable`; both are required on every entry or the slice fails to compile. The six `Option<*Profile>` fields are a component-style shape: an item carries exactly the profiles that apply to it (a weapon has `weapon`, an armor piece has `armor`, most rows have none).

| field | type | role |
| --- | --- | --- |
| `id` | `&'static str` | stable string id (e.g. `"iron_ore"`). Lives forever in saves. |
| `name` | `&'static str` | display name. |
| `description` | `&'static str` | tooltip copy, present on every entry. |
| `stack_size` | `u16` | declared slot limit. |
| `equipable` | `bool` | gates whether the item can be held in hand (read in `held.rs`, `tool_swap.rs`, `inventory/slot.rs`, `server/queries.rs`). Raw materials and the hidden building blocks are `false`. |
| `model` | `ItemModel` | first-person animation archetype. Now `Bag`, `Hatchet`, `Pickaxe`, `Deployable` plus the melee archetypes (club chop, spear thrust, sword arc, mace overhead) and the bow/crossbow draw archetypes. Same-kind items share an archetype. |
| `held_mesh` | `HeldMesh` | first-person mesh selector: the tools (`StoneHatchet`, `IronHatchet`, `StonePickaxe`, `IronPickaxe`), `Hammer`, `BuildingPlan`, plus the weapon and explosive-prop meshes. Decoupled from `model` so look and animation vary independently. Replicated as a 1-byte selector on `PlayerHeldItem` so peers render the right mesh without shipping the string id. |
| `tint` | `ItemTint` | `ItemTint::new(r, g, b)`, the placeholder/UI tint. |
| `tool` | `Option<ToolProfile>` | present only for gather tools (`src/items/tools.rs`). |
| `weapon` | `Option<WeaponProfile>` | present only for dedicated melee weapons (`src/items/weapons.rs`): `pvp_damage`, `reach_m`, `cooldown_ticks`, `knockback_speed`, `armor_pierce_pct`, `max_durability`. Combat resolution reads `weapon` first, then falls back to `tool`. |
| `ranged` | `Option<RangedProfile>` | present only for the bow and crossbow (`src/items/ranged.rs`): draw/damage band, projectile speed, cooldown, ammo item. |
| `armor` | `Option<ArmorProfile>` | present only for armor pieces (`src/items/armor.rs`): the worn `EquipmentSlot`, per-kind protection (melee / projectile / blast), and durability. |
| `explosive` | `Option<ExplosiveProfile>` | present only for the four charges (`src/items/explosives.rs`): `ExplosiveKind`, base damage, radius, fuse ticks, delivery (`Placed` / `PlacedSticky` / `Thrown`), max health. |
| `consumable` | `Option<ConsumableProfile>` | present only for the bandage (`src/items/consumables.rs`): charge ticks, instant heal, heal-over-time and its window, use movement slow. A consumable is held down and applies only when the charge completes **on the server's clock**. |
| `deployable` | `Option<DeployableProfile>` | present only for placeables. |

`effective_stack_size()` short-circuits to `1` when the item carries per-item durability (tools, weapons, armor: two worn items can never share a slot). Everything else stacks to its `stack_size`. The torch is an equipable deployable that is *not* a tool, so it stacks normally (`stack_size: 10`); arrows stack to 24.

Lookup goes through a build-once `OnceLock<HashMap<&'static str, &'static ItemDefinition>>`:

- `item_definition(id) -> Option<&'static ItemDefinition>`
- `stack_limit(id) -> Option<u16>`
- `normalize_stack(stack)` clamps quantity into `[1, stack_limit]` and returns `None` for unknown ids (this is also the choke point that rejects malformed wire input). It **clones** the stack rather than rebuilding via `ItemStack::new`, on purpose, so a worn tool's `durability` survives normalization instead of resetting to factory-fresh.

### ItemId interning

`src/items/registry.rs - ItemId` is a newtype over `Arc<str>` (`#[serde(transparent)]`, so it encodes exactly like the bare string), deliberately distinct from `RecipeId` so passing one where the other is expected is a compile error. `intern_item_id(id)` returns the interned id: registry constants resolve without allocating via an `RwLock<HashMap<Box<str>, Arc<str>>>` cache seeded from `REGISTERED_ITEMS`; unknown ids fall through to a fresh `Arc` that is then cached so subsequent hits also avoid allocating. Clones of an `ItemId` are a refcount bump. Deserialized ids reuse the cached `Arc` on a hit (interning also runs inside the newtype's own `Deserialize`, so ids decode deduped even without the protocol's `deserialize_with` hooks). The newtype derefs to `str` and offers `as_str`/`AsRef<str>`/`Borrow<str>`/`Display`, plus `From<&str>` that routes through the interner; `ptr_eq` is the test hook for the interning guarantee. `RecipeId` in `src/crafting/types.rs` is the same newtype-over-`Arc<str>` story.

## ToolProfile and tier scaling

`src/items/tools.rs - ToolProfile` carries `kind` (`ToolKind`), `tier: u8`, `gather_amount: u16`, `cooldown_ticks: u64`, `max_durability: Option<u32>`, and `player_damage: u32`.

### Consumables (healing)

The bandage is the only consumable and the only healing in the game outside a respawn. Its shape is deliberately the **bow's**, not the powder bomb's: the client sends only `UseStart` / `UseCancel`, and the server stamps the start tick, re-derives the charge from its own ticks, and applies the heal itself when it completes. There is no "apply" message, because a forged one would be a free instant heal (the bow can safely accept a client `Fire` only because a forged early release deals *less* damage; a heal has no such gradient). Releasing early is a clean cancel and costs nothing: the item is spent on completion, never on the press.

Healing routes through a single `apply_player_heal` tail (`src/server/heal.rs`), mirroring the single `apply_player_damage` tail on the damage side, so the clamp to `MAX_HEALTH` and the don't-heal-a-corpse rule are stated once. The value splits into an instant chunk and a heal-over-time remainder; the trickle accumulates sub-point in a server-only field and only writes `controller.health` on whole points, or a 10-second heal would ship a replication diff every tick. Balance lives in `game_balance.rs` (`BANDAGE_*`).

`ToolKind` is `Hands`, `Axe`, `Pickaxe`, `Hammer`, `Sickle`. `Hands` is the `Default` and is synthesized via `HANDS_TOOL` when no tool is held, so the gather pipeline always has a profile to read; it is never a valid gather tool (see the crude-node rule below). The hammer never gathers and never damages (`gather_amount: 0`, `player_damage: 0`); its swing repairs and its wheel upgrades/demolishes. The sickle satisfies exactly one node's `ToolRequirement` (the Tall Grass tuft) and does zero structure damage (see the sickle section below).

Tier is how progression scales, with zero per-tool branching:

- Stone tools are `tier: 1`, `gather_amount: 6`. Iron tools are the same `kind` at `tier: 2`, `gather_amount: 12`.
- `ToolRequirement::allows` (`src/resource_nodes.rs`) checks `tool.kind == req.kind && tool.tier >= req.min_tier`, so a higher tier automatically satisfies a lower-tier node.
- `max_durability` is the impact budget. Only swings that connect (gather payout, player hit, structure hit) cost a point; whiffs are free. The single wear path is `consume_active_tool_durability` in `src/server/tool_wear.rs`, and the remaining count rides on `ItemStack::durability`. `None` means the tool never wears (`HANDS_TOOL`).
- `player_damage` is per-swing PvP damage before armor; `0` means the swing is rejected rather than landing a zero-damage hit.

`tier`, `gather_amount`, and `cooldown_ticks` are literals inline in `REGISTERED_ITEMS`. Only `max_durability` and `player_damage` pull from `src/game_balance.rs` (`STONE_TOOL_DURABILITY = 200`, `IRON_TOOL_DURABILITY = 600`, `HAMMER_DURABILITY = 600`, and the per-tool `*_PVP_DAMAGE` constants). Do not move tier/gather/cooldown into game_balance; they are intentionally inline.

> The tier-2 gate went live with the meteorite node: `METEORITE_NODE_ID` is the one row that requires `min_tier 2` (an iron pickaxe; a stone pickaxe is rejected). Every ore and tree is still `min_tier 1`, felt only through bigger `gather_amount`/`durability`/PvP. Meteorite is the first node that hard-gates on the iron upgrade, so it sits behind the furnace + iron loop; its worldgen (far rocky/ore chunks only, low density) lives in `src/world/chunk/` and is gated by `METEORITE_MIN_CENTER_DISTANCE_FRACTION` / `METEORITE_ORE_CHANNEL_FLOOR` in `game_balance.rs`.

## Gather payout: yield comes from the tool, optionally capped by the node

A node's `storage` is its finite reservoir; the per-swing quantity scales with the *tool*, clamped by the node's optional `per_swing_yield` cap (rare small-storage nodes only; `None` everywhere else).

`src/resource_nodes.rs - next_payout_from_storage` is the one rule, shared by the server's `next_resource_payout` and the client-side gather prediction (both resolve the cap from the same `ResourceNodeDefinition`):

```
quantity = tool.gather_amount.max(1)            // 6 stone, 12 iron, 1 hands
quantity = quantity.min(per_swing_yield)        // only if the node defines a cap
// walk storage, take the first non-empty stack, grant min(stack.quantity, quantity)
```

The only capped node today is meteorite: `METEORITE_PER_SWING_YIELD = 2` (`game_balance.rs`) over its 8-item storage makes the rare find a deliberate 4-swing beat instead of one iron-pickaxe hit vaporising the node (tests `meteorite_takes_several_swings_to_exhaust`, `client_storage_payout_matches_server_node_payout`).

Server and client run the identical function so optimistic client gain matches the authoritative payout (test `client_storage_payout_matches_server_node_payout` in `src/resource_nodes.rs` checks this for all storage+tool combos, uncapped and capped). `storage` is a `&[ResourceMaterial]` list even though every current node has exactly one entry; the payout walks the first non-empty stack, do not assume one-item-per-node is enforced.

Reach is one knob per category: `PICKUP_RANGE = 3.4` (`src/items.rs`) for dropped items, `RESOURCE_GATHER_RANGE = 2.75` (`src/resource_nodes.rs`) for nodes. The server validates with lenient distance-only checks (`within_pickup_reach` / `within_gather_reach`) plus `PICKUP_SERVER_REACH_SLACK_M = 1.5` (`src/game_balance.rs`) so it doesn't false-reject a target the client already locked.

## Sickle: reaping Tall Grass tufts

The iron sickle (`IRON_SICKLE_ID`, bench tier 1 beside the iron hatchet/pickaxe: 1 hewn log + 8 iron bars + 2 twine; `ToolKind::Sickle`, its own `ItemModel::Sickle` reaping-slash swing archetype) is the fiber tool, and it works through the **ordinary resource-node gather path**, no bespoke wire message. It is iron-only by design (a knapped stone crescent made no sense, and everything it accelerates, the cloth/twine sinks like armor/bow/bags, is bench-tier anyway). The Tall Grass node (`HAY_GRASS_NODE_ID`) is the one node whose `required_tool` is `Sickle`; its 40-fiber storage against the sickle's `gather_amount: 40` means one sweep reaps the whole tuft and despawns it (fresh-position respawn via the chunk manager like any depleted node). The 40 is sized against the cloth/twine sinks (a padded chest piece is 33 fiber, a full starter kit a couple hundred): one tuft per armor piece, not a mowing session. Its contact cue is `SoundId::InventoryPickup`, the same grass-rustle the hand E-pluck plays (`impact_sound_for` special-cases `ItemModel::Sickle`), so collecting fiber sounds the same however you collect it.

The pre-forge fiber path is the crude quick-pickup: the tuft is still `is_crude()`, so a bare-handed E-pluck works long before the sickle exists, but it is capped by the definition's `hand_pickup_yield` (3 fiber) and RUINS the tuft (the node despawns, the rest of the storage is discarded). Every early twine recipe (stone tools, hammer, spear) is fed by plucks; the sickle is the bench-tier multiplier for the fiber-heavy mid game. Hand pluck 3 vs sickle sweep 40 is the whole crafting incentive. Fiber abundance is tuned by tuft density per biome (`base_capacity` in `src/world/chunk/classification.rs`: Plains 28, Forest 12, Mixed 8, Rocky 2, Ore 1), not by a per-swing density formula.

Note the gating asymmetry this introduced: the E quick-pickup path is keyed on `ResourceNodeModel::is_crude()` (client `resource_target_is_crude`, server `pick_up_resource_node`), NOT on `required_tool.kind == Hands`, because the hay tuft requires a sickle for swings yet stays E-pluckable. Branch piles and surface stones keep `Hands` (pickup-only, no tool swings at all).

`SICKLE_PVP_DAMAGE` lives in `src/game_balance.rs`. Tests in `src/server/tests/resource_nodes.rs` (`sickle_reaps_a_whole_tall_grass_tuft_in_one_swing`, `other_tools_cannot_swing_tall_grass`, `hand_pluck_takes_a_handful_and_ruins_the_tuft`).

## ResourceNodeDefinition shape

`src/resource_nodes.rs - ResourceNodeDefinition` has exactly these fields. There is **no** `capacity`, **no** `yields_item_id`, and **no** regrow field.

| field | type | role |
| --- | --- | --- |
| `id` | `&'static str` | stable string id. |
| `name` | `&'static str` | display name. |
| `model` | `ResourceNodeModel` | which mesh family (ore/vein/tree-by-size/crude). Drives the render path and collider. |
| `required_tool` | `ToolRequirement` | `{ kind, min_tier }`, not a bare `ToolKind`. |
| `storage` | `&'static [ResourceMaterial]` | the finite reservoir; each `ResourceMaterial` is `{ item_id, quantity }`. A node spawns full from this. |
| `per_swing_yield` | `Option<u16>` | per-swing extraction cap, clamping the tool's `gather_amount`. `None` (tool-limited) everywhere except rare small-storage nodes (meteorite: `Some(METEORITE_PER_SWING_YIELD)`). |
| `anchor_height` | `f32` | vertical offset for the targeting anchor. |
| `ray_radius` | `f32` | focus-cylinder radius for look-at targeting. |

Authoritative node state lives in `GameServer::resource_nodes` as `HashMap<ResourceNodeId, ResourceNodeState>`; the ECS mirror in `src/server/resource_node_ecs.rs` carries the replicated component split. See [docs/replication.md](replication.md).

### Crude E-pickup nodes

`SurfaceStone`, `BranchPile`, `HayGrass` carry `ToolRequirement::new(ToolKind::Hands, 0)`. `ToolRequirement::allows` returns `false` for any `Hands` requirement, so **no swing gathers them, even with a tool equipped**: they are E-pickup-only. A `Hands` requirement is the crude-pickup marker, not "gatherable by punching". `ResourceNodeModel::is_crude` drives both the gather-path tool skip and a smaller render scale, and crude nodes have no collider (walk-through). Each holds exactly one item (`stone`/`wood`/`fiber`), the no-tool bootstrap so a fresh player can craft their first crude tools.

### Regrow timing lives in chunk_manager, not on the node

A mined-out node respawns 5 to 15 minutes later (jittered) at a noise-valid chunk position. The window is `MIN_REGROW_TICKS = 5 * 60 * SERVER_TICK_RATE_HZ` and `MAX_REGROW_TICKS = 15 * 60 * SERVER_TICK_RATE_HZ` in `src/server/chunk_manager.rs`; the jitter is applied in `src/server/chunk_manager/regrow.rs`. It is keyed off `SERVER_TICK_RATE_HZ`, not a hardcoded 20. See [docs/chunks-and-aoi.md](chunks-and-aoi.md).

### Tree dead-snag state is server-authoritative

`spawn_resource_node` decides `dead` from `world_seed + position` via `tree_is_dead` (forest-growth noise channel, smoothstep alive-chance, deterministic per-node hash). It is frozen on the `ResourceNodeState` (replicated and saved), not re-derived per client. A `world_seed` of `None` (the client menu backdrop, which neither replicates nor saves) leaves trees alive.

## The single tool-vs-material table

`src/items/materials.rs - tool_effectiveness_pct(ToolKind, DestructibleMaterial) -> u32` is the one place that answers "how well does tool X bite material Y", as an integer percentage multiplier. Every destructible-entity damage path reads through it instead of branching on entity type, so balancing a matchup is a one-line edit and a new material is one new arm. Its sibling `explosive_effectiveness_pct(ExplosiveKind, DestructibleMaterial) -> u32` lives right beside it and does the same job for the blast raid path (the percentages are in `game_balance.rs` under `*_EFFECTIVENESS_*_PCT`); see [docs/pvp-combat.md](pvp-combat.md) for the explosive raid math.

| tool \ material | Wood | Stone | Sticks | WoodBuilding | StoneBuilding | MetalBuilding | Cloth |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Axe | 150 | 50 | 300 | 15 | 0 | 0 | 300 |
| Pickaxe | 50 | 150 | 200 | 5 | 0 | 0 | 300 |
| Hammer | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Sickle | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Hands | 50 | 50 | 50 | 50 | 50 | 50 | 50 |

Matched proper tool is roughly 1.5x, mismatched proper tool roughly 0.5x. `StoneBuilding` and `MetalBuilding` return `0` for every tool, so those entities are tool-proof by construction; the explosives (through `explosive_effectiveness_pct`) are the intended breach path for them. The hammer returns `0` for everything, which closes the "hammer as a free raid tool" hole; the sickle mirrors it (a fiber tool must not double as a raid tool). `Hands` is a worst-case catch-all (bare hands are normally rejected upstream before reaching here).

**Make a deployable tool-immune by giving it a 0-arm material, never by special-casing the damage handler.** The iron door is the worked example. The same table shape carries the `Cloth` column the placed charges use, so a charge is fizzle-able by a defender's swing.

## DoorVariant: the immutable-identity pattern

`src/items/deployables.rs - DoorVariant` (`HewnLog`, `Iron`) is the canonical example of immutable spawn identity. The variant travels on the replicated `Deployable` component and in the save, and is the single lookup for the door's item id, HP, raid material, and name:

| variant | item id | `max_hp()` | `material()` |
| --- | --- | --- | --- |
| `HewnLog` | `hewn_log_door` | `DOOR_MAX_HP = 1500` | `WoodBuilding` (raidable with tools, slowly) |
| `Iron` | `iron_door` | `IRON_DOOR_MAX_HP = 3000` | `MetalBuilding` (tool-immune, exactly double the HP) |

All accessors are `const fn`. Adding a door is one arm here plus a recipe and a model; nothing in the damage/placement/persistence paths changes.

`DeployableKind` (the broader enum, `src/items/deployables.rs`) now has ten variants: `Workbench { tier }`, `Furnace { tier }`, `Building`, `Door`, `SleepingBag`, `StorageBox`, `Torch`, `ToolCupboard`, plus the `RuinCache` (v19) and `Explosive { kind }` (v20). Variants are postcard-encoded by index and appended only, never reordered. Its `material()` is the source of truth for each kind's `DestructibleMaterial` (a placed charge is `Cloth`, so a defender can shoot it out; a ruin cache is indestructible via its own damage guard); `raidable()` flags which kinds anyone may damage. The `Workbench` and `Furnace` `tier` fields are what the generic upgrade table (`src/items/upgrades.rs - DEPLOYABLE_UPGRADES`) mutates in place. Placement, damage, ownership, charges, and the tier upgrade all live in [docs/crafting-and-deployables.md](crafting-and-deployables.md) and [docs/base-building-and-claims.md](base-building-and-claims.md).

## Full roster

**Raw materials / refined (`Bag` mesh, not equipable):** `wood`, `stone`, `coal`, `iron_ore`, `sulfur_ore`, `fiber`, `plant_twine`, `iron_bar`, `hewn_log`, plus the materials `sulfur` (smelted from sulfur ore), `cloth` (4 fiber to 1, hand-crafted armor padding / charge wrap), `gunpowder` (2 coal + 2 sulfur to 1, bench t1; the master raid-cost lever), `meteorite_alloy` (mined from the rare meteorite node), `meteorite_ingot` (furnace-smelted from the alloy, the top-tier metal), and `salvaged_fittings` (looted from the salvage chests in burnt-out houses, never craftable).

**Tools (four, all equipable, all stack to 1):** `wood_stone_hatchet` (Axe t1), `wood_stone_pickaxe` (Pickaxe t1), `iron_hatchet` (Axe t2), `iron_pickaxe` (Pickaxe t2). Every tool is an authored Blender glb matched to its icon; see [docs/playbooks/art-pipeline.md](playbooks/art-pipeline.md).

**Melee weapons (`weapon` profile, equipable, stack to 1):** `wooden_club` (12 dmg, fast, hand-crafted), `stone_spear` (16, reach 4.5 m, hand), `iron_sword` (20, bench t1), `iron_mace` (26, biggest knockback, 50% armor pierce, bench t2). Combat reads `weapon` before `tool`; weapons gather nothing and do hands-tier damage to structures (they are not raid tools).

**Ranged weapons and ammo (`ranged` profile):** `wooden_bow` (hold-to-draw, 15 to 40 damage, bench t1: 12 wood + 6 plant twine + 2 cloth), `crossbow` (55, slow reload, bench t2: 6 hewn logs + 8 iron bars + 4 plant twine + 4 `salvaged_fittings`, making it a ruin-loot sink), `arrow` (stacks to 24, ~50% recoverable, hand-crafted four at a time).

**Armor (`armor` profile, 12 pieces = 3 sets x 4 slots):** the Padded, Lamellar, and Iron sets, each with a head / chest / legs / feet piece keyed to an `EquipmentSlot`. Worn in the four equipment slots, not the actionbar; visible on the third-person rig. Mitigation and slots are detailed in [docs/pvp-combat.md](pvp-combat.md).

**Explosives (`explosive` profile):** `powder_bomb` (thrown, bench t1), `powder_keg` (placed, bench t1), `satchel_charge` (placed, bench t2). The two placed charges become a `DeployableKind::Explosive { kind }` when set; the bomb is lit on the throw and rides the projectile sim end to end (charged throw, bounce/roll, detonates in place). The ember charge was retired. See [docs/crafting-and-deployables.md](crafting-and-deployables.md).

**Build/utility holdables:** `hammer` (Hammer tool, `Hatchet` animation, `Hammer` mesh), `building_plan` (`BuildingPlan` mesh, no tool/deployable).

**Deployables (equipable, render as the bag in hand):** `workbench_t1` (upgrades in place to tier 2), `crude_furnace`, `hewn_log_door`, `iron_door`, `sleeping_bag`, `torch` (stacks to 10), `storage_box_small`, `storage_box_large`, `tool_cupboard`. The `ruin_cache` is a world-spawned, non-placeable, indestructible loot container (`equipable: false`, no recipe).

**Six hidden building-block defs** built via `building_piece_item`: foundation, wall, window wall, doorway, ceiling, stairs. Never craftable and never in an inventory; they exist so `DeployedEntity::item_id` resolves through the registry (saves, mirror views, colliders). Their `max_health` and `collider_half_width` are **placeholders**: the authoritative HP and colliders for placed building blocks come from `src/building.rs`, not the registry entry. Do not trust those numbers.

**Resource nodes (14 in `RESOURCE_NODE_DEFINITIONS`):**

- Ores (Pickaxe t1): `coal_node`, `iron_node` (72 each), `sulfur_node` (48, deliberately the leanest ore: sulfur only becomes gunpowder, so raid prep is paced by nodes toured), `stone_node` (Stone Vein, 120 stone, deliberately the most generous: stone is the defensive material).
- Meteorite (Pickaxe t2, the one iron-gated node): `meteorite_node`, yielding `meteorite_alloy` (furnace-smelts to `meteorite_ingot`). Rare, a scorched slag boulder studded with pale alloy nuggets, far-from-center rocky/ore chunks only (worldgen gating in `src/world/chunk/`); also runtime-spawned in rich clusters by a meteor shower impact.
- Trees (Axe t1, six size variants): `pine_tree_small` / `pine_tree` / `pine_tree_large`, `birch_tree_small` / `birch_tree` / `birch_tree_large` (wood, scaling 18 to 84 by size). The un-suffixed ids (`pine_tree`, `birch_tree`) are the **medium** variants on purpose, so pre-size-variant saves load as medium without migration.
- Crude (Hands, E-pickup): `surface_stone` (1 stone), `branch_pile` (1 wood), `hay_grass` (1 fiber).

## Ore depletion stages (cosmetic, client-side)

Ore and stone-vein nodes step through `ORE_NODE_STAGE_COUNT = 3` meshes while being mined (untouched, worn, gutted), defined in `src/app/scene/mesh/ore.rs`. The stage meshes are authored glbs (`assets/ore/<type>/stage_<n>.glb`, built by `art/ore/build_ore.py`) loaded into `ResourceVisualAssets` in `src/app/scene/assets.rs`.

The ore boulders are cel-shaded via the shared `ToonMaterial` (`src/app/scene/toon.rs`), the same material the deployable props and trees use; ores are no longer the only cel family. The spawn path (`src/app/systems/items/resource_nodes/spawn.rs`) imports `ToonMaterial`, not the old `OreToonMaterial`. See [docs/toon-shading.md](toon-shading.md).

Staging is purely cosmetic and entirely client-side. `apply_resource_node_stage_system` (`src/app/systems/items/resource_nodes/stages.rs`) watches `Changed<ResourceNodeStorage>`, maps remaining fraction to a stage (`ORE_STAGE_WORN_BELOW = 0.70`, `ORE_STAGE_GUTTED_BELOW = 0.35`), and on a real crossing swaps the mirror's `Mesh3d`, fires a half-magnitude ore-shatter burst, and plays `OreStageCrumble`. Full depletion plays the full shatter plus `OreNodeBreak`. Gather rules, colliders, and targeting are untouched by stage. Part-mined nodes from replication or a save spawn directly at the correct stage mesh.

Admin helper: `/drain [remaining-fraction]` (function `command_drain` in `src/server/commands/world.rs`) sets the looked-at node's storage absolutely (default 0.5; accepts `0..=1` or a percentage like `/drain 40`; `0` removes the node through the regular depletion path), exercising the full replication chain without forty pickaxe swings.

## Client reconciliation is event-driven

The client mirrors replicated nodes into local `NetworkResourceNode` visuals via `apply_resource_nodes_system` (`src/app/systems/items/resource_nodes.rs`), reacting to `Added<ResourceNode>` and `RemovedComponents<ResourceNode>`, never iterating the full replicated set per frame (that costs 1 to 4 ms at AoI scale). The `ResourceNodeEntities` resource holds a forward `id -> Entity` map, a reverse `Entity -> id` map (so `RemovedComponents` can find the local mirror), and a `pending_spawns: VecDeque` that drains a per-frame spawn budget across frames. A one-time catch-up scan on the first run after connect handles entities that arrived during early-return `client_id == None` frames. `Ref::is_changed()` lies for Lightyear-touched components, so do not gate work behind it. This is the canonical pattern for any new `apply_*` system; the full rationale is in [docs/replication.md](replication.md) and CLAUDE.md.

## Smelting is not in the recipe registry

`src/crafting/types.rs - RecipeStation` has only two variants: `None` (hand-craftable) and `Workbench { min_tier }`. A workbench tier 2 satisfies a tier-1 requirement, mirroring tool tiers. **There is no `Furnace` variant.** Smelting happens inside the furnace's own UI, not the recipe registry: `smelt_result` in `src/server/furnace/state.rs` maps `iron_ore -> iron_bar`, `sulfur_ore -> sulfur`, and `meteorite_alloy -> meteorite_ingot`, and extending it is a one-line change. The recipe queue, furnace state machine, loot bags, deployable damage, the workbench tier upgrade, and the placed charges all live in [docs/crafting-and-deployables.md](crafting-and-deployables.md).

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
