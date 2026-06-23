---
title: Game design, direction, and core loop
owns: The "what is this game and where is it going" source of truth: design pillar, the implemented core loop as a gated sequence, the raid-balance lever, and the honest content-ceiling inventory.
when_to_read: Before designing a new gameplay feature, tuning the loop, or to understand intent/scope and what is deliberately absent.
sources:
  - README.md - design pillar and content-ceiling honesty
  - src/items.rs - tool_effectiveness_pct (raid-balance table), DestructibleMaterial::raidable, tool tiers
  - src/crafting.rs - REGISTERED_RECIPES, RecipeStation gating
  - src/resources.rs - crude starter nodes, ToolRequirement::allows tier logic
  - src/game_balance.rs - every gameplay tuning constant
related:
  - CLAUDE.md - the singleplayer==multiplayer and gameplay-never-pauses invariants this doc builds on
  - docs/items-and-resources.md - the registries this loop is built on, and how to add a tool/ore/node
  - docs/crafting-and-deployables.md - the crafting queue, furnace, and deployables that gate the loop
  - docs/base-building-and-claims.md - building tiers, stability, doors, and the Tool Cupboard claim
  - docs/pvp-combat.md - the combat, death, respawn, and loot-bag mechanics at the end of the loop
  - docs/art-direction.md - the look-and-feel trajectory (PBR to cel/anime)
---

# Game design, direction, and core loop

> When to read this: before designing a new gameplay feature, tuning the loop, or to learn what is deliberately absent. Source of truth: `README.md` (pillar), `src/items.rs` + `src/crafting.rs` + `src/resources.rs` + `src/game_balance.rs` (the loop and its numbers). Canonical invariants live in CLAUDE.md.

This is direction, not mechanics. For how a subsystem works, follow the cross-links. Every number here is re-derived from the registries; do not trust prose over `src/game_balance.rs`.

## Design pillar

Ashwend does the familiar survival loop well; it does not reinvent the genre. From `README.md`: "Ashwend isn't trying to reinvent the genre. It shares its core mechanics with the survival games already out there... The goal is to take the familiar loop and do it well." When designing a feature, the test is "does this sharpen the known loop," not "is this novel." Novelty for its own sake is off-pillar.

The second pillar is an engineering invariant, not a player-facing mode: solo and together run on the exact same core. Singleplayer is a loopback host of the exact `GameServer` the dedicated server runs; both speak `ClientMessage` / `ServerMessage` and consume per-component replication, enforced as the singleplayer==multiplayer invariant in CLAUDE.md. Players reach the game through Multiplayer; the singleplayer menu entry is a dev/test convenience, gated out of release builds (`#[cfg(debug_assertions)]` on the main-menu button), not a shipped way to play. The pillar earns its keep precisely because that dev/test path runs the identical code, so a feature can never work in one mode and silently break the other. Do not build a feature that only works in one mode.

## The core loop (implemented, as ordered gates)

Each step is gated by the one before it. The gate is the design, not an accident of code. All gather/craft numbers below are read live from `src/resources.rs`, `src/crafting.rs`, and `src/items.rs`.

1. **Crude hand-gather (no tool).** Spawn with empty hands. Three crude nodes carry `ToolRequirement{kind: Hands}` (`src/resources.rs` - `SURFACE_STONE_NODE_ID`, `BRANCH_PILE_NODE_ID`, `HAY_GRASS_NODE_ID`): Loose Stone (1 stone), Branch Pile (1 wood), Tall Grass (1 fiber). A `Hands` requirement means E-pickup only; `ToolRequirement::allows` (`src/resources.rs`) rejects swinging any tool, including bare hands, at them. They exist solely to bootstrap the first tools. Do not make them tool-gatherable.

2. **No-station stone tools.** Stone Hatchet and Stone Pickaxe craft at `RecipeStation::None` (`src/crafting.rs` - `STONE_HATCHET_RECIPE_ID`, `STONE_PICKAXE_RECIPE_ID`), no workbench required. Tier 1, `gather_amount: 6`, durability 200 (`STONE_TOOL_DURABILITY`).

3. **Mine and chop.** Real nodes (trees, ore boulders) require a tier-1 Axe or Pickaxe. A matched tool yields its `gather_amount`; mismatched proper tools bite at reduced effectiveness via `tool_effectiveness_pct`.

4. **Workbench gate.** The Workbench crafts at `RecipeStation::None` (you can build the first one by hand). Once placed, it unlocks everything tagged `RecipeStation::Workbench{min_tier: 1}`: hewn log, furnace, iron tools, both doors, large storage box, and the Tool Cupboard. This is the single most important progression gate.

5. **Refine and smelt (the iron gate).** Hewn Log squares raw wood into a structural billet (10 wood to 1 hewn log, bench-gated; `src/crafting.rs` - `HEWN_LOG_RECIPE_ID`). The Furnace smelts iron ore to iron bars; the only smelt recipe today is iron ore to iron bar (`src/server/furnace/state.rs` - `smelt_result`). Hewn logs plus iron bars feed the iron tools.

6. **Iron tools.** Iron Hatchet and Iron Pickaxe, tier 2, bench-gated. The upgrade is felt as bigger payouts and longer life, NOT faster swings: `gather_amount: 12` (double stone) and durability 600 (`IRON_TOOL_DURABILITY`). Swing cadence is gated by the animation (`AXE_SWING_SECONDS`, `PICKAXE_SWING_SECONDS` in `src/app/state/gather.rs`), not by `cooldown_ticks`. Preserve this when adding tools: bump yield and durability, not swing speed.

7. **Build a base.** Building pieces come in three tiers, `Sticks` to `HewnWood` to `Stone` (`src/building.rs` - `BuildingTier`), across six pieces: Foundation, Wall, WindowWall, Doorway, Ceiling, Stairs (`src/building.rs` - `BuildingPiece`). Placement always costs raw wood at the Sticks tier; upgrading to HewnWood/Stone pays the tier cost with a Hammer. See [base building](base-building-and-claims.md).

8. **Lock it: code doors + Tool Cupboard claim.** Hewn Log Door and Iron Door (code-locked). The Tool Cupboard is the anti-grief claim object: place it inside your base and it projects a build-block margin of 5 grid cells (~15 m, `BUILDING_PRIVILEGE_MARGIN_CELLS`) so outsiders cannot wall you in or build adjacent. Destroying the cupboard lifts building privilege, so it is itself a raid objective with WoodBuilding-band HP (`TOOL_CUPBOARD_MAX_HP = 1000`).

9. **Melee PvP.** Combat is melee-only and server-authoritative. Per-tool damage tracks tier: stone hatchet 8, iron hatchet 12, stone pickaxe 15, iron pickaxe 22 (`src/game_balance.rs` - `*_PVP_DAMAGE`). See [PvP combat](pvp-combat.md).

10. **One loot bag, instant respawn.** On death the player drops ONE loot bag holding the entire carry (`LOOT_BAG_SLOT_COUNT` = inventory 60 + actionbar 9 = 69 slots; `src/protocol/mod.rs`), not N scattered items. Respawn is instant at full HP, placed at least 12 m from any live player (`RESPAWN_MIN_DISTANCE_M`) to stop spawn-camping. There is no respawn cooldown.

Tier progression is implicit, not branchy: a higher-tier tool auto-satisfies every lower-tier node requirement via `tool.tier >= min_tier` (`src/resources.rs` - `ToolRequirement::allows`), and a tier-2 workbench satisfies a tier-1 station via `tier >= min_tier` (`src/crafting.rs` - `RecipeStation::satisfied_by`). Do not special-case tiers; bump the number.

The full tech tree today is 17 recipes (`src/crafting.rs` - `REGISTERED_RECIPES`): plant twine, hewn log, stone+iron hatchet, stone+iron pickaxe, workbench, furnace, building plan, hammer, hewn-log door, iron door, sleeping bag, torch, small storage box, large storage box, tool cupboard. That is the whole tree; a determined evening or two reaches its end.

## Raid balance is the central lever

The single most load-bearing design decision lives in one function: `tool_effectiveness_pct(tool, material)` in `src/items.rs`. It is a percentage multiplier table (integer math) read by every destructible-damage path. The building arms encode the entire raid economy:

| Building material | Axe | Pickaxe | Felt result |
|---|---:|---:|---|
| `Sticks` | 300% | 200% | Shreds. A stone hatchet does ~90/hit; a Sticks wall (250 HP, `BUILDING_STICKS_WALL_HP`) falls in three hits. |
| `WoodBuilding` (hewn-wood, doors) | 15% | 5% | Slow but real raid. An iron hatchet at 15% does ~9/hit; a hewn-wood wall (3600 HP, `BUILDING_HEWN_WOOD_WALL_HP`) costs ~400 swings, roughly 5 minutes of continuous swinging and most of a tool's 600 durability. Loud, expensive, possible. |
| `StoneBuilding` | 0% | 0% | Tool-immune by construction. Stone wall HP (6000) matters only for future siege gear. |
| `MetalBuilding` (iron doors) | 0% | 0% | Tool-immune, kept distinct from stone so explosives can later balance metal independently. |

The Hammer does 0% to everything (`ToolKind::Hammer, _ => 0`): it builds, it never raids. This closes the "hammer as a free raid tool" hole outright.

Two contracts here are easy to break and must be preserved:

- **Stone/metal returning 0 is deliberate, not a stub.** Raiding stone and metal bases is impossible by construction; it waits for an explosives/siege damage path that does not exist yet. Do not casually add a nonzero arm.
- **`DestructibleMaterial::raidable()` (`src/items.rs`) sets the grief model.** Building pieces, doors, sleeping bags, and the Tool Cupboard are damageable by non-owners (raiding cannot exist otherwise). Workbench, furnace, and storage boxes keep an owner-only damage gate so griefers cannot idly chew through someone's crafting corner. Changing this changes the whole raid/grief model.

Tier order (`BuildingTier`, `BuildingPiece`) and recipe/item ids are postcard-encoded into saves and the wire. Append-only forever; never reorder. See [base building](base-building-and-claims.md) and [items and resources](items-and-resources.md).

## Felt-archetype design language

The same two-tool archetype recurs across gather, combat, and raid, so a player's intuition transfers between activities:

- **Hatchet = DPS / light.** Fast animation (`AXE_SWING_SECONDS = 0.78`), low knockback (`HATCHET_KNOCKBACK_SPEED = 1.8` m/s), best against wood. The sustained-damage option.
- **Pickaxe = burst / heavy.** Slow animation (`PICKAXE_SWING_SECONDS = 1.60`), high knockback (`PICKAXE_KNOCKBACK_SPEED = 4.0` m/s), best against stone, higher per-hit damage. The committed-shove option.

Weapon feel is intentionally heavy and committed, not fast and light. Swings have a real wind-up and miss-recovery punishes spam (combat tuning in `src/game_balance.rs` under the `COMBAT_` prefix; swing timing in `src/app/state/gather.rs`). Audio is synced to contact. When adding a weapon, fit it to one of these archetypes rather than inventing a third feel.

## Day/night and world

The day/night cycle is authoritative and shared (`src/world_time.rs` - `WorldTime`). A full 24 h in-game day spans 30 real minutes at the default multiplier (`REAL_SECONDS_PER_DAY = 30 * 60`). Torches are placeable 8-hour light sources (`TORCH_BURN_TICKS`, 8 game-hours). There is a hold-M world map with biome legend and per-player markers. None of this gates the loop; it is ambience and orientation.

## Content ceiling (what is deliberately absent today)

The README is honest that you hit the ceiling fast. Stated plainly, verified absent in the current code:

- **No survival meters.** No hunger, thirst, temperature, or stamina. The loop is gather/craft/build/PvP only.
- **No PvE.** No animals, NPCs, AI, or mobs. The only threat is other players.
- **No explosives or siege.** Stone and metal bases cannot be raided by anyone but the wear of time. Explosives exist only as a reserved future damage path in code comments.
- **No armor items.** `PlayerArmor` is wired and replicated and `damage_after_armor` applies it, but no item sets it; every player ships at 0. Adding armor is a server-side mutation with no protocol change. See [PvP combat](pvp-combat.md).
- **Instant respawn, friendly-fire-always.** No respawn cooldown, no teams/factions, no per-zone PvP toggle, no safe zones. Friendly fire is unconditional.
- **No combat log, scoreboard, healing/bandages, or combat music.** Health regenerates on respawn only; killer name appears only on the death splash.

These are direction, not bugs. Mark any feature that adds to this list as a deliberate scope expansion.

## Art direction (status: mid-transition)

The look is moving from PBR toward a cel-shaded / anime style. Converted to the toon/posterized look today: ore nodes, deployables, grass, biome ground, and trees. Still PBR: building pieces and doors. This is a stated look-and-feel direction with an explicit converted/pending split. Before making a new prop cel-shaded or planning a wider shift, read [art direction](art-direction.md) for the trajectory and [toon shading](toon-shading.md) for the shader mechanics.

## Source-of-truth pointers

- **Every gameplay tuning constant lives in `src/game_balance.rs`**, never inline in a subsystem. The file header makes this a hard rule; subsystem files only re-export. Any doc or instinct that points a balance edit elsewhere is wrong (this is a CLAUDE.md invariant).
- The loop's shape lives in the registries: `src/items.rs` (items, tools, `tool_effectiveness_pct`, `DestructibleMaterial`), `src/crafting.rs` (recipes, stations), `src/resources.rs` (nodes, gather rules), `src/building.rs` (tiers, pieces).
- **Do not trust pre-recalibration combat tuning notes.** Earlier docs carried a swing-timing column (0.50 s hatchet) and a "constants live in `combat.rs`" sourcing that both drifted; the real swing is 0.78 s and the constants moved to `game_balance.rs` under `COMBAT_`. Re-derive from `src/game_balance.rs` plus `src/app/state/gather.rs` before quoting combat numbers. The current truth is in [PvP combat](pvp-combat.md).

Protocol/runtime anchors (verify before quoting): `PROTOCOL_VERSION = 37`, `SERVER_TICK_RATE_HZ = 20.0`, `MAX_HEALTH = 100.0` (`src/protocol/mod.rs`). Every time-based balance constant derives its tick count from `SERVER_TICK_RATE_HZ`.

## Related docs

- [CLAUDE.md](../CLAUDE.md) - the singleplayer==multiplayer and gameplay-never-pauses invariants treated here as design choices.
- [docs/items-and-resources.md](items-and-resources.md) - the item/tool/resource registries the loop is built on, plus how to add one.
- [docs/crafting-and-deployables.md](crafting-and-deployables.md) - crafting queue, furnace state machine, and the deployable system that gates the loop.
- [docs/base-building-and-claims.md](base-building-and-claims.md) - building tiers, costs/HP, stability, doors, and the Tool Cupboard claim.
- [docs/pvp-combat.md](pvp-combat.md) - combat validation, weapon feel, death/respawn, and loot bags.
- [docs/art-direction.md](art-direction.md) - the PBR-to-cel look trajectory and converted/pending status.
- [docs/toon-shading.md](toon-shading.md) - cel shader mechanics for extending the toon family.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - world generation, biomes, and persistence.
