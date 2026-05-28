# Crafting, Furnaces, and Loot Bags

Three distinct interaction surfaces share the same client/server pattern: a server-authoritative state machine, a client UI that reads state and sends commands, and per-component replication of the result.

## Crafting (recipe queue)

**Authoritative state:** `GameServer::clients[id].crafting` — per-player `PlayerCraftingState` carrying a bounded queue of jobs. Each job tracks `recipe_id`, `quantity` (batch), `progress_ticks`, and `total_ticks`.

**Wire shape:** `ClientMessage::Crafting(CraftingCommand)` with `Enqueue { recipe_id, quantity }` / `Cancel { job_id }`. State flows back via `PlayerPrivate` replication — the client UI reads its own crafting queue straight off the replicated component, no separate snapshot variant.

**Flow:**

1. Client sends `Enqueue`. Server checks: recipe exists, batch ≤ `MAX_CRAFT_BATCH_SIZE`, queue ≤ `MAX_CRAFTING_QUEUE_LEN`, station-in-range (workbench tier or none), all input materials present in inventory.
2. Server consumes inputs atomically (`batch × per_recipe_qty`) and appends the job. Output is granted on completion.
3. Each tick, `GameServer::tick_crafting` advances the head job's `progress_ticks`. On completion the output spawns in inventory; if it doesn't fit, it drops at the player's feet via the standard dropped-item path. Then the next job becomes head.
4. `Cancel` refunds the unconsumed remaining batch and removes the job. Mid-job cancellation refunds proportionally.
5. On disconnect, the player's in-flight jobs are cancelled (refunded into the persisted inventory snapshot).

**UI:**

- [`src/app/ui/crafting.rs`](../src/app/ui/crafting.rs) — the full-screen modal browser. Recipe list with filter chips (categories) + search. Each row shows inputs/outputs, "craft" button.
- [`src/app/ui/crafting_queue.rs`](../src/app/ui/crafting_queue.rs) — the always-on top-right HUD stack. Survives closing the browser. Animates the head job's progress between snapshots via a baseline-then-extrapolate scheme so the bar doesn't tick visibly with each replication frame.

**Adding a new recipe:** append to the `RECIPES` slice in [`src/crafting.rs`](../src/crafting.rs). Fields:

```rust
RecipeDefinition {
    id: "stone_pickaxe",
    name: "Stone Pickaxe",
    category: RecipeCategory::Tools,
    station: RecipeStation::Workbench { min_tier: 1 },
    inputs: &[(STONE_ID, 4), (STICK_ID, 2)],
    output_item: BASIC_PICKAXE_ID,
    output_quantity: 1,
    base_ticks: 2 * SERVER_TICK_RATE_HZ as u32,
    tier: 1,
}
```

The output item must exist in the items registry ([`src/items.rs`](../src/items.rs)).

## Furnaces

**Authoritative state:** `GameServer::deployed_entities[id].furnace: Option<FurnaceState>` — per-furnace fuel slot + items grid + active flag + burn/smelt timers. Furnaces are deployables (placed via the deployable path) with this extra sub-state attached.

**Module layout** (post-Phase-2 split):

- [`src/server/furnace/state.rs`](../src/server/furnace/state.rs) — `FurnaceState`, constants re-exported from [`game_balance.rs`](../src/game_balance.rs), pure helpers (fuel lookup, smelt result table, stack merge primitives). No `GameServer` impl so the smelt math is unit-testable in isolation.
- [`src/server/furnace/tick.rs`](../src/server/furnace/tick.rs) — `tick_one_furnace` + the `GameServer::tick_furnaces` entry point. Burn fuel, smelt the head input, auto-shutoff if fuel runs out or the output won't fit.
- [`src/server/furnace/commands.rs`](../src/server/furnace/commands.rs) — `apply_furnace_command` dispatcher and all `Open`/`Close`/`SetActive`/`Move`/`QuickTransfer` handlers. Every post-`Open` command re-validates the player's distance to the furnace (`open_furnace_in_range`) so a stale client UI can't move items out of line-of-sight.

**Smelt loop semantics:**

- Active flag is the master switch. Toggling off resets `smelt_progress_ticks` so a player can't "save" a 99%-smelted timer by flipping off/on.
- Each tick consumes one tick of fuel (`fuel_burn_ticks_left`) and advances `smelt_progress_ticks`. When progress reaches `SMELT_TICKS_PER_OUTPUT`, one input is consumed and one output is granted.
- Auto-shutoff in three cases: fuel exhausted mid-smelt, nothing left to smelt, or the output won't fit anywhere in the grid.
- Persistence: `FurnaceState::to_persisted()` / `from_persisted()` round-trip everything. A reload picks up mid-smelt where it left off.

**Quick-transfer rules:**

- Player → furnace fuel slot: fuel items only, swap-with-displaced supported if the displaced fuel can fit back in the player's bag.
- Player → furnace items grid: anything; merges into matching stacks first, then takes the first empty slot.
- Furnace → player: routes through the same `add_stack_to_inventory` the pickup path uses — merging into matching stacks first, then first-empty.
- Partial drag (a `quantity` constraint) suppresses swap and never moves an item the player can't get back.

**UI:** [`src/app/ui/furnace.rs`](../src/app/ui/furnace.rs) — modal panel rendered when `PlayerPrivate::open_furnace` is `Some`. Reads the live view (`OpenFurnaceView`) straight off the replicated component.

## Loot bags

**Authoritative state:** `GameServer::loot_bags: HashMap<LootBagId, LootBag>`. Spawned on player death carrying the dead player's inventory; despawned when fully looted or after a lifetime expiry.

**Module:** [`src/server/loot_bag.rs`](../src/server/loot_bag.rs). Same range-revalidation pattern as furnaces — `open_loot_bag_in_range` gates every `Move`/`QuickTransfer` command.

**UI:** [`src/app/ui/loot_bag.rs`](../src/app/ui/loot_bag.rs). The slot grid renders from the replicated `LootBagContents` component (server pushes diffs whenever stacks move in or out).

## Deployables

**Authoritative state:** `GameServer::deployed_entities: HashMap<DeployedEntityId, DeployedEntity>`. Every placed structure.

**Module:** [`src/server/deployables.rs`](../src/server/deployables.rs).

**Placement** validates: reach (`DEPLOYABLE_PLACEMENT_REACH_M`), ground-level (`|y| ≤ 0.25 m`), no overlap with existing structures, item is present in the player's inventory. The placing player's steam id is stamped onto `DeployedEntity::owner` so damage can later be gated.

**Damage** validates: tool present (bare hands rejected), per-tool cooldown via `next_gather_tick`, range (`DEPLOYABLE_DAMAGE_RANGE_M`), and — since [src/server/deployables.rs](../src/server/deployables.rs) — ownership. World-spawned entities (`owner = None`) are damageable by anyone; player-placed entities can only be damaged by their placer. Admins can still destroy by issuing a spawn-without-place command path.

**Destruction:** removes from the map, untracks from the chunk manager, and clears any client's `open_furnace`/`open_loot_bag` pointer at this id.

## Where the wires live

Every per-entity state above is replicated as a Lightyear component, not as a `ServerMessage` snapshot. The component splits are documented in [docs/networking.md § Replication](networking.md#replication). Crafting queue, furnace contents, and loot bag contents live on `PlayerPrivate` (owner-only) or `LootBagContents` (everyone in the room) respectively; deployable health/active flag live on `DeployableHealth`/`DeployableActive` (everyone in the room).

## Where to look next

- [docs/items-and-resources.md](items-and-resources.md) for how item ids resolve and how new tools/materials slot in.
- [docs/networking.md](networking.md) for the replication contract.
- [src/game_balance.rs](../src/game_balance.rs) for every tuning constant referenced above.
