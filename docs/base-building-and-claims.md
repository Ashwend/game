---
title: Base building, stability, doors, and Tool Cupboard claims
owns: building geometry/taxonomy, structural stability, the hammer flow, doors, and the Tool Cupboard claim system
when_to_read: Before changing building geometry, costs/HP, stability, doors, or the Tool Cupboard claim system.
sources:
  - src/building.rs (+ src/building/) - shared piece/tier taxonomy and dimensions (root), sockets.rs, collision.rs, claims.rs, stability.rs, costs.rs (client + server)
  - src/server/building.rs - placement validation, hammer repair/upgrade/demolish authority
  - src/server/stability.rs - refresh_structural_stability Dijkstra + orphan sweep
  - src/server/door.rs - code-lock door authority (DoorState)
  - src/server/claim.rs - Tool Cupboard claim projection + auth (CupboardState)
  - src/game_balance.rs - building/stability/door/claim tuning constants
  - src/items.rs - DeployableKind, DoorVariant, DestructibleMaterial, tool_effectiveness_pct
related:
  - docs/crafting-and-deployables.md - the unified DeployedEntity pipeline these pieces ride
  - docs/replication.md - DeployableStability/DeployableAuth diffs and the host mirror
  - docs/pvp-combat.md - the damage path that resolves tool-vs-material for raiding
  - docs/worlds-and-saves.md - the persisted deployable sub-states and save versioning
  - docs/server-authority.md - where building/door/claim command handlers sit on GameServer
---

# Base building, stability, doors, and Tool Cupboard claims

> When to read this: before changing building geometry, costs/HP, stability, doors, or the Tool Cupboard claim system. Source of truth: `src/building.rs` + `src/building/` (shared geometry), `src/server/building.rs` + `src/server/stability.rs` + `src/server/door.rs` + `src/server/claim.rs` (authority), `src/game_balance.rs` (tuning). Canonical invariants live in CLAUDE.md.

Base-building pieces, doors, and the Tool Cupboard are all ordinary `DeployedEntity` records riding the unified deployable pipeline (placement, replication, AoI, persistence, damage). This doc owns their geometry, the stability graph, the hammer, the door code lock, and the claim system. The pipeline itself (the one `DeployedEntity` struct, spill-on-destroy, the mirror sync) is documented in [crafting-and-deployables.md](crafting-and-deployables.md); read it first if you are adding a new deployable kind.

`src/building.rs` is the single source of truth for geometry/cost rules and is read by **both** the client (ghost preview + snapping) and the server (placement validation, damage, repair/upgrade costs), so the two can never disagree about what a legal placement is. Never fork a geometry rule into a client-only or server-only copy. The module is split by concern, re-exported flat (`crate::building::X` regardless of subfile): the taxonomy and piece dimensions in the root `src/building.rs`, socket-snap geometry in `src/building/sockets.rs`, multi-box colliders in `src/building/collision.rs`, claim cell math in `src/building/claims.rs`, support relations in `src/building/stability.rs`, and the cost/HP lookup tables in `src/building/costs.rs`.

## Piece and tier taxonomy

`BuildingPiece` (`src/building.rs` - `enum BuildingPiece`) has six variants, in declaration order: `Foundation`, `Wall`, `WindowWall`, `Doorway`, `Ceiling`, `Stairs`. `BuildingPiece::ALL` is the wheel/UI order. Two predicates classify them:

- `is_platform()` -> `Foundation | Ceiling`: the horizontal 3 m grid cells that carry walls on their edges.
- `is_wall_like()` -> `Wall | WindowWall | Doorway`: pieces that mount on a platform edge socket.

`BuildingTier` (`src/building.rs` - `enum BuildingTier`): `Sticks`, `HewnWood`, `Stone`, ordered cheapest to strongest. Every piece places at `Sticks` and upgrades in place with the hammer (`BuildingTier::next` walks `Sticks -> HewnWood -> Stone -> None`). The tier sets the `DestructibleMaterial` (`DeployableKind::material`) and therefore the raid-balance arm.

**Costs and HP** (`src/game_balance.rs`). Placement is always at Sticks, paid in raw `wood`; upgrades pay the tier cost. Wall HP per tier, foundations carry **1.5x** the wall budget (`building_max_health` returns `wall + wall / 2` for foundations):

| Tier | Wall HP | Foundation HP | Place/upgrade cost (foundation / wall) | Material |
| --- | --- | --- | --- | --- |
| Sticks | `BUILDING_STICKS_WALL_HP` 250 | 375 | 30 / 25 `wood` (placement) | `Sticks` |
| HewnWood | `BUILDING_HEWN_WOOD_WALL_HP` 3600 | 5400 | 12 / 10 `hewn_log` | `WoodBuilding` |
| Stone | `BUILDING_STONE_WALL_HP` 6000 | 9000 | 150 / 125 `stone` | `StoneBuilding` |

`hewn_log` is workbench-refined (10 raw `wood` each). There is no separate sticks item; branch piles hand-yield 1 `wood`. The constants are `BUILDING_*_WALL_HP`, `BUILDING_STICKS_COST_*`, `BUILDING_HEWN_WOOD_COST_*`, `BUILDING_STONE_COST_*` in `src/game_balance.rs`. Per CLAUDE.md, all balance values live there, not inline.

**Variant order is load-bearing.** `BuildingPiece`, `BuildingTier`, `DoorVariant`, and `DeployableKind` are positional in postcard saves and on the wire. New variants append at the end; reordering silently reinterprets old saves. Adding a field to a previously fieldless variant (as `Door` gained `variant` at save v17) also changes the layout. Any such change bumps `SAVE_FORMAT_VERSION` (`src/save/format.rs` - currently `20`). The most recent `DeployableKind` append is `Explosive { kind }` (placed blackpowder charges, v20), after `RuinCache` (world-spawned ruin loot cache, v19).

## Geometry, quarter-turn sockets, and multi-box colliders

Dimensions (`src/building.rs`): `FOUNDATION_SIZE_M` 3.0 (also every wall-like piece's width, so walls exactly span a foundation edge), `FOUNDATION_HEIGHT_M` 0.5, `WALL_HEIGHT_M` 3.0, `CEILING_THICKNESS_M` 0.2, `STAIR_STEP_COUNT` 8 (each step `STAIR_RISE_M / 8` = 0.375 m rise, under the controller's 0.45 m auto-step; the flight rises exactly one storey). A piece's `position` is the centre of its **base**.

**Yaw is always quarter-turn snapped** (`snap_yaw_quarter_turn`). This is the load-bearing invariant: every collider is then an exact axis-aligned box, which the AABB-only collision pipeline (`WorldBlock` + `BlockGrid`) represents losslessly. The server re-snaps a free foundation rather than trusting client yaw; a foundation left at arbitrary yaw would skew every wall it carries.

**Snapping** (`src/server/building.rs` - `apply_place_building_command`). The server re-derives the snapped pose (`snap_foundation` / `snap_wall_socket` / `snap_ceiling` / `snap_stairs`); the client preview is a best guess. A requested pose more than `SNAP_TOLERANCE_M` (0.75 m) from the snapped socket is refused, not silently corrected. Foundations place anywhere in the raise band (ground up to `FOUNDATION_RAISE_MAX_M` 1.5 m, down to `FOUNDATION_SINK_MAX_M` 0.25 m); snapping onto an existing foundation's 3 m neighbour grid inherits that foundation's yaw and height. Wall-like pieces mount on a platform edge socket or stack on a wall below, one piece per slot. Ceilings nest at `WALL_HEIGHT_M - CEILING_THICKNESS_M` so the walkable top is flush with wall tops.

**Colliders are multi-box.** `piece_local_boxes` (`src/building/collision.rs` - `piece_local_boxes`) returns per-piece local solid boxes; `building_collider_blocks` rotates and translates them into world `WorldBlock`s. Multi-box geometry is what makes window and doorway openings genuinely passable. The same boxes feed the client movement grid, the server spawn-safety grid, and the claim footprint test, so visual, collision, and gameplay always agree.

**Building-vs-building collision is resolved by SOCKET OCCUPANCY, not the AABB overlap test** (`src/server/building.rs` - `apply_place_building_command`, the `skip` match around the `obstruction` check). Walls legitimately touch their foundation and corner-overlap their neighbours, so the box-overlap test is skipped for wall-like-vs-building and stairs/ceiling-vs-wall pairs; `wall_slot_blocked` / `positions_match` reject same-socket duplicates instead. The box-overlap test only runs against non-building deployables (don't bisect a furnace) and foundation-vs-foundation.

## Structural stability

Every structural piece (and door) carries a stability percentage derived from its best path to the ground. The **relations** are defined once in `src/building/stability.rs` (`candidate_stability_pct`); the full-world recompute is a max-propagation Dijkstra over the support graph in `src/server/stability.rs` (`compute_stabilities`, driven by `refresh_structural_stability`).

- Foundations = 100.
- Each vertical hop (wall on a platform, wall on a wall, ceiling on a wall, stairs on a platform) retains `STABILITY_RETENTION_VERTICAL_PCT` (90%).
- A ceiling hanging off an adjacent ceiling (cantilever) retains `STABILITY_RETENTION_CEILING_NEIGHBOR_PCT` (35%) per tile, so ledges die fast (81 -> 28 -> 9, the third tile falls under the placement minimum and is refused).
- A door inherits its parent doorway's stability at **100% retention** (`src/server/stability.rs` - the `BuildingPiece::Doorway` block pushes `(door, 100)` as the retention factor, so the door equals the doorway's value, it is not pinned to an absolute 100).

**Placement gate.** A new piece must compute at least `BUILDING_MIN_PLACEMENT_STABILITY_PCT` (10%) or `apply_place_building_command` rejects it; `building_candidate_stability` computes the value from current stored stabilities. The client predicts the same number from the replicated `DeployableStability` component so the ghost goes red exactly where the server would refuse, and the look-at tooltip (`src/app/ui/deployable_overlay.rs`) shows the percentage and HP.

**`refresh_structural_stability` runs only on structural change** (place / destroy / upgrade-respawn / load), never per tick: it is a full-world Dijkstra. It is the one entry point that must run after every structural change, and it does three things in one pass (`src/server/stability.rs` - `refresh_structural_stability`):

1. **Cascade destroy.** Pieces that compute exactly 0 (their ground path is gone) are removed via `remove_deployed_entity_tracked`. One pass suffices: a piece supported only through doomed pieces already computes 0, so removing the zeros can't strand a survivor at a stale value. Knock out a foundation and the walls, ceilings, upper storeys, and mounted doors above all fall.
2. **Orphan free-deployable sweep.** Non-building, non-door deployables (furnace, box, bag, torch, cupboard) standing above `y > 0.25` whose `valid_deployable_surface` is gone are removed and their container contents spilled as a loot bag via `spill_container_contents`. A floor collapsing drops the furnace that sat on it.
3. **Claim footprint rebuild.** `recompute_claim_footprints` re-projects every Tool Cupboard claim from the structure graph that just changed.

**Stability and claim footprints are derived, never persisted.** `restore_deployed_entities` seeds stability at 100 and the post-load `refresh_structural_stability` recomputes both. Do not try to save or load them.

## The hammer: repair, upgrade, demolish

Routed through `BuildingCommand` (`src/server/building.rs` - `apply_building_command`). The hammer deals **zero** damage to everything (`tool_effectiveness_pct(Hammer, _) == 0`), it is not a raid tool; raiding goes through the swing/damage path in the deployable damage gate. The hammer's reach is the **placement reach** `DEPLOYABLE_PLACEMENT_REACH_M` (5.0 m), not the melee range: building pieces are 3 m spans whose `position` sits on the far edge of the foundation you stand on, so the melee radius would reject a tap on a wall right in front of you (`hammer_in_range` enforces this; its docstring says "melee range" but the code uses `PLACEMENT_REACH_M`).

- **Repair (anyone).** `repair_building` restores `BUILDING_REPAIR_FRACTION_PCT` (25%) of max HP per hit, cost on the swinger. Building blocks cost their tier material (`repair_cost`: `BUILDING_REPAIR_COST_STICKS/HEWN_WOOD/STONE` = 5 / 2 / 20). Doors cost their own material (`hewn_log` for the wood door, `iron_bar` for the iron door). Crafted deployables (furnace, workbench, bag, boxes) cost their recipe's primary material via `crafting::repair_material_for`. Helping a neighbour patch a wall is allowed by design.
- **Upgrade (authorized only).** `upgrade_building` advances to `tier.next()`, costs the target tier's materials (`upgrade_cost`), refills HP to the new max, and restarts the demolish window (`placed_at_tick = tick`). Authorization is `building_modify_allowed` (the builder, or anyone on a covering Tool Cupboard).
- **Demolish (authorized only, within the window).** `demolish_building` works only on `Building`/`Door` kinds, requires `building_modify_allowed`, and only while `tick - placed_at_tick <= BUILDING_DEMOLISH_WINDOW_TICKS` (15 minutes). Long enough to fix layout mistakes, short enough that a compromised base can't be deleted out from under a raid.

**Tier upgrade respawns the mirror entity.** `Deployable` identity (including `kind`) is immutable post-spawn, so a tier change despawns and respawns the mirror; clients see a normal remove + add (`sync_deployable_entities`). See [replication.md](replication.md).

## Doors

Doors are generic over `DoorVariant` (`src/items.rs` - `enum DoorVariant`): `HewnLog` and `Iron`. The variant is immutable spawn identity carried in `DeployableKind::Door { variant }`, and is the single lookup for the door's item id (`item_id`), HP (`max_hp`), raid material (`material`), and label. Adding a door is one `DoorVariant` arm plus a recipe and a model; nothing in placement, damage, replication, or persistence changes.

| Variant | Material | HP | Tool damage |
| --- | --- | --- | --- |
| HewnLog | `WoodBuilding` | `DOOR_MAX_HP` 1500 | axe 15% / pickaxe 5% (the soft spot) |
| Iron | `MetalBuilding` | `IRON_DOOR_MAX_HP` 3000 | 0% for every tool (tool-proof) |

Iron Door recipe (`src/crafting.rs` - `IRON_DOOR_RECIPE_ID`): 30 `iron_bar` + 4 `hewn_log` + 2 `plant_twine`, station `Workbench { min_tier: 1 }`. (The recipe's own `tier: 2` is its tech-gate field, distinct from the workbench tier it needs, which is 1.)

**Authority and lock** (`src/server/door.rs` - `DoorState`, `apply_door_command`). A door mounts only in a doorway opening via `DoorCommand::Place`; `DoorState.parent` is the doorway block, and destroying the doorway destroys the door. The lock code is 4-6 ASCII digits (`DOOR_CODE_MIN_LEN`/`DOOR_CODE_MAX_LEN` = 4/6, `code_is_valid`), chosen in a dialog when the door is hung. Nobody, including the placer, is authorized until they enter the code at the door once. A correct code **authorizes only**; the door stays shut until an explicit E-press.

- **Tap E** (`DoorCommand::Interact`) toggles open/closed for authorized accounts; everyone else gets `ServerMessage::DoorCodePrompt`.
- **Enter code** (`enter_door_code`) authorizes and replies `ServerMessage::DoorCodeResult` so the keypad plays its accepted/denied cue.
- **Change code** (`change_door_code`, hold-RMB wheel) requires authorization and revokes everyone else.
- **Hold E** opens the pick-up wheel; `pick_up_door` returns the item only when the area is unclaimed, or the sender is cupboard-authorized **and** has unlocked the door.

The open flag replicates via `DeployableActive`, animating the panel client-side and moving the door collider between the closed opening plane and the swung-panel AABB (`door_collider_blocks`), so movement collision, E-targeting, repair taps, and damage swings all land on the panel where it visibly is. **The code and authorized list are server-only + saved (`PersistedDoorState`), never replicated** (per CLAUDE.md's replicated-state rules; a code in a wire diff is a code leak).

## Tool Cupboard claims

The Tool Cupboard (`DeployableKind::ToolCupboard`, recipe `TOOL_CUPBOARD_RECIPE_ID`) is the anti-grief base-claim object. Authority is in `src/server/claim.rs` (`CupboardState` = the authorized account list). It must sit on a building platform (`on_building_platform`); while it stands it projects **building privilege** over its base.

**Footprint projection** (`src/building/claims.rs` - `claim_footprint_cells`). The claim is foundation-projected, not a sphere: a flood fill from the platform the cupboard rests on over platform adjacency (cardinal neighbours at the same height = contiguous floor; same XZ column at any height = stacked storeys), grown by a margin ring of `BUILDING_PRIVILEGE_MARGIN_CELLS` (5 cells, ~15 m). The result is stored as real XZ cell centres in `GameServer.claim_footprints` so the gate is a cheap point-in-cell test. Rebuilt on every structural change and on cupboard placement (`recompute_claim_footprints`, called from `refresh_structural_stability`). A foundation-projected claim means a raid base can't be wedged against someone's wall the way a fixed radius would allow.

**Auth model** (mirrors the door lock list, deliberately). The owner is implicit and permanent (on the `Deployable`, never in the list, never removable, so clearing the list can never lock the owner out). Anyone within reach authorizes **themselves** by tapping E (`ClaimCommand::AuthorizeSelf`, the Rust model where the real protection is keeping the cupboard behind locked doors). The placer is auto-added at placement but is otherwise an ordinary member. The hold-E wheel offers `DeauthorizeSelf` and `ClearList` (deauthorize everyone else; the caller keeps their access). Range is re-validated on every command (`cupboard_actor_in_range`), measured to the collider surface.

**Gates.** Three functions read `claim_footprints`:

- `claim_blocks_placement(position, placer)` and `claim_blocks_footprint(blocks, placer)`: gate **all** construction. `apply_place_building_command` uses the footprint-aware variant so a wall can't be butted against the boundary to poke into protected ground. Authorized by **any** covering claim, so a cooperating ally with their own cupboard isn't blocked by a neighbour's.
- `building_modify_allowed(position, account, is_owner)`: upgrade/demolish rights. Inside a claim they follow the cupboard's authorized list (shared base management); outside any claim they fall back to the original builder.

**No admin bypass.** Claim authorization binds everyone, including admins (`src/server/claim.rs` - the comment on `claim_blocks_placement`). This is distinct from the damage path, where admins **can** destroy anything. To build in a claimed area, walk up and tap E.

**The cupboard is a raidable soft target.** Its material is `WoodBuilding` (`DeployableKind::material` and `raidable()` both include `ToolCupboard`), HP `TOOL_CUPBOARD_MAX_HP` (1000). Destroying it lifts the base's claim, so it is a deliberate raid objective: an iron hatchet chews through over a couple of minutes. It is not stone-immune by design.

**Replication.** The authorized list replicates to the room via `DeployableAuth` (`src/server/deployable_ecs.rs` - `DeployableAuth(Vec<AccountId>)`) so clients can show the authorize tooltip and derive `CupboardAuthState` without a round trip; "am I authorized" is `owner == me || authorized.contains(me)`. Empty for every non-cupboard deployable. The list also persists in `PersistedCupboardState`.

## Raid balance: the central lever

The one place that answers "how well does tool X bite material Y" is `tool_effectiveness_pct` (`src/items.rs`), an integer percentage multiplier read by every destructible damage path (no per-entity special-casing). The building/door arms are the raid-balance table:

| Material (tier) | Axe | Pickaxe | Effect |
| --- | --- | --- | --- |
| `Sticks` | 300% | 200% | shreds in a few swings |
| `WoodBuilding` (hewn wood, wood door, cupboard) | 15% | 5% | slow but real tool raids (an iron hatchet needs ~400 swings and most of its durability per wall) |
| `StoneBuilding` (stone tier) | 0% | 0% | tool-proof by construction |
| `MetalBuilding` (iron door) | 0% | 0% | tool-proof; only explosives breach it |
| `Cloth` (sleeping bag) | 300% | 300% | tears in a couple of hits |

So a stone base with iron doors is tool-raid-proof by construction, and the wood door / Tool Cupboard are the intentional soft entry points. Bare hands never reach this table (rejected upstream in the damage gate); the `Hands` catch-all keeps the math total. This is balance data, so it lives in `src/items.rs`/`src/game_balance.rs` and changing a matchup is a single-line edit, per CLAUDE.md.

### Explosives: the stone/metal counter

Tools cannot breach `StoneBuilding` or `MetalBuilding`; blackpowder explosives are the designed counter, and their raid table is the explosive analogue of the tool one. The one place that answers "how well does charge X breach material Y" is `explosive_effectiveness_pct(kind, material)` (`src/items.rs`, beside `tool_effectiveness_pct`), a percent multiplier on the charge's `base_damage`; the actual numbers live in `src/game_balance.rs`. An explosion applies `base * effectiveness_pct/100 * linear_falloff` to every building piece, door, and deployable in radius, through the same deployable damage internals (so a destroyed piece spills contents and `refresh_structural_stability` collapses what it held up). The three charges and their per-material effectiveness (the wall-sticking ember charge was retired; a future top-tier charge will take its slot):

| Charge (base dmg) | Sticks | Wood | Stone | Metal |
| --- | --- | --- | --- | --- |
| Powder Bomb (300, thrown) | 100% | 40% | 8% | 0% |
| Powder Keg (900, placed) | 100% | 80% | 25% | 0% |
| Satchel Charge (2,000, placed) | 100% | 85% | 45% | 8% |

Resulting point-blank raid math (tuned against the wall/door HP above): a hewn wood wall (3,600) falls to 5 kegs; a stone wall (6,000) to 7 satchels. An iron door (3,000) is effectively raid-proof until the ember charge's replacement lands: the satchel's 8% (160/charge) would need ~19 charges. The raid-economics tests in `src/server/tests/explosives.rs` pin these counts end to end through `resolve_explosion`.

Charge mechanics that interact with the claim/building system:
- **Placing a charge is allowed inside an enemy claim.** That is the point of raiding, so `place_charge` (`src/server/deployables.rs`) skips the `claim_blocks_footprint` gate every other deployable passes; every other placement check (reach, finite guard, surface) still runs.
- **Placing arms the fuse immediately.** The charge carries a server-only `FuseState` countdown (`fuse: Option<FuseState>` on `DeployedEntity`, ticked by `tick_fuses` next to `tick_torches`); on zero it detonates via `resolve_explosion` and is removed.
- **Charges are fizzleable.** A placed charge has small HP (`EXPLOSIVE_CHARGE_HP`) and `Cloth` material, so any tool or projectile shreds it in a couple of hits; reaching 0 HP through the normal deployable damage path FIZZLES it (destroyed, no detonation, no refund, a toast to the owner), the defender's counterplay.
- **Resource nodes are exempt.** A charge is a raiding tool, not a mining tool: `resolve_explosion` deliberately leaves resource nodes untouched.

## Persistence and gotchas

- Seven persisted deployable sub-states live on `PersistedDeployedEntity`: `PersistedFurnaceState`, `PersistedDoorState`, `PersistedStorageBoxState`, `PersistedTorchState`, `PersistedCupboardState`, `PersistedRuinCacheState`, `PersistedFuseState` (`src/save/`). Stability and claim footprints are **not** persisted; both are recomputed on load.
- `SAVE_FORMAT_VERSION` is `20` (`src/save/format.rs`). Relevant history: torch v13, cupboard v16, door `variant` v17, player equipment v18, ruin cache v19, explosive charge fuse v20. Postcard is positional, so old saves are rejected on a layout change; any enum-variant append or fieldless-variant-gains-a-field change requires a version bump.
- When mutating any replicated field on a deployable, go through `deployed_entity_mut(id)` or `mark_deployable_dirty(id)` so the mirror re-syncs. A bare `deployed_entities.get_mut` bypasses the dirty flag and silently drops the diff. See the replicated-state rules in CLAUDE.md and [replication.md](replication.md).
- Range checks against placed structures measure to the **collider surface** (`within_horizontal_range_of_blocks`), load-bearing for hitting/opening 3 m foundation slabs and swung-open door panels, not the entity centre.

## Related docs

- [docs/crafting-and-deployables.md](crafting-and-deployables.md) - the unified `DeployedEntity` pipeline, spill-on-destroy, and the deployable damage gate these pieces ride.
- [docs/replication.md](replication.md) - `DeployableStability`/`DeployableAuth`/`DeployableActive` diffs, the host mirror, and why upgrade respawns the entity.
- [docs/pvp-combat.md](pvp-combat.md) - the swing/damage path that resolves `tool_effectiveness_pct` and the loot-bag spill from raided containers.
- [docs/items-and-resources.md](items-and-resources.md) - how item ids resolve and how tools/materials slot into the matchup table.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - the persisted deployable sub-states and save format versioning.
- [docs/server-authority.md](server-authority.md) - where the building/door/claim command handlers sit on `GameServer`.
- [src/game_balance.rs](../src/game_balance.rs) - every building/stability/door/claim constant cited above.
