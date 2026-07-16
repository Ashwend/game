---
title: Game design, direction, and core loop
owns: The "what is this game and where is it going" source of truth: design pillar, the implemented core loop as a gated sequence, the raid-balance lever, and the honest content-ceiling inventory.
when_to_read: Before designing a new gameplay feature, tuning the loop, or to understand intent/scope and what is deliberately absent.
sources:
  - README.md - design pillar and content-ceiling honesty
  - src/items/materials.rs - tool_effectiveness_pct + explosive_effectiveness_pct (raid-balance tables), DestructibleMaterial::raidable
  - src/items/ - the item registry split (weapons, armor, ranged, explosives, deployables, upgrades)
  - src/crafting/registry.rs - REGISTERED_RECIPES, RecipeStation gating
  - src/resource_nodes.rs - crude starter nodes, ToolRequirement::allows tier logic
  - src/game_balance.rs - every gameplay tuning constant (weapons, armor, explosives, meteor shower)
related:
  - CLAUDE.md - the singleplayer==multiplayer and gameplay-never-pauses invariants this doc builds on
  - docs/items-and-resources.md - the registries this loop is built on, and how to add a tool/ore/node
  - docs/crafting-and-deployables.md - the crafting queue, furnace, and deployables that gate the loop
  - docs/base-building-and-claims.md - building tiers, stability, doors, and the Tool Cupboard claim
  - docs/pvp-combat.md - the combat, death, respawn, and loot-bag mechanics at the end of the loop
  - docs/meteor-shower.md - the meteor shower event, the one world event punctuating the loop
  - docs/art-direction.md - the look-and-feel trajectory (PBR to cel/anime)
---

# Game design, direction, and core loop

> When to read this: before designing a new gameplay feature, tuning the loop, or to learn what is deliberately absent. Source of truth: `README.md` (pillar), `src/items.rs` + `src/crafting.rs` + `src/resource_nodes.rs` + `src/game_balance.rs` (the loop and its numbers). Canonical invariants live in CLAUDE.md.

This is direction, not mechanics. For how a subsystem works, follow the cross-links. Every number here is re-derived from the registries; do not trust prose over `src/game_balance.rs`.

## Design pillar

Ashwend does the familiar survival loop well; it does not reinvent the genre. From `README.md`: "Ashwend isn't trying to reinvent the genre. It shares its core mechanics with the survival games already out there... The goal is to take the familiar loop and do it well." When designing a feature, the test is "does this sharpen the known loop," not "is this novel." Novelty for its own sake is off-pillar.

The second pillar is an engineering invariant, not a player-facing mode: solo and together run on the exact same core. Singleplayer is a loopback host of the exact `GameServer` the dedicated server runs; both speak `ClientMessage` / `ServerMessage` and consume per-component replication, enforced as the singleplayer==multiplayer invariant in CLAUDE.md. Players reach the game through Multiplayer; the singleplayer menu entry is a dev/test convenience, gated out of release builds (`#[cfg(debug_assertions)]` on the main-menu button), not a shipped way to play. The pillar earns its keep precisely because that dev/test path runs the identical code, so a feature can never work in one mode and silently break the other. Do not build a feature that only works in one mode.

## The core loop (implemented, as ordered gates)

Each step is gated by the one before it. The gate is the design, not an accident of code. All gather/craft numbers below are read live from `src/resource_nodes.rs`, `src/crafting.rs`, and `src/items.rs`.

1. **Crude hand-gather (no tool).** Spawn with empty hands. Three crude models are E-pluckable (`ResourceNodeModel::is_crude()`, `src/resource_nodes.rs`): Loose Stone (1 stone), Branch Pile (1 wood), Tall Grass (a 3-fiber handful capped by `hand_pickup_yield`, ruining the 40-fiber tuft). Loose Stone and Branch Pile carry `ToolRequirement{kind: Hands}`, meaning E-pickup only (`ToolRequirement::allows` rejects swinging any tool at them); Tall Grass instead requires the Sickle for swings, whose one sweep reaps the tuft's full 40 fiber. Hand plucks are the ONLY fiber source until the bench-tier Iron Sickle is forged (step 6), covering every early twine recipe. Do not make Loose Stone or Branch Pile tool-gatherable.

2. **No-station stone tools.** Stone Hatchet and Stone Pickaxe craft at `RecipeStation::None` (`src/crafting.rs` - `STONE_HATCHET_RECIPE_ID`, `STONE_PICKAXE_RECIPE_ID`), no workbench required. Tier 1, `gather_amount: 6`, durability 200 (`STONE_TOOL_DURABILITY`).

3. **Mine and chop.** Real nodes (trees, ore boulders) require a tier-1 Axe or Pickaxe. A matched tool yields its `gather_amount`; mismatched proper tools bite at reduced effectiveness via `tool_effectiveness_pct`.

4. **Workbench gate.** The Workbench crafts at `RecipeStation::None` (you can build the first one by hand). Once placed, it unlocks everything tagged `RecipeStation::Workbench{min_tier: 1}`: hewn log, furnace, iron tools, both doors, large storage box, and the Tool Cupboard. This is the single most important progression gate.

5. **Refine and smelt (the iron gate).** Hewn Log squares raw wood into a structural billet (10 wood to 1 hewn log, bench-gated; `src/crafting.rs` - `HEWN_LOG_RECIPE_ID`). The Furnace smelts iron ore to iron bars; the only smelt recipe today is iron ore to iron bar (`src/server/furnace/state.rs` - `smelt_result`). Hewn logs plus iron bars feed the iron tools.

6. **Iron tools.** Iron Hatchet and Iron Pickaxe, tier 2, bench-gated. The upgrade is felt as bigger payouts and longer life, NOT faster swings: `gather_amount: 12` (double stone) and durability 600 (`IRON_TOOL_DURABILITY`). Swing cadence is gated by the animation (`AXE_SWING_SECONDS`, `PICKAXE_SWING_SECONDS` in `src/app/state/gather.rs`), not by `cooldown_ticks`. Preserve this when adding tools: bump yield and durability, not swing speed.

7. **Build a base.** Building pieces come in three tiers, `Sticks` to `HewnWood` to `Stone` (`src/building.rs` - `BuildingTier`), across six pieces: Foundation, Wall, WindowWall, Doorway, Ceiling, Stairs (`src/building.rs` - `BuildingPiece`). Placement always costs raw wood at the Sticks tier; upgrading to HewnWood/Stone pays the tier cost with a Hammer. See [base building](base-building-and-claims.md).

8. **Lock it: code doors + Tool Cupboard claim.** Hewn Log Door and Iron Door (code-locked). The Tool Cupboard is the anti-grief claim object: place it inside your base and it projects a build-block margin of 5 grid cells (~15 m, `BUILDING_PRIVILEGE_MARGIN_CELLS`) so outsiders cannot wall you in or build adjacent. Destroying the cupboard lifts building privilege, so it is itself a raid objective with WoodBuilding-band HP (`TOOL_CUPBOARD_MAX_HP = 1000`).

9. **Weapons and melee PvP.** Combat is server-authoritative. Tools stay viable desperation weapons (per-tool damage tracks tier: stone hatchet 8, iron hatchet 12, stone pickaxe 15, iron pickaxe 22; `src/game_balance.rs` - `*_PVP_DAMAGE`), but the dedicated weapons widen the spectrum via a `WeaponProfile`: wooden club (12, fast, hand-crafted), stone spear (16, reach 4.5 m vs the 3.5 m standard), iron sword (20, bench t1), iron mace (26, biggest knockback, 50% armor pierce, bench t2). See [PvP combat](pvp-combat.md).

10. **Armor and ranged.** Four equipment slots (head, chest, legs, feet) take three armor sets (Padded hand-crafted, Lamellar bench t1, Iron bench t2), summing per-kind mitigation (melee / projectile / blast) capped at 60% and worn visibly on the third-person rig. Ranged adds the wooden bow (hold-to-draw, 15 to 40 damage) and the crossbow (55, slow reload), both firing server-simulated arrows. Bow and crossbow craft at the bench (t1 / t2). See [PvP combat](pvp-combat.md).

11. **Exploration resources (the exploration acquisition loop).** Three new paths feed the top of the tree. Meteorite is a rare scorched slag node in far-from-center rocky/ore chunks gated behind the iron pickaxe; it yields `meteorite_alloy`, furnace-smelted into `meteorite_ingot`. Burnt-out houses (homes gutted by the meteor storm) scatter across the map and hold `ruin_cache` salvage chests that are the only source of `salvaged_fittings` (the crossbow / iron-armor / satchel sink). The meteor shower is a periodic world event: two size-varied fireballs streak in on staggered arcs, strike building-cleared sites spread across the map, and each scatters a rich meteorite cluster scaled to its size, a contested PvP flashpoint. The event is deliberately unannounced (no countdown, no map marker; the sky and the audio are the announcement), so discovering one is part of the loop. See [worlds and saves](worlds-and-saves.md), [items and resources](items-and-resources.md), and [meteor-shower.md](meteor-shower.md).

12. **Tier-2 workbench and explosives (siege).** A placed workbench upgrades in place to tier 2 (30 iron bar + 6 salvaged fittings + 4 meteorite ingots) via the generic deployable upgrade path, unlocking the top-tier gear and the blackpowder charges. Gunpowder (coal + sulfur) feeds three explosives: powder bomb (thrown with a charged wind-up, lit on the throw, bounces and rolls), powder keg, and satchel charge (placed). These are the designed counter to stone bases: charges deal `Blast` damage through an effectiveness matrix, and placed charges hiss on an 8 to 9 s fuse that a claim-authorized defender can hold-E defuse or shoot out. (The wall-sticking ember charge was retired; the satchel's 30% metal arm is the metal counter, an iron door costing 5 satchels.) See [crafting and deployables](crafting-and-deployables.md) and [PvP combat](pvp-combat.md).

13. **One loot bag, instant respawn.** On death the player drops ONE loot bag holding the entire carry (`LOOT_BAG_SLOT_COUNT` = inventory 60 + actionbar 9 = 69 slots; `src/protocol.rs`), not N scattered items. Respawn is instant at full HP, placed at least 12 m from any live player (`RESPAWN_MIN_DISTANCE_M`) to stop spawn-camping. There is no respawn cooldown.

Tier progression is implicit, not branchy: a higher-tier tool auto-satisfies every lower-tier node requirement via `tool.tier >= min_tier` (`src/resource_nodes.rs` - `ToolRequirement::allows`), and a tier-2 workbench satisfies a tier-1 station via `tier >= min_tier` (`src/crafting/types.rs` - `RecipeStation::satisfied_by`). Do not special-case tiers; bump the number.

The tech tree is substantial. The base tree is still the tools-and-base spine (plant twine, hewn log, stone+iron hatchet, stone+iron pickaxe, workbench, furnace, building plan, hammer, hewn-log door, iron door, sleeping bag, torch, small+large storage box, tool cupboard). On top of it sit the advanced rows: cloth and gunpowder intermediates, the melee weapons and armor sets, the bow/arrow/crossbow line, and the four explosives, plus the in-place tier-2 workbench upgrade (a table row, not a recipe). The authoritative list is always `src/crafting/registry.rs` - `REGISTERED_RECIPES`. A determined session or two still reaches the end, but the raiding tier now has real farm depth behind it.

## Raid balance is the central lever

The single most load-bearing design decision lives in two parallel tables in `src/items/materials.rs`: `tool_effectiveness_pct(tool, material)` for the melee/tool raid path and `explosive_effectiveness_pct(kind, material)` for the blast path. Both are percentage multiplier tables (integer math) read by every destructible-damage path. The tool table encodes the melee raid economy:

| Building material | Axe | Pickaxe | Felt result |
|---|---:|---:|---|
| `Sticks` | 300% | 200% | Shreds. A stone hatchet does ~90/hit; a Sticks wall (250 HP, `BUILDING_STICKS_WALL_HP`) falls in three hits. |
| `WoodBuilding` (hewn-wood, doors) | 15% | 5% | Slow but real raid. An iron hatchet at 15% does ~9/hit; a hewn-wood wall (3600 HP, `BUILDING_HEWN_WOOD_WALL_HP`) costs ~400 swings, roughly 5 minutes of continuous swinging and most of a tool's 600 durability. Loud, expensive, possible. |
| `StoneBuilding` | 0% | 0% | Tool-immune by construction. A stone wall (6000 HP) needs explosives. |
| `MetalBuilding` (iron doors) | 0% | 0% | Tool-immune, and the effectiveness matrix keeps metal balanced independently of stone: only the top charges scratch it. |

The Hammer does 0% to everything (`ToolKind::Hammer, _ => 0`): it builds, it never raids. This closes the "hammer as a free raid tool" hole outright. Explosives are now the answer to stone and metal: the effectiveness matrix runs a powder bomb at 40% of wood and 0% of metal up to a satchel at 85% of wood, 45% of stone, and 30% of metal. Point-blank counts: hewn wood wall 5 kegs, wood door 2 satchels, iron door 5 satchels, stone wall 7 satchels, so the door is always the designed breach point. The satchel is the only charge that touches metal; revisit its metal arm if a third door tier lands. The raid-cost tuning is pinned by `src/server/tests/explosives.rs`.

Two contracts here are easy to break and must be preserved:

- **Tools returning 0 on stone/metal is deliberate, and explosives are the intended counter, not tools.** Do not add a nonzero tool arm for stone or metal; raiding those materials is an explosives job, balanced through `explosive_effectiveness_pct`, not by making a pickaxe chip stone.
- **`DestructibleMaterial::raidable()` (`src/items/materials.rs`) sets the grief model.** Building pieces, doors, sleeping bags, and the Tool Cupboard are damageable by non-owners (raiding cannot exist otherwise). Workbench, furnace, and storage boxes keep an owner-only damage gate so griefers cannot idly chew through someone's crafting corner. Ruin caches are indestructible (a world object, not a raid target). Changing this changes the whole raid/grief model.

Tier order (`BuildingTier`, `BuildingPiece`) and recipe/item ids are postcard-encoded into saves and the wire. Append-only forever; never reorder. See [base building](base-building-and-claims.md) and [items and resources](items-and-resources.md).

## Felt-archetype design language

The same two-tool archetype recurs across gather, combat, and raid, so a player's intuition transfers between activities:

- **Hatchet = DPS / light.** Fast animation (`AXE_SWING_SECONDS = 0.78`), low knockback (`HATCHET_KNOCKBACK_SPEED = 1.8` m/s), best against wood. The sustained-damage option.
- **Pickaxe = burst / heavy.** Slow animation (`PICKAXE_SWING_SECONDS = 1.60`), high knockback (`PICKAXE_KNOCKBACK_SPEED = 4.0` m/s), best against stone, higher per-hit damage. The committed-shove option.

Weapon feel is intentionally heavy and committed, not fast and light. Swings have a real wind-up and miss-recovery punishes spam (combat tuning in `src/game_balance.rs` under the `COMBAT_` prefix; swing timing in `src/app/state/gather.rs`). Audio is synced to contact. The melee weapons widen this same spectrum rather than blurring it: the club is a short chop, the spear a long committed thrust that controls space, the sword a balanced arc, and the mace a huge wind-up with a huge payoff (the heaviest end of the axis). Ranged extends the language with the bow's draw-tension ramp and the crossbow's earned-by-a-slow-reload single shot. When adding a weapon, place it somewhere on this light-to-heavy axis rather than inventing a feel that reads like none of them.

## Day/night and world

The day/night cycle is authoritative and shared (`src/world_time.rs` - `WorldTime`). A full 24 h in-game day spans 30 real minutes at the default multiplier (`REAL_SECONDS_PER_DAY = 30 * 60`). Torches are placeable 8-hour light sources (`TORCH_BURN_TICKS`, 8 game-hours). There is a hold-M world map with biome legend and per-player markers only: no ruin glyphs and no meteor markers, by design, so ruins are discovered by exploring and a meteor shower is discovered by looking up (the fireballs and audio are its only announcement). The meteor shower is the one world event that punctuates the cycle; otherwise the clock is ambience and orientation and does not gate the loop.

## Content ceiling (what is deliberately absent today)

Much of the old ceiling has been retired: armor, ranged weapons, explosives/siege, and exploration content (rare nodes, ruin POIs, a timed world event) all ship now. What remains deliberately absent, verified against the current code:

- **No survival meters.** No hunger, thirst, temperature, or stamina. The loop is gather/craft/build/raid/PvP only. (The absence of stamina is also why iron armor carries no movement penalty for now; there is no meter to trade against.)
- **No PvE.** No animals, NPCs, AI, or mobs. The meteor shower is a scripted environmental hazard, not a creature; the only intelligent threat is other players.
- **No firearms.** The weapon ceiling is melee plus bow and crossbow by design. Blackpowder exists only as explosives (bombs, kegs, charges), never as a gun.
- **No tier-3 gear.** The Emberforged set and a tier-3 workbench are reserved and deliberately not designed ahead; meteorite's third use is held for them.
- **Instant respawn, friendly-fire-always.** No respawn cooldown, no teams/factions, no per-zone PvP toggle, no safe zones. Friendly fire is unconditional (and explosive self-damage is on: your own charges and the meteor can kill you).
- **No combat log, scoreboard, healing/bandages, or combat music.** Health regenerates on respawn only; killer name appears only on the death splash. Armor repair is craft-cost only (no repair bench).

Retired from this list (now live, cross-linked for detail): armor sets on the rig, the ranged line, the explosive/siege raid path against stone and metal, and the meteorite/ruins/meteor shower exploration content. These are direction, not bugs. Mark any feature that adds back to the absent list, or removes another item from it, as a deliberate scope expansion.

## Art direction (status: mid-transition)

The look is moving from PBR toward a cel-shaded / anime style. Converted to the toon/posterized look today: ore nodes, deployables, grass, biome ground, and trees. Held items and armor carry their own material families: held items follow the PBR-baked tools-rework path, and armor matches the player rig's material family (PBR today, flipping to cel when the rig does). Still PBR: building pieces and doors. This is a stated look-and-feel direction with an explicit converted/pending split. Before making a new prop cel-shaded or planning a wider shift, read [art direction](art-direction.md) for the trajectory and [toon shading](toon-shading.md) for the shader mechanics.

## Source-of-truth pointers

- **Every gameplay tuning constant lives in `src/game_balance.rs`**, never inline in a subsystem. The file header makes this a hard rule; subsystem files only re-export. Any doc or instinct that points a balance edit elsewhere is wrong (this is a CLAUDE.md invariant).
- The loop's shape lives in the registries: `src/items/` (items, tools, weapons, armor, ranged, explosives, `tool_effectiveness_pct`/`explosive_effectiveness_pct` in `materials.rs`, `DestructibleMaterial`), `src/crafting/` (recipes in `registry.rs`, stations in `types.rs`), `src/resource_nodes.rs` (nodes, gather rules), `src/building.rs` (tiers, pieces), plus `src/world/ruins.rs` and `src/world/meteor_shower.rs` for the exploration content.
- **Do not trust pre-recalibration combat tuning notes.** Earlier docs carried a swing-timing column (0.50 s hatchet) and a "constants live in `combat.rs`" sourcing that both drifted; the real swing is 0.78 s and the constants moved to `game_balance.rs` under `COMBAT_`. Re-derive from `src/game_balance.rs` plus `src/app/state/gather.rs` before quoting combat numbers. The current truth is in [PvP combat](pvp-combat.md).

Protocol/runtime anchors (verify before quoting): `PROTOCOL_VERSION = 43`, `SAVE_FORMAT_VERSION = 20`, `SERVER_TICK_RATE_HZ = 20.0`, `MAX_HEALTH = 100.0` (`src/protocol.rs`, `src/save/format.rs`). Every time-based balance constant derives its tick count from `SERVER_TICK_RATE_HZ`.

## Related docs

- [CLAUDE.md](../CLAUDE.md) - the singleplayer==multiplayer and gameplay-never-pauses invariants treated here as design choices.
- [docs/items-and-resources.md](items-and-resources.md) - the item/tool/resource registries the loop is built on, plus how to add one.
- [docs/crafting-and-deployables.md](crafting-and-deployables.md) - crafting queue, furnace state machine, and the deployable system that gates the loop.
- [docs/base-building-and-claims.md](base-building-and-claims.md) - building tiers, costs/HP, stability, doors, and the Tool Cupboard claim.
- [docs/pvp-combat.md](pvp-combat.md) - combat validation, weapon feel, death/respawn, and loot bags.
- [docs/meteor-shower.md](meteor-shower.md) - the meteor shower event (scheduler, siting, impact, contested loot).
- [docs/art-direction.md](art-direction.md) - the PBR-to-cel look trajectory and converted/pending status.
- [docs/toon-shading.md](toon-shading.md) - cel shader mechanics for extending the toon family.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - world generation, biomes, and persistence.
