---
title: Crafting queue, furnace, and the unified deployable system
owns: The placed-object layer (one DeployedEntity struct for every deployable kind) plus the serial per-player crafting queue, the furnace smelt state machine, torches, and loot bags.
when_to_read: Before touching crafting, furnaces, deployable placement/damage/destroy/spill, torches, loot bags, or adding a new deployable kind.
sources:
  - src/server/deployables.rs - DeployedEntity struct, apply_place_deployable_command, apply_damage_deployable_command, destroy_deployed_entity, spill_container_contents, place_torch, place_charge
  - src/server/crafting.rs - apply_crafting_command, enqueue_craft, cancel_craft, tick_crafting, cancel_all_jobs_for_disconnect
  - src/server/workbench.rs - apply_workbench_command, the tier upgrade via DEPLOYABLE_UPGRADES
  - src/server/fuse.rs - FuseState, tick_fuses, detonate_charge
  - src/server/explosion.rs - resolve_explosion
  - src/server/defuse.rs - the claim-authorized defuse + refund
  - src/server/ruin_cache.rs - RuinCacheState, tick_ruin_caches, roll_loot
  - src/crafting/registry.rs - REGISTERED_RECIPES, RecipeCategory
  - src/crafting/types.rs - RecipeDefinition, RecipeStation, MAX_CRAFTING_QUEUE_LEN
  - src/server/furnace/state.rs - FurnaceState, smelt_result, fuel_burn_ticks_for, FurnaceContainer
  - src/server/furnace/tick.rs - tick_one_furnace, tick_furnaces
  - src/server/furnace/commands.rs - apply_furnace_command, set_open_furnace_active, place_in_fuel_slot_with_swap
  - src/server/torch.rs - TorchState, tick_torches
  - src/server/loot_bag.rs - LootBag, OpenContainer, close_container, spawn_loot_bag, sleeper_is_lootable
  - src/items/deployables.rs - DeployableKind, DoorVariant, raidable, material
  - src/items/materials.rs - tool_effectiveness_pct, explosive_effectiveness_pct
  - src/items/upgrades.rs - DEPLOYABLE_UPGRADES
  - src/game_balance.rs - all tuning constants referenced here
related:
  - docs/base-building-and-claims.md - building pieces, stability, doors, and the Tool Cupboard claim that ride this same deployable pipeline
  - docs/items-and-resources.md - item ids, tool profiles, smeltables, and the registries recipes resolve against
  - docs/replication.md - how DeployableHealth/Active/Label and the per-player view components ship
  - docs/pvp-combat.md - the death flow that spawns loot bags
  - docs/playbooks/add-content.md - step-by-step for adding a recipe, smeltable, or deployable kind
  - docs/worlds-and-saves.md - the save format and append-only-enum rule
---

# Crafting queue, furnace, and the unified deployable system

> When to read this: before touching crafting, furnaces, deployable placement/damage, loot bags, or adding a new deployable kind. Source of truth: `src/server/deployables.rs`, `src/server/crafting.rs`, `src/server/furnace/`, `src/server/torch.rs`, `src/server/loot_bag.rs`. Canonical invariants live in CLAUDE.md.

This is the server-authoritative "placed stuff and crafting" layer. Several interaction surfaces share one pattern: authoritative state on `GameServer`, a client UI that reads replicated state and sends commands, and per-component replication of the result. Base building, doors, and the Tool Cupboard claim ride the same deployable pipeline but are documented in [base-building-and-claims.md](base-building-and-claims.md); this doc owns the deployable substrate they sit on, plus crafting, furnaces, torches, loot bags, the workbench tier upgrade, placed explosive charges, and world-spawned ruin caches.

## The central invariant: one DeployedEntity for every placed object

Every placed object in the world, workbench, furnace, building block, door, sleeping bag, storage box, torch, Tool Cupboard, is a single `DeployedEntity` record (`src/server/deployables.rs` - `DeployedEntity`) in `GameServer::deployed_entities: HashMap<DeployedEntityId, DeployedEntity>`. There is no per-kind table. The struct carries:

- Identity that is immutable post-spawn: `id`, `item_id`, `kind: DeployableKind`, `position`, `yaw`, `owner: Option<AccountId>`, `placed_at_tick`.
- Mutable shared state: `health`, `max_health`, `label: Option<String>`, `stability: u8`.
- One `Option<_>` sub-state field per kind that needs extra state: `furnace: Option<FurnaceState>`, `door: Option<DoorState>`, `storage: Option<StorageBoxState>`, `torch: Option<TorchState>`, `cupboard: Option<CupboardState>`, plus `fuse: Option<FuseState>` (a live placed charge) and `ruin_cache: Option<RuinCacheState>` (a world-spawned loot cache's refill timer). Every other kind leaves these `None`.

`DeployableKind` (`src/items/deployables.rs` - `DeployableKind`) has these variants: `Workbench { tier }`, `Furnace { tier }`, `Building { piece, tier }`, `Door { variant }`, `SleepingBag`, `StorageBox { tier }`, `Torch { wall }`, `ToolCupboard`, plus `RuinCache` (v19) and `Explosive { kind }` (v20). The enum is positional in postcard saves and on the wire, so new variants MUST append at the end and a new field on an existing fieldless variant changes the layout (see the save-format note below).

### Add-a-deployable-kind recipe

To add a placed object that carries new state, touch these five points (all in `src/server/deployables.rs` unless noted):

1. **Add the `DeployableKind` variant** at the end of the enum (`src/items/deployables.rs`), plus its `name()`, `material()`, and `raidable()` arms.
2. **Add a sub-state field** `Option<MyState>` to `DeployedEntity` (skip if the kind is stateless). Default it to `None` in `DeployedEntity::new`.
3. **Initialise it on place** in `apply_place_deployable_command` (the `if matches!(entity.kind, ...)` block after insert), mirroring how furnaces get `FurnaceState::default()` and boxes get `StorageBoxState::new(tier)`.
4. **Restore and persist it** in `restore_deployed_entities` and `persisted_deployed_entities`, and add the matching `Option<>` to `PersistedDeployedEntity` (`src/save/`). Bump `SAVE_FORMAT_VERSION`.
5. **Spill it on destroy** if it holds items: extend `spill_container_contents` so breaking it open is looting, not deletion (see below).

If the new state mutates post-spawn and the client must see it, also replicate a component (read [replication.md](replication.md) and [playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md) first). Mutate the entity via `self.deployed_entity_mut(id)` or `self.mark_deployable_dirty(id)`; a bare `deployed_entities.get_mut` bypasses the dirty flag and silently drops the diff.

## Crafting queue

Strictly serial, one queue per player. State lives on the per-client `crafting: PlayerCraftingState`, a bounded `Vec<CraftingJob>`. Each job tracks `recipe_id`, `quantity` (batch size), `progress_ticks`, `total_ticks`.

**Wire shape:** `ClientMessage::Crafting(CraftingCommand)` with `Enqueue { recipe_id, quantity }` / `Cancel { job_id }` (`src/protocol/commands.rs` - `CraftingCommand`). The queue replicates back to its owner via the `PlayerCrafting` component (owner-only), so the HUD reads its own queue straight off the replicated component, no snapshot variant.

**Caps:** queue length `MAX_CRAFTING_QUEUE_LEN = 16` (`src/crafting.rs`), batch size `MAX_CRAFT_BATCH_SIZE = 100` (`src/protocol/mod.rs`). `enqueue_craft` clamps the requested batch to `1..=100` before any input math.

**Flow** (`src/server/crafting.rs` - `enqueue_craft`, `tick_crafting`, `cancel_craft`):

1. `Enqueue` checks: recipe exists, station-in-range (`station_in_range`, see Stations below), queue not full, and `has_inputs` for the full batch. Then it consumes `per_unit x quantity` of every input atomically (one `take_items_from_inventory` per input, not a loop over units, so a partial failure can't half-debit) and pushes the job. Output is granted on completion.
2. `tick_crafting` runs once per `GameServer::tick`, sorted by client id for deterministic side-effect order. It advances only the head job's `progress_ticks` by one. When `progress_ticks >= total_ticks` the job is removed and `grant_craft_output` runs: the output lands in the inventory; overflow drops at the player's feet via the shared dropped-item path. Then the next job becomes head.
3. `Cancel` refunds the **FULL batch quantity** regardless of progress, `batch_input_quantity(input.quantity, job.quantity)`, not a proportional refund of the in-progress unit (test: `cancel_refunds_full_batch_quantity`). Overflow on refund drops at the feet.
4. On disconnect, `cancel_all_jobs_for_disconnect` refunds **every** queued job's inputs before the inventory snapshot persists, so a player is never billed for jobs that did not complete (test: `disconnect_refunds_queued_inputs`).

`craft_total_ticks` converts `recipe.craft_seconds * SERVER_TICK_RATE_HZ` (min 1 tick); `batch_total_ticks` scales by quantity.

### Recipe registry

`REGISTERED_RECIPES: &[RecipeDefinition]` in `src/crafting/registry.rs` is the append-only source of truth (`src/crafting.rs` is now a thin front re-exporting `registry.rs` for the rows and `types.rs` for the shapes). `RecipeDefinition` fields: `id`, `name`, `description`, `category: RecipeCategory` (`Materials | Tools | Weapons | Armor | Explosives | Building | Misc`, the combat categories back the browser filter chips), `inputs: &[CraftingInput]`, `output_item`, `output_quantity`, `craft_seconds: f32`, `tier: u8` (browser sort order ONLY, higher surfaces first; it is NOT the station tier and does not gate anything, `station.min_tier` gates), `station: RecipeStation`. Adding a recipe = appending one entry; keep ids stable since queued jobs and saves reference them. Registry tests assert every input/output resolves to a known item. Step-by-step in [playbooks/add-content.md](playbooks/add-content.md).

The list also holds the cloth and gunpowder intermediates, the four melee-weapon recipes, the three ranged recipes (wooden_bow, crossbow, arrow), the twelve armor-piece recipes, and the four explosive recipes. The bow and crossbow craft at their workbench tiers (t1 and t2), arrows craft by hand four at a time.

### Stations

`RecipeStation` is `None` or `Workbench { min_tier }`. `station_in_range` (`src/server/deployables.rs`) scans the player's placed deployables for one whose kind satisfies the station within that deployable's `station_radius` profile field. A tier-2 workbench satisfies a tier-1 requirement (`tier >= min_tier`), mirroring tool tiers. Station gating is enforced at craft time only, not at placement: a player who somehow holds a furnace can place it without a workbench.

### Workbench tiers and the generic upgrade

A placed workbench upgrades in place from tier 1 to tier 2; it is not a separate deployable. The upgrade path is DATA, not workbench-shaped code: `src/items/upgrades.rs - DEPLOYABLE_UPGRADES` declares `(from kind, to kind, cost)` rows (today the one row is Workbench t1 to t2 for 30 iron_bar + 6 salvaged_fittings + 4 meteorite ingots), and `upgrade_for(kind)` looks up the available upgrade. Because `DeployableKind::Furnace { tier }` already exists, a furnace tier 2 later is a table row, not new plumbing.

The client opens the bench with tap-E, which sets the replicated `open_workbench` pointer (`OpenWorkbenchView { id, tier }` on `PlayerPrivate`, the furnace-view pattern). `src/app/ui/workbench.rs - workbench_ui` renders the current tier, a blurb, and the cost list read from the compile-time upgrade table (costs never travel the wire), with an Upgrade button gated on client-side affordability. The server handler (`src/server/workbench.rs - apply_workbench_command`, `WorkbenchCommand::{Open, Close, Upgrade}`) re-validates entity, kind, range, and materials, consumes the cost, and mutates the kind.

Replication gotcha (the identity-component rule): `DeployableKind` is an immutable identity component post-spawn. The upgrade therefore removes and re-inserts the entry under the SAME id (plain remove + insert, not the `_tracked` variants), so the mirror despawns and respawns the entity with the new kind and model. The model swap is instant anyway, so the client-side respawn is invisible. Do not add a separate mutable tier component; that would split tier across two sources of truth.

The crafting browser also learned to show station requirements: a recipe gated behind `Workbench { min_tier: N }` the player cannot satisfy renders a grey "Requires Workbench Tier N" state rather than nothing (the server always enforced the gate; only the UI hint is new). Client can mirror station proximity for the label because deployable kinds replicate to peers.

### Crafting UI

- `src/app/ui/crafting/` (a directory: `mod.rs`, `recipes.rs`, `filter.rs`, `list.rs`, `details.rs`, `icon.rs`, `tests.rs`), the master/detail recipe browser: a searchable, category-filtered recipe list on the left (item icons + craftable-status dots) and a detail card on the right (description, per-ingredient have/need, batch quantity, Craft). Selection lives in `CraftingUiState::selected_recipe`. (Legacy docs cited a single `src/app/ui/crafting.rs` and later a per-row `rows.rs`; neither exists anymore.)
- `src/app/ui/crafting_queue.rs`, the always-on HUD stack that survives closing the browser and extrapolates the head job's progress bar between replication frames.

## Furnace smelt state machine

Furnaces are deployables with a `FurnaceState` sub-state (`src/server/furnace/state.rs`). The module is split three ways (each pure-data half is unit-testable without a `GameServer`):

- `state.rs`: `FurnaceState`, the `FurnaceContainer` shared-move adapter, persistence shims, and pure helpers (`smelt_result`, `fuel_burn_ticks_for`, slot math).
- `tick.rs`: `tick_one_furnace` + `GameServer::tick_furnaces`.
- `commands.rs`: `apply_furnace_command` and the `Open`/`Close`/`SetActive`/`Move`/`QuickTransfer` handlers.

**Slots:** one fuel slot + `FURNACE_ITEM_SLOT_COUNT = 6` smelt slots (`src/protocol/mod.rs`). The `FurnaceContainer` flat index is `0` = fuel, `1 + i` = item slot `i`. The fuel slot only accepts fuel items and never swaps on the move path; draining it resets the burn timer so the UI bar reads 0%.

**Fuel and recipes** (the two extension points for new smelt content):

- `fuel_burn_ticks_for`: `wood` burns `FURNACE_WOOD_BURN_TICKS` (4s), `coal` burns `FURNACE_COAL_BURN_TICKS` (16s).
- `smelt_result`: `iron_ore -> iron_bar` and `sulfur_ore -> sulfur`. One output per `FURNACE_SMELT_TICKS_PER_OUTPUT` (6s). Add a smelt recipe by extending `smelt_result` (and `fuel_burn_ticks_for` for a new fuel). The sulfur smelt is what activates the gunpowder chain (gunpowder is a bench t1 recipe, 2 coal + 1 sulfur to 2).

**Smelt loop** (`tick_one_furnace`):

- `active` is the master switch. Toggling it off (via `SetActive`, `set_open_furnace_active`) snaps `smelt_progress_ticks` to 0 so a player can't "save" a 99%-done timer by flipping off/on.
- Each tick: ignite a fresh fuel unit if `fuel_burn_ticks_left == 0`, find the head smeltable slot, pre-check the output fits, then spend one tick of fuel and advance `smelt_progress_ticks`. At `SMELT_TICKS_PER_OUTPUT` it consumes one input and deposits one output (merge into a matching stack first, else first empty slot).
- **Auto-shutoff** sets `active = false` and resets progress in three cases: no fuel, nothing smeltable, or the output will not fit anywhere in the grid.
- Persistence round-trips everything, so a reload resumes mid-smelt.

**Replication split (load-bearing):** of the fields the tick mutates, only the `active` flag is room-replicated, through `DeployableActive`. `tick_furnaces` uses `for_each_mut_then_mark` and flags a furnace dirty **only when `active` flips** (auto-shutoff). A steady burn mutates `fuel_burn_ticks_left` and `smelt_progress_ticks`, which are server-only and must stay out of the replication delta. Fuel/items/progress reach the **owning viewer** through the per-player view component (`PlayerOpenContainers`, carrying `OpenFurnaceView`), not the room mirror. If a furnace UI shows stale slots, it is the per-player view path, not the deployable mirror.

**Range revalidation:** every post-`Open` command re-runs `open_furnace_in_range` (`src/server/furnace/commands.rs`), because the client UI persists after the player walks away. Walking out of `FURNACE_INTERACT_RANGE_M` (3.0) closes the furnace and drops the command. The fuel-slot swap on quick-transfer (`place_in_fuel_slot_with_swap`) lives in `commands.rs` rather than the pure helpers because re-housing the displaced fuel needs the player's inventory; it calls `mark_deployable_dirty` after mutating.

**UI:** `src/app/ui/furnace.rs`, the modal panel rendered when the owner's `OpenFurnaceView` is `Some`.

## Deployable placement, damage, destroy, spill

### Placement (`apply_place_deployable_command`)

Free placement is for the simple kinds. Doors and building blocks reject this path (they go through `DoorCommand::Place` and the building plan); torches take their own `place_torch`. Gates, in order:

1. Item resolves to a deployable profile.
2. Reach: feet-to-target horizontal distance `<= DEPLOYABLE_PLACEMENT_REACH_M` (5.0).
3. Surface: `valid_deployable_surface`, which is `|y| <= 0.25` (world floor) OR exactly on the walkable top of a building platform cell. Yaw and cells are axis-aligned because building yaw is quarter-turn snapped.
4. Ruin exclusion: `ruin_blocks_placement` rejects any spot inside a ruin footprint plus `RUIN_PLACEMENT_EXCLUSION_MARGIN_M` (3 m), so the shared salvage chests can't be walled in or bag-camped. Applies to free placement, torches, and building pieces; explosive charges are exempt (raid tools work anywhere). The client ghost mirrors the gate (turns red) from the same seed-pure footprints.
5. Finite-value check on position and yaw.
6. AABB overlap: the candidate's `collider_blocks` must not intersect any existing deployable's solid boxes (`any_deployable_overlaps`).
7. Claim: `claim_blocks_footprint` rejects placement inside someone else's Tool Cupboard claim (footprint-aware so a wide box can't be slid halfway in). Tool Cupboards additionally must sit on a building platform.
8. Consume one item from inventory; stamp `owner = placing player's AccountId`.

On success it assigns the id, initialises any kind sub-state, inserts, and tracks the entity in the chunk manager. A placed Tool Cupboard triggers `recompute_claim_footprints`.

### Damage (`apply_damage_deployable_command`)

1. Per-tool cooldown: the swing obeys the same `next_gather_tick` throttle as gathering.
2. Bare hands rejected (`ToolKind::Hands`), defence in depth behind the client gate.
3. **Ownership gate:** if the attacker is not an admin AND the kind is not `raidable()` AND it has an owner that is not the attacker, the hit is dropped. `DeployableKind::raidable()` (`src/items/deployables.rs`) returns `true` for `Building`, `Door`, `SleepingBag`, **and `ToolCupboard`** (the cupboard is a deliberately raidable soft target; destroying it lifts the claim). World-spawned entities (`owner = None`) are damageable by anyone. Non-raidable player-placed entities (workbench, furnace) only by their owner. The ruin cache early-returns as indestructible regardless. Admins bypass for moderation.
4. **Range to the collider surface, not the centre:** `within_horizontal_range_of_blocks(player_pos, resolved_collider_blocks, DEPLOYABLE_DAMAGE_RANGE_M)` (3.0). This is load-bearing: a foundation is a 3 m slab whose centre sits out of range while its edge is at the player's feet, and a swung-open door panel's collider moves. Centre-distance would silently drop those hits.
5. Damage = `tool.gather_amount * DEPLOYABLE_DAMAGE_PER_GATHER_POINT (5) * tool_effectiveness_pct(tool.kind, kind.material()) / 100` (`tool_effectiveness_pct` lives in `src/items/materials.rs`). The decrement goes through `deployed_entity_mut` so `DeployableHealth` re-syncs. Stone-tier and metal building materials return 0% for every tool by construction (tool-proof); explosives are the intended breach path, routing through `explosive_effectiveness_pct` on the detonation side instead. Then the swing cooldown is applied and the active tool's durability is consumed.

### Destroy and spill

`destroy_deployed_entity` removes the entity (chunk untrack, clears any client's open-furnace / open-storage-box pointer, re-floats resting loot bags via `unsettle_loot_bags_on`), then calls `spill_container_contents`, then `refresh_structural_stability` (the single full-world Dijkstra that collapses 0-stability pieces and sweeps orphaned free deployables, documented in [base-building-and-claims.md](base-building-and-claims.md)).

`spill_container_contents` drops a removed entity's stored items as a loot bag at its position: storage box slots, and furnace fuel + smelt slots. **Breaking a container open is looting, not deletion.** This is the raid-design invariant. A kind with no contents is a no-op.

## Torches

A torch (`src/server/torch.rs` - `TorchState`) is a deployable carrying `{ active (lit), burn_ticks_left }`. Placed via `place_torch` (`src/server/deployables.rs`), which is the only free-placement path that relaxes the floor-surface check: a wall-mounted torch trusts the client raycast (`command.wall_mounted` -> `DeployableKind::Torch { wall }`), a floor torch still needs `valid_deployable_surface`. The mount is baked into the immutable `kind`, so it replicates for free. Placement is still reach-gated, claim-gated, and ruin-gated.

`tick_torches` counts `burn_ticks_left` down while lit (`TORCH_BURN_TICKS`, 8 hours) and extinguishes at 0. Like the furnace, only the `active` flip is replicated (`DeployableActive`), flagged dirty only on the extinguish edge; the steady countdown is server-only and persisted, so a reload resumes the timer.

## Explosives: placed charges, fuses, detonation, defuse

The two placed charges (powder keg, satchel charge) become a `DeployableKind::Explosive { kind }` when set, carrying a `FuseState` sub-state (`src/server/fuse.rs`). This is the torch/furnace timer pattern verbatim: `FuseState { armed, ticks_left }`, ticked by `tick_fuses` beside `tick_torches`, with the countdown server-only (never in the replication delta) and only a replicated flag flipping. A placed charge arms the instant it is set and hisses for 8 to 9 s (`POWDER_KEG_FUSE_TICKS` / `SATCHEL_CHARGE_FUSE_TICKS`). (The wall-sticking ember charge was retired; `ExplosiveKind` kept its first three variants stable, so saves and the wire are unaffected.)

- **Placement** (`place_charge` in `src/server/deployables.rs`): reach-gated like any deployable but it SKIPS the claim gate (raiding into an enemy claim is the point) and skips the footprint-overlap check (like torches). The thrown powder bomb is not placed at all: holding left click charges the throw like a bow draw (client `ThrowChargeState`, min fraction `POWDER_BOMB_MIN_THROW_FRACTION`, HUD reuses the ranged draw bar) and release sends `ClientMessage::Explosive(ExplosiveCommand::Throw { aim_dir, power })`. The server lights the fuse at the throw, scales launch speed by the clamped power (`POWDER_BOMB_MIN/MAX_THROW_SPEED_MPS`), and the bomb lives its whole life in the projectile sim: it bounces and rolls off solids (`step_thrown_explosive` in `src/server/projectiles.rs`, restitution + bounce friction in `game_balance`) and detonates IN PLACE via `resolve_explosion` when `POWDER_BOMB_FUSE_TICKS` runs out, never converting into a deployable (so it cannot be defused or fizzled; its short fuse is the counterplay window). The collider is a SPHERE of just the cloth ball (`POWDER_BOMB_BALL_RADIUS_M`; the fuse cap never collides): the sim sweeps the ball center against radius-inflated AABBs and a radius-lifted ground plane, and the client sinks the mesh by the same radius under the visual root and rolls it at `speed / radius`, so the bomb rolls smoothly on its ball and eases upright at rest (fuse tip surfaced for its spark rig).
- **Fizzle:** a charge is `Cloth` material with `EXPLOSIVE_CHARGE_HP = 50`, so a defender can shoot or hit it to 0 through the normal deployable-damage path. That destroys it with no detonation, no refund, and an owner toast (distinct from a defuse).
- **Detonation** (`detonate_charge` -> `src/server/explosion.rs - resolve_explosion`): at fuse zero the server resolves the blast. `resolve_explosion(center, kind)` applies `base_damage * explosive_effectiveness_pct(kind, material) * linear_falloff` to building pieces, doors, and deployables through the existing damage path (so `refresh_structural_stability` handles collapse), and `Blast` damage with falloff to players (self included) through the shared `apply_player_damage` tail. Resource nodes and ruin caches are deliberately untouched. Then it emits the cosmetic `ServerMessage::Explosion { position, kind }` cue within `EXPLOSION_CUE_RANGE_M` = 120 m. The effectiveness matrix and its raid-cost tests are in [items-and-resources.md](items-and-resources.md) and `src/server/tests/explosives.rs`; the raid math and damage-kind detail are in [pvp-combat.md](pvp-combat.md).
- **Defuse** (`src/server/defuse.rs`, `ExplosiveCommand::Defuse { id }`): a claim-authorized defender within `EXPLOSIVE_DEFUSE_REACH_M` = 5 m can hold-E defuse a live charge (a charge outside any claim is defusable by anyone). It removes the charge without detonation and refunds half the recipe materials (`EXPLOSIVE_DEFUSE_REFUND_NUMERATOR/DENOMINATOR`, floored), overflow dropping at the defuser's feet. The client drives it through the same hold-E wheel as door/bag/cupboard actions (`WheelAction::DefuseCharge`).

## Ruin caches

A `DeployableKind::RuinCache` (`src/server/ruin_cache.rs`) is a world-spawned salvage chest inside a burnt-out house (ruin POI), reusing the storage-box container (loot lives in `DeployedEntity::storage`, opened via `ClientMessage::OpenStorageBox` broadened to accept the cache kind at `RUIN_CACHE_INTERACT_RANGE_M`, `RUIN_CACHE_SLOT_COUNT = 6`). It is the only source of `salvaged_fittings`. Server-only refill state is `RuinCacheState { refill_at_tick, refill_counter }`, ticked by `tick_ruin_caches` beside the furnace: on empty it schedules `RUIN_CACHE_REFILL_TICKS`, and on fire it rolls `roll_loot(cache_id, refill_counter)` (seeded: always salvaged_fittings, weighted gunpowder / iron_bar / cloth, rare meteorite_alloy). Caches spawn on fresh worldgen (owner `None`), are indestructible (the damage path early-returns on the kind), and are not player-placeable (`equipable: false`, no recipe). Placement and the seed-pure ruin layout live in [worlds-and-saves.md](worlds-and-saves.md).

## Loot bags

A loot bag (`src/server/loot_bag.rs` - `LootBag`) is the container spawned at a dead player's feet holding everything the corpse carried, in `GameServer::loot_bags: HashMap<LootBagId, LootBag>`. It also receives spilled container contents (above). `LOOT_BAG_SLOT_COUNT` matches inventory + actionbar.

**Settle physics:** a bag spawns at chest height (`BAG_SPAWN_HEIGHT_M = 1.0`) and gravity-settles straight down to `rest_y`, the highest support surface under its XZ (world floor or any building/deployable top, scanned at spawn). Once `resting`, it skips per-tick integration, so the cost is O(spawned-this-tick), not O(every-bag). `unsettle_loot_bags_on` re-floats bags when their support is destroyed so they fall to the next surface.

**Interact range:** `LOOT_BAG_INTERACT_RANGE_M = 4.5`, deliberately looser than the 3.5 m melee swing range (`COMBAT_ATTACK_RANGE_M`) so a kill that knocks the corpse a step away does not put the loot out of reach. Every `Move`/`QuickTransfer` re-validates range, same as the furnace.

**Lifetime: NOT implemented.** The only despawn path is `close_container` GC of an **empty** bag that no one has open (`src/server/loot_bag.rs`). A lifetime/expiry sweep is an explicit future TODO; `spawn_tick` is dead-code-annotated bookkeeping for a future loot-glint cue. Do not document a lifetime sweep as shipped.

**`OpenContainer` unifies three container UIs on one wire path** (`LootBagCommand` + `OpenLootBagView`, tagged by `ContainerViewKind` so the panel titles correctly):

- `LootBag(LootBagId)`: a death-drop bag.
- `Sleeper(ClientId)`: a logged-out body's **live** inventory, read and written in place (non-destructive). `sleeper_is_lootable` requires offline AND not-`Dead` AND `health > 0`; all three terms are required and must stay single-sourced (the health term was a real bug, the view path once missed it). Detailed in [pvp-combat.md](pvp-combat.md).
- `StorageBox(DeployedEntityId)`: a placed storage box (`src/server/storage_box.rs`), opened via `ClientMessage::OpenStorageBox`. Small box = `STORAGE_BOX_SMALL_SLOT_COUNT` (8), large = `STORAGE_BOX_LARGE_SLOT_COUNT` (18); `tier >= 2` is large. Slots live on `DeployedEntity::storage` and persist; destroying a box spills via `spill_container_contents`.

**UI:** `src/app/ui/loot_bag.rs`, rendering from the replicated `LootBagContents` component (everyone in the room) and the owner-only open view.

## Module map and gotchas

- **Authority is single files, not directories.** Edit `src/server/deployables.rs` and `src/server/loot_bag.rs` directly. The same-named subdirectories hold only `tests.rs` (and `loot_bag/slots.rs`); there is no `mod.rs` with the implementation.
- **`src/protocol/` is a directory** (`commands.rs`, `messages.rs`, `mod.rs`, `items.rs`, ...). `MAX_CRAFT_BATCH_SIZE` and `FURNACE_ITEM_SLOT_COUNT` live in `src/protocol/mod.rs`; `OpenFurnaceView`, `CraftingCommand`, `FurnaceCommand`, `LootBagCommand`, and `PlaceDeployableCommand` in `src/protocol/commands.rs`.
- **`src/app/ui/crafting/` is a directory.** `src/app/ui/crafting_queue.rs`, `src/app/ui/furnace.rs`, `src/app/ui/workbench.rs` (the tier-upgrade overlay), `src/app/ui/loot_bag.rs`, and `src/app/ui/deployable_overlay.rs` (the look-at tooltip showing stability % and HP) are single files.
- **Save format is at `SAVE_FORMAT_VERSION = 20`** (`src/save/format.rs`). This slice persists the deployable sub-states on `PersistedDeployedEntity`: furnace, door, storage box, torch, cupboard, plus the fuse state (a live placed charge round-trips a reload, v20) and the ruin-cache refill state (v19); player equipment landed at v18. `DeployableKind` and its inner enums are positional in postcard, so **any new variant appends at the end and any new field on a variant bumps the version**; reordering silently reinterprets old saves. Stability and claim footprints are **not** persisted, they are recomputed on load. Meteor shower event state is deliberately not persisted (the scheduler rolls a fresh event on load).
- **Mutate replicated deployable fields through `deployed_entity_mut` / `mark_deployable_dirty`.** A bare `deployed_entities.get_mut` bypasses the dirty flag and the diff is silently dropped.
- All tuning constants referenced here live in `src/game_balance.rs` (balance never lives inline; see CLAUDE.md).

## Related docs

- [base-building-and-claims.md](base-building-and-claims.md) - building pieces, stability Dijkstra, doors, and the Tool Cupboard claim that share this deployable pipeline.
- [items-and-resources.md](items-and-resources.md) - item ids, tool profiles, smeltables, and the registries recipes resolve against.
- [replication.md](replication.md) - how `DeployableHealth`/`DeployableActive`/`DeployableLabel` and the per-player view components ship.
- [pvp-combat.md](pvp-combat.md) - the death/respawn flow that spawns loot bags and the sleeper-body lootability rule.
- [playbooks/add-content.md](playbooks/add-content.md) - step-by-step for adding a recipe, smeltable, or deployable kind.
- [playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md) - the procedure when a new deployable kind needs replicated mutable state.
- [worlds-and-saves.md](worlds-and-saves.md) - save format and the append-only-enum rule.
