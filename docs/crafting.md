# Crafting, Furnaces, and Loot Bags

Three distinct interaction surfaces share the same client/server pattern: a server-authoritative state machine, a client UI that reads state and sends commands, and per-component replication of the result.

## Crafting (recipe queue)

**Authoritative state:** `GameServer::clients[id].crafting`, per-player `PlayerCraftingState` carrying a bounded queue of jobs. Each job tracks `recipe_id`, `quantity` (batch), `progress_ticks`, and `total_ticks`.

**Wire shape:** `ClientMessage::Crafting(CraftingCommand)` with `Enqueue { recipe_id, quantity }` / `Cancel { job_id }`. State flows back via `PlayerPrivate` replication, the client UI reads its own crafting queue straight off the replicated component, no separate snapshot variant.

**Flow:**

1. Client sends `Enqueue`. Server checks: recipe exists, batch ≤ `MAX_CRAFT_BATCH_SIZE`, queue ≤ `MAX_CRAFTING_QUEUE_LEN`, station-in-range (workbench tier or none), all input materials present in inventory.
2. Server consumes inputs atomically (`batch × per_recipe_qty`) and appends the job. Output is granted on completion.
3. Each tick, `GameServer::tick_crafting` advances the head job's `progress_ticks`. On completion the output spawns in inventory; if it doesn't fit, it drops at the player's feet via the standard dropped-item path. Then the next job becomes head.
4. `Cancel` refunds the unconsumed remaining batch and removes the job. Mid-job cancellation refunds proportionally.
5. On disconnect, the player's in-flight jobs are cancelled (refunded into the persisted inventory snapshot).

**UI:**

- [`src/app/ui/crafting.rs`](../src/app/ui/crafting.rs), the full-screen modal browser. Recipe list with filter chips (categories) + search. Each row shows inputs/outputs, "craft" button.
- [`src/app/ui/crafting_queue.rs`](../src/app/ui/crafting_queue.rs), the always-on top-right HUD stack. Survives closing the browser. Animates the head job's progress between snapshots via a baseline-then-extrapolate scheme so the bar doesn't tick visibly with each replication frame.

**Adding a new recipe:** append to the `REGISTERED_RECIPES` slice in [`src/crafting.rs`](../src/crafting.rs). Fields:

```rust
RecipeDefinition {
    id: IRON_PICKAXE_RECIPE_ID,
    name: "Iron Pickaxe",
    description: "Forge a heavy iron head and set it on hewn handle stock.",
    category: RecipeCategory::Tools,
    inputs: &[
        CraftingInput::new(HEWN_LOG_ID, 2),
        CraftingInput::new(IRON_BAR_ID, 18),
        CraftingInput::new(PLANT_TWINE_ID, 2),
    ],
    output_item: IRON_PICKAXE_ID,
    output_quantity: 1,
    craft_seconds: 24.0,           // server converts to ticks at SERVER_TICK_RATE_HZ
    tier: 2,                       // sort order in the browser; 1 = stone-age, 2 = iron
    station: RecipeStation::Workbench { min_tier: 1 },
}
```

The output item must exist in the items registry ([`src/items.rs`](../src/items.rs)). The tier-2 chain is a good template for new progression: a `Materials` recipe refines a raw input at the bench (`wood` → `hewn_log`), the furnace smelts ore into a bar (`iron_ore` → `iron_bar`), and a workbench-gated `Tools` recipe assembles the refined inputs into the higher-tier tool. The registry tests in `crafting.rs` automatically assert every recipe's inputs and output resolve to known items.

## Furnaces

**Authoritative state:** `GameServer::deployed_entities[id].furnace: Option<FurnaceState>`, per-furnace fuel slot + items grid + active flag + burn/smelt timers. Furnaces are deployables (placed via the deployable path) with this extra sub-state attached.

**Module layout** (post-Phase-2 split):

- [`src/server/furnace/state.rs`](../src/server/furnace/state.rs), `FurnaceState`, constants re-exported from [`game_balance.rs`](../src/game_balance.rs), pure helpers (fuel lookup, smelt result table, stack merge primitives). No `GameServer` impl so the smelt math is unit-testable in isolation.
- [`src/server/furnace/tick.rs`](../src/server/furnace/tick.rs), `tick_one_furnace` + the `GameServer::tick_furnaces` entry point. Burn fuel, smelt the head input, auto-shutoff if fuel runs out or the output won't fit.
- [`src/server/furnace/commands.rs`](../src/server/furnace/commands.rs), `apply_furnace_command` dispatcher and all `Open`/`Close`/`SetActive`/`Move`/`QuickTransfer` handlers. Every post-`Open` command re-validates the player's distance to the furnace (`open_furnace_in_range`) so a stale client UI can't move items out of line-of-sight.

**Smelt loop semantics:**

- Active flag is the master switch. Toggling off resets `smelt_progress_ticks` so a player can't "save" a 99%-smelted timer by flipping off/on.
- Each tick consumes one tick of fuel (`fuel_burn_ticks_left`) and advances `smelt_progress_ticks`. When progress reaches `SMELT_TICKS_PER_OUTPUT`, one input is consumed and one output is granted.
- Auto-shutoff in three cases: fuel exhausted mid-smelt, nothing left to smelt, or the output won't fit anywhere in the grid.
- Persistence: `FurnaceState::to_persisted()` / `from_persisted()` round-trip everything. A reload picks up mid-smelt where it left off.

**Quick-transfer rules:**

- Player → furnace fuel slot: fuel items only, swap-with-displaced supported if the displaced fuel can fit back in the player's bag.
- Player → furnace items grid: anything; merges into matching stacks first, then takes the first empty slot.
- Furnace → player: routes through the same `add_stack_to_inventory` the pickup path uses, merging into matching stacks first, then first-empty.
- Partial drag (a `quantity` constraint) suppresses swap and never moves an item the player can't get back.

**UI:** [`src/app/ui/furnace.rs`](../src/app/ui/furnace.rs), modal panel rendered when `PlayerPrivate::open_furnace` is `Some`. Reads the live view (`OpenFurnaceView`) straight off the replicated component.

## Loot bags

**Authoritative state:** `GameServer::loot_bags: HashMap<LootBagId, LootBag>`. Spawned on player death carrying the dead player's inventory; despawned when fully looted or after a lifetime expiry.

**Module:** [`src/server/loot_bag.rs`](../src/server/loot_bag.rs). Same range-revalidation pattern as furnaces, `open_loot_bag_in_range` gates every `Move`/`QuickTransfer` command. Bags fall from chest height to the highest support under them (the world floor or a building/deployable top, `loot_bag_rest_y`), so a death on an upper storey leaves the loot on that floor; destroying the supporting piece re-floats the bag so it falls to the next support (`unsettle_loot_bags_on`).

**UI:** [`src/app/ui/loot_bag.rs`](../src/app/ui/loot_bag.rs). The slot grid renders from the replicated `LootBagContents` component (server pushes diffs whenever stacks move in or out).

## Deployables

**Authoritative state:** `GameServer::deployed_entities: HashMap<DeployedEntityId, DeployedEntity>`. Every placed structure.

**Module:** [`src/server/deployables.rs`](../src/server/deployables.rs).

**Placement** validates: reach (`DEPLOYABLE_PLACEMENT_REACH_M`), ground-level (`|y| ≤ 0.25 m`), no overlap with existing structures, item is present in the player's inventory. The placing player's steam id is stamped onto `DeployedEntity::owner` so damage can later be gated.

**Damage** validates: tool present (bare hands rejected), per-tool cooldown via `next_gather_tick`, range (`DEPLOYABLE_DAMAGE_RANGE_M`), and, since [src/server/deployables.rs](../src/server/deployables.rs), ownership. World-spawned entities (`owner = None`) are damageable by anyone; player-placed entities can only be damaged by their placer, **except raid targets** (`DeployableKind::raidable()`: building blocks, doors, sleeping bags), which anyone may damage. Admins can damage anything.

**Destruction:** removes from the map, untracks from the chunk manager, and clears any client's `open_furnace`/`open_loot_bag` pointer at this id, then recomputes structural stability (below), which is also what takes a doorway's mounted door with it.

## Base building

Rust-style building blocks placed via the **Building Plan** item (hold right click for the piece wheel, left click to place the ghost). Shared geometry/cost rules live in [`src/building.rs`](../src/building.rs); server authority in [`src/server/building.rs`](../src/server/building.rs); client snapping preview in [`src/app/systems/deployables/placement.rs`](../src/app/systems/deployables/placement.rs).

**Pieces:** foundation, wall, window wall, doorway, ceiling, stairs. Foundations place anywhere in the raise band, from ground level up to `FOUNDATION_RAISE_MAX_M` (1.5 m, aim-driven: looking at ground inside reach places at ground level, raising the aim past the reach ring lifts the slab), with a skirt mesh and a ground-reaching collider so stilted platforms never float; yaw snaps to quarter turns and the free ghost keeps one face toward the player until R takes over. Snapping onto an existing foundation's 3 m neighbour grid inherits that foundation's yaw *and* height, so bases stay square and level. Wall-like pieces (wall / window wall / doorway) mount on a **platform**'s edge sockets (foundation or ceiling) or stack directly on top of another wall, one piece per slot (`wall_slot_blocked` treats the coincident wall-top and ceiling-edge sockets as the same slot). Ceilings nest into the top of the wall band (base at `WALL_HEIGHT_M - CEILING_THICKNESS_M`), so the slab's walkable top is exactly flush with the wall tops and every storey is exactly one wall height tall regardless of build order; they snap to the cells flanking a wall's top edge and to cells adjacent to an existing ceiling (extending a ledge). Stairs occupy a full platform cell (eight 0.375 m steps, under the controller's 0.45 m auto-step; the flight rises exactly one storey so the top tread lands flush with the nested ceiling's top) and need the cell above open. Pieces are ordinary `DeployedEntity` records with `DeployableKind::Building { piece, tier }`, so replication, persistence, AoI, and damage all ride the deployable pipeline. Colliders are multi-box (`building_collider_blocks`) so window and doorway openings are genuinely passable; the same boxes feed the client movement grid and the server spawn-safety grid. New `BuildingPiece` variants must append at the end of the enum; the save layer and wire encode the variant index.

**Structural stability:** every structural piece carries a stability percentage derived from its best path to the ground (relations defined once in `candidate_stability_pct` in [`src/building.rs`](../src/building.rs); the full-world recompute is a max-propagation Dijkstra in [`src/server/stability.rs`](../src/server/stability.rs)). Foundations are 100%; each vertical hop (wall on a platform or on a wall, ceiling on a wall, stairs on a platform) keeps `STABILITY_RETENTION_VERTICAL_PCT` (90%), and a ceiling hanging off an adjacent ceiling keeps `STABILITY_RETENTION_CEILING_NEIGHBOR_PCT` (35%) per tile, so ledges die fast (81 → 28, the next tile computes 9 and is refused). Placement requires `BUILDING_MIN_PLACEMENT_STABILITY_PCT` (10%), which is what caps tower height and overhang; the client predicts the same number from the replicated `DeployableStability` component, so the ghost goes red where the server would refuse, and the look-at tooltip shows the percentage. The recompute runs only on structural change (place / destroy / load), never per tick. Destroying a piece drops everything whose stability hits zero: knock out a foundation and the walls, ceilings, upper storeys, and mounted doors above it all fall; knock out a ledge's only wall and the ledge goes with it.

**Tiers and raid balance:** every piece places at the Sticks tier and upgrades in place (Sticks → Hewn Wood → Stone). The first-draft Sticks look is paid in raw `wood`; the Hewn Wood upgrade is paid in workbench-refined `hewn_log` (10 wood each), and Stone in `stone`. There is no separate sticks item: branch piles on the ground hand-yield 1 wood. The tier sets the `DestructibleMaterial` and therefore the `tool_effectiveness_pct` arm: the sticks draft shreds in a few swings, hewn-wood buildings take a slow trickle (an iron hatchet needs ~400 swings and most of its durability for one wall), stone-tier takes **zero** damage from every tool. HP and costs live in [`src/game_balance.rs`](../src/game_balance.rs).

**Hammer:** left click on any placed structure repairs it (`BUILDING_REPAIR_FRACTION_PCT` of max HP per hit, anyone may repair). Building blocks cost tier materials, doors cost their own material (hewn logs for the wood door, iron bars for the iron door), and crafted deployables (furnace, workbench, bag, boxes) cost their recipe's *primary* material, stone for the furnace, wood for the workbench, at a quarter of the recipe amount per hit (`crafting::repair_material_for`), so a full repair from near-dead costs about the primary input of crafting fresh without the secondary materials. Hold right click for the wheel: Upgrade (owner-only, costs the target tier's materials, refills HP, restarts the demolish window) and Demolish (owner-only, only within `BUILDING_DEMOLISH_WINDOW_TICKS` of placement/upgrade, 15 minutes). The hammer deals zero damage to everything, it is not a raid tool.

**Tier upgrades and the mirror:** `Deployable` identity (including `kind`) is immutable post-spawn, so an upgrade despawns and respawns the mirror entity (see `sync_deployable_entities`); clients see a normal remove + add.

## Doors

**Door variants:** doors are generic. `DeployableKind::Door { variant: DoorVariant }` carries the variant (`DoorVariant::{HewnLog, Iron}` in [`src/items.rs`](../src/items.rs)), which is immutable spawn identity and the single lookup for the door's item id, HP, raid material, and display name. Adding a new door is one `DoorVariant` arm (item id + HP + material + label) plus a recipe and a model, nothing in the placement, damage, replication, or persistence paths changes. `DoorCommand::Place` carries the variant; the client derives it from the held door item's `DeployableKind::Door { variant }` and threads it through `GhostIntent::Door(variant)` and the code dialog.

**Hewn Log Door** ([`src/server/door.rs`](../src/server/door.rs)): crafted at a workbench, mounts only in a doorway opening via `DoorCommand::Place`. The ghost snaps to the nearest free doorway and shows a swing-arc indicator; right click flips hinge + swing (a half-turn of the pose). The lock code (4-6 digits) is chosen in a dialog when the door is hung; nobody, including the placer, is authorized until they enter the code at the door once; a correct code *authorizes only*, the door stays shut until an explicit E-press. Both outcomes also ship `ServerMessage::DoorCodeResult` so the keypad plays its accepted/denied cue, and the swing itself is voiced client-side by `door_swing_audio_system` watching the replicated open flag by value. A quick **tap** of E (`DoorCommand::Interact`) toggles open/closed for authorized accounts and answers everyone else with `ServerMessage::DoorCodePrompt`; **holding** E opens the pick-up wheel, whose `PickUp` returns the door item to inventory when the area is unclaimed or the sender is authorized on the covering Tool Cupboard *and* the sender has unlocked the door (so an unclaimed door's only protection is its code, the same key that opens it); `ChangeCode` (hold right-click) requires authorization and revokes everyone else. The open flag replicates via `DeployableActive`, animating the panel client-side and moving the door's collider between the closed opening plane and the swung panel's AABB (`door_collider_blocks` in [`src/building.rs`](../src/building.rs)), so movement collision, E-targeting, repair taps, and damage swings all land on the panel where it visibly is in either pose; the code + authorized list persist in `PersistedDoorState`. The wood door is the **soft spot** of a stone base: `WoodBuilding` material, so an iron hatchet chews through `DOOR_MAX_HP` (1500) in ~2.5 minutes.

**Iron Door:** the tool-proof upgrade. Same code lock and doorway mount, but the `Iron` variant's material is `DestructibleMaterial::MetalBuilding`, whose `tool_effectiveness_pct` arm is **0 for every tool**, so no hatchet or pickaxe can scratch it; only future explosives (a separate damage path) will breach it. It carries `IRON_DOOR_MAX_HP` (3000, double the wood door) and is the priciest door to put up: 30 iron bars + 4 hewn logs + 2 plant twine at a workbench (tier 2), a real iron sink for a base you intend to hold. With a stone base + iron doors, tool-raiding is impossible by construction, which is the whole point.

## Storage boxes

Placeable item containers ([`src/server/storage_box.rs`](../src/server/storage_box.rs)): the small box (8 slots, hand-craftable from wood) and the large box (18 slots, workbench tier 1). Both are authored Blender glbs matched to their icons (`art/items/storage_box_{small,large}/`). Press E on a placed box to open it; the box deliberately reuses the **loot-bag container machinery**: opening (`ClientMessage::OpenStorageBox`, kind + range validated) sets the client's `OpenContainer::StorageBox`, after which close/move/quick-transfer ride the shared `LootBagCommand` path and the same transfer panel (titled by `ContainerViewKind` on the view). Slots live on the `DeployedEntity` (`storage`) and persist in the save (`PersistedStorageBoxState`, format v12). Boxes are raidable plain Wood; destroying one spills its contents as a loot bag at the box's position, so breaking in is looting, not deletion. Furnaces spill the same way (fuel slot + smelt slots) via the shared `spill_container_contents`. Keeping things safe is what walls and doors are for.

## Sleeping bags

[`src/server/sleeping_bag.rs`](../src/server/sleeping_bag.rs): crafted from plant fiber, placed through the normal deployable path. The owner's bags are offered as spawn points on the death screen (`PlayerKilled.respawn_bags`, honoured by `ClientMessage::RespawnAtBag`). Tap E picks an owned bag back up; hold E opens the wheel with Rename (name replicates via `DeployableLabel` and persists as `label`).

## Where the wires live

Every per-entity state above is replicated as a Lightyear component, not as a `ServerMessage` snapshot. The component splits are documented in [docs/networking.md § Replication](networking.md#replication). Crafting queue, furnace contents, and loot bag contents live on `PlayerPrivate` (owner-only) or `LootBagContents` (everyone in the room) respectively; deployable health/active flag/label live on `DeployableHealth`/`DeployableActive`/`DeployableLabel` (everyone in the room). The `Deployable` identity component also carries `owner` so the client can gate owner-only affordances (hammer wheel, bag rename) before the server's authoritative check. Door lock codes are deliberately **not** replicated; they exist only server-side and in the save.

## Where to look next

- [docs/items-and-resources.md](items-and-resources.md) for how item ids resolve and how new tools/materials slot in.
- [docs/networking.md](networking.md) for the replication contract.
- [src/game_balance.rs](../src/game_balance.rs) for every tuning constant referenced above.
