# PvP combat

Player-vs-player melee, server-authoritative. Hits, knockback, death,
respawn, and the loot bag at the corpse all flow through the same
`ClientMessage` / `ServerMessage` / per-component replication path the
rest of the game uses — singleplayer loopback and direct multiplayer
both exercise the full chain. The client predicts swing visuals/audio
for responsiveness; the server is the only thing that decides whether
a hit landed, how much damage to deal, who killed whom, and where the
respawn lands.

## End-to-end hit chain

```
left-click (real tool equipped)
  → client picks SwingTarget::Player(id) from the pickup raycast
  → SwingImpact at the impact fraction of the swing animation
  → dispatch_player_swing: chip burst + camera kick + audio + floating "−N"
  → ClientMessage::AttackPlayer(target_id)
        ↓ reliable channel
        ↓
server::apply_attack_player_command
  1. cooldown gate (next_attack_tick)
  2. attacker == target reject
  3. attacker / target alive (PlayerLifecycle::Alive)
  4. real tool equipped (Hands / non-tool → reject)
  5. feet-to-feet distance ≤ ATTACK_RANGE_M
  6. target chest inside the attacker's view cone (ATTACK_CONE_COS)
  7. line-of-sight against world blocks
  8. damage_after_armor(raw, target.armor) → controller.health
  9. cooldown stamped
        ↓ envelopes
        ↓
ServerMessage::Knockback (→ target client only)
ServerMessage::PlayerImpact (→ broadcast except attacker)
PlayerPublic.health diff (→ all peers via replication)
```

The attacker never receives `PlayerImpact` — they already produced
their own feedback via prediction. Peers see the chip burst + audio.
Only the target gets the knockback impulse and the screen kick.

When `controller.health` hits zero on the same swing, the kill chain
runs inside `apply_attack_player_command` before it returns — see
[Death and respawn](#death-and-respawn) below.

## Damage model

Damage primitives live in `src/combat.rs` and never ship on the wire.

- `DamageKind` — `Blunt` today, `Projectile` reserved for the future
  bow/gun pass.
- `DamageInstance { raw, kind, knockback_speed, source }` — built on
  the server, lives on the stack while the damage path runs.
- `DamageSource::Player { client_id, tool }` — credits the attacker
  for the death splash and any future hit-direction indicator.
- `tool_player_damage(tool, attacker)` — returns `None` for
  `ToolKind::Hands`, so bare-hand swings short-circuit before any
  state mutates.
- `damage_after_armor(raw, armor)` — armor is clamped to `[0, 100]`
  percent and applied as `raw * (100 - armor) / 100`.

Armor is a per-player u8 (`PlayerArmor` component, replicated). Every
player ships with 0 today — there are no armor items defined — but
the math is wired up so a future `PlayerArmor` mutation just works
without any further protocol or path changes.

## Tuning

All combat constants are in `src/combat.rs`, `src/server/combat.rs`,
and `src/app/systems/items/pickup.rs`. Numbers chosen for "feels like
a survival game's first tier" — easy to revise.

| Knob | Value | Where | Why |
|---|---:|---|---|
| Player max HP | 100 | `MAX_HEALTH` in `protocol.rs` | Round number, allows 2-decimal % readouts. |
| Hatchet damage | 8 | `HATCHET_PVP_DAMAGE` | 0.50 s swing × 8 ≈ 16 DPS; 13 hits / ~6.5 s to kill. |
| Pickaxe damage | 15 | `PICKAXE_PVP_DAMAGE` | 1.60 s swing × 15 ≈ 9 DPS; 7 hits / ~11 s. |
| Hatchet knockback | 1.8 m/s | `HATCHET_KNOCKBACK_SPEED` | Light tap. |
| Pickaxe knockback | 4.0 m/s | `PICKAXE_KNOCKBACK_SPEED` | Heavy shove. |
| Attack range (client) | 3.0 m | `ATTACK_RANGE_M` in `pickup.rs` | Tight enough that "swing at player" wins over the resource node behind them. |
| Attack range (server) | 3.5 m | `ATTACK_RANGE_M` in `server/combat.rs` | 0.5 m looser than client to absorb movement-prediction delta. |
| Attack cone cosine | 0.92 (~23°) | `ATTACK_CONE_COS` | Matches `DEPLOYABLE_INTERACT_CONE_COS`. |
| Vertical knockback | 25 % of horizontal | `KNOCKBACK_VERTICAL_FRACTION` | Small upward kick so the target slides instead of grinding into the floor. |
| Per-swing cooldown | tool's `cooldown_ticks` | `set_attack_cooldown` | Reuses the existing per-tool swing cadence. |
| Respawn min distance | 12 m | `RESPAWN_MIN_DISTANCE_M` | Prevents spawn-camping. |

Hatchet is the DPS option; pickaxe is the burst option. Same trade as
the gather tools express.

## Wire protocol

In `src/protocol.rs`:

- `ClientMessage::AttackPlayer(AttackPlayerCommand { target_player_id })`
  — server picks the damage from the attacker's active tool so the
  client can't lie about how hard they hit.
- `ClientMessage::Respawn` — request the server to relocate + reset
  health. Rejected unless the issuer is currently dead.
- `ClientMessage::LootBag(LootBagCommand)` — `Open / Close / Move /
  QuickTransfer`, modelled on `FurnaceCommand` because the UI shape
  is identical.
- `ServerMessage::PlayerImpact { attacker, target, position, tool, damage_dealt }`
  — broadcast (except attacker) for the chip burst + impact audio +
  floating damage text. The post-armor `damage_dealt` is what the
  client displays.
- `ServerMessage::Knockback { impulse }` — target-only; the local
  prediction adds it to velocity.
- `ServerMessage::PlayerKilled { killer, killer_name }` — target-only;
  opens the death splash.

All five are reliable channel — they aren't safe to drop.

HP itself ships via the replicated `PlayerPublic.health` diff. No
separate message.

## Death and respawn

`PlayerLifecycle` (`src/server/player_ecs.rs`) is the authoritative
state: `Alive` (default) or `Dead { since_tick, killer }`. Replicated
to every peer in the chunk room — peers use it to drive the corpse
animation, the local owner uses it to gate input.

**Kill chain** (`kill_player` in `src/server/combat.rs`):

1. Snapshot the death position.
2. Drain every inventory + actionbar slot into a `Vec<ItemStack>` and
   spawn a single loot bag via `spawn_loot_bag` (see
   [Loot bag](#loot-bag)).
3. Set `PlayerLifecycle::Dead { since_tick, killer }`, zero velocity,
   clamp `controller.health` to 0.
4. Send `ServerMessage::PlayerKilled { killer, killer_name }` to the
   dying client (killer name pulled from the live `ServerClient.name`).

While dead:

- `apply_client_movement` in `src/server.rs` drops `Movement`
  messages so the corpse can't slide.
- The attack handler rejects any swing whose attacker or target is
  `Dead`.
- The client's `dispatch_player_swing` and held-item visual gate on
  `LocalPlayerState.lifecycle`.

**Respawn** (`apply_respawn_command`): rejects when the caller is
already alive, then `pick_safe_respawn_position` samples up to 24
candidates inside a 6–32 m ring around origin, picking one ≥ 12 m
from every alive peer. Falls back to world origin if no candidate
clears. Resets controller (full HP, zero velocity, grounded), clears
cooldowns, flips lifecycle to `Alive`, sends
`ServerMessage::Correction` so the client predictor snaps cleanly.

### Death splash + fade

`DeathSplash` lives on `MenuState` (set by the network tick when
`PlayerKilled` arrives) and carries two timers:

- `elapsed`: counts forward from death. Drives a 4 s fade from a
  transparent screen to fully black, then a 0.6 s fade-in of the
  "YOU DIED" title and the Respawn button.
- `closing_elapsed`: set by `begin_closing()` when the respawn
  `Correction` lands. Multiplies the black + title alpha by
  `(1 - closing/0.45s)` so the screen fades from black back to
  transparent instead of vanishing. The auto-clear hits exactly when
  the fade hits zero, so the HUD doesn't pop in for a frame under
  black.

The splash + dim both render at `Order::Tooltip` to sit above the
peer overlay, floating damage text, and HUD. Held items disappear
when the local lifecycle is `Dead` so no weapon dangles through the
fade.

Input while dead: every UI toggle (`chat_shortcut_system`,
`toggle_inventory_system`, `toggle_crafting_system`,
`toggle_pause_system`) bails on `death_splash.is_some()`. The
Respawn button is the only thing the player can press.

### Remote corpse animation

When a remote player flips to `Dead` the visual entity stays alive
and gets a `DyingPlayer` component (`src/app/systems/players.rs`).
The tick system drives:

1. **Kick** (0.12 s) — small upward shudder off the feet.
2. **Fall** (0.65 s) — rotates ~95° around the **feet pivot** (not
   the chest), so the head sweeps down to the ground.
3. **Bounce** — damped sine pulse layered on top of the fall angle
   right at impact.
4. **Hold** (0.4 s) — settled.
5. **Fade** (0.9 s) — alpha 1 → 0 via a per-spawn cloned
   `StandardMaterial { alpha_mode: Blend }` so the fade doesn't drag
   every other remote player along.
6. Visibility flips to `Hidden`; the entity stays in place so a
   respawn restores it without re-spawning the mesh.

Per-spawn random roll axis + magnitude keep stacked kills from
landing in identical poses. The animation isn't a true ragdoll — the
player mesh is a single baked mesh, no skeleton — but the pivot + roll
+ bounce read as a collapse rather than a stiff tilt.

Dead players are also filtered out of `collect_peer_overlay_entries`
so the nameplate doesn't hover over a corpse.

## Loot bag

A single container spawned at the death position holding every stack
the corpse was carrying. Replaces the older N-dropped-items pattern
so a kill is one E-press away from full loot. Behaves like a furnace
from the wire layer's perspective.

**Authoritative state** — `LootBag` in `src/server/loot_bag.rs`:
- `position, yaw, slots: Vec<Option<ItemStack>>` — fixed
  `LOOT_BAG_SLOT_COUNT = 49` (one player's worst-case inventory).
- `velocity_y, resting: bool` — gravity settles the bag from chest
  height (`+1.0 m`) at `BAG_GRAVITY = 18 m/s²` to `BAG_RESTING_Y =
  0.05 m`. `tick_loot_bags` integrates only non-resting bags so the
  cost is O(falling) not O(total).
- Stored in `GameServer::loot_bags: HashMap<LootBagId, LootBag>`.
- Anchored via `chunk_manager.track_loot_bag` so the AoI ring
  replicates it like any other chunk-anchored entity.

**Replication** — per-component, three components:
- `LootBag { id }` — identity, immutable post-spawn.
- `LootBagTransform { position, yaw }` — refreshed every tick by
  `sync_loot_bag_entities` in `src/net/host.rs` while gravity is
  running, then quiet.
- `LootBagContents(Vec<Option<ItemStack>>)` — refreshed when a
  player drags items in/out.

The client spawns a `NetworkLootBag` visual entity with the
shared dropped-item mesh + material in
`apply_loot_bags_system`.

**Open/close/move** — `apply_loot_bag_command` mirrors the furnace's
shape:

- `Open { id }`: range check (`LOOT_BAG_INTERACT_RANGE_M = 4.5 m`)
  then stamps `ServerClient.open_loot_bag`. The replication path
  ships an `OpenLootBagView` to the owning client via
  `PlayerPrivate.open_loot_bag`.
- `Close`: clears the open pointer. If no one else still has the
  bag open AND it's empty, the bag is destroyed.
- `Move { from, to, quantity }`: validated against the player's
  open-bag pointer; routes through the slot-level
  insert/take/restore helpers that handle merge / swap / overflow.
- `QuickTransfer { from }`: Shift+click. Bag → first empty
  inventory slot (merging into matching stacks first); player slot
  → first empty bag slot.

**Client UI** — `src/app/ui/loot_bag.rs`:

The bag panel renders bag contents + player inventory using the
same `draw_slot` widget the main inventory + furnace use. The drag
pipeline is unified via `UnifiedSlotRef::Bag(usize)`; drag releases
across bag↔inventory route through `LootBagCommand::Move` in
`drag.rs`. The actionbar is intentionally not drawn inside the bag
panel — the hotbar at the bottom of the viewport already shows it,
and a player who wants to loot straight to a hotbar slot drags down
into it.

Opening the inventory or crafting closes the bag via
`LootBagCommand::Close` (same pattern furnaces use). ESC closes the
bag instead of opening the pause menu (`close_loot_bag_on_escape_system`
fires the Close; `handle_pause_escape` bails on `loot_bag_open`).

Pickup tooltip: when the look ray hits a bag, the tooltip reads
"Loot bag — Press E to open / Drag items between the bag and your
inventory."

## Client feedback

| Effect | Source | Code |
|---|---|---|
| Chip burst on hit | Local prediction + `PlayerImpact` to peers | `ImpactEffectKind::FleshHit` in `app/state/gather.rs`, spawned by `spawn_impact_burst` |
| Impact audio | Same; both attacker (predicted) and observers | `SoundId::ImpactPlayerBlunt`, routed via `impact_sound_for_player(tool)`; `RemoteImpactEvent.is_player_hit` switches the audio dispatcher |
| Camera kick on attacker | Local prediction | `CameraImpactKick::trigger(tool)` — same kick as a deployable damage swing |
| Camera kick on target | `PlayerImpact` whose `target == local_client_id` | `CameraImpactKick::trigger_from_hit(attacker_tool)` — sharper, downward-biased |
| Knockback | Server-only message, target snaps velocity | `ServerMessage::Knockback` handler in `runtime.rs` |
| Floating damage text | Bevy entity with `FloatingDamageText` component | `app/ui/floating_text.rs` — orange for damage dealt, red for damage taken, randomised cone direction, sine-pop scale |

The audio + chip burst use the same `RemoteImpactEvent` plumbing as
resource impacts — adding `is_player_hit: bool` to the event was
cheaper than building a parallel pipeline.

## Multiplayer-test helpers

`./cli multiplayer-test` (see [Multiplayer Testing](multiplayer-testing.md))
pre-seeds the temp world save's `admins` list with both test client
Steam IDs and sets `GAME_TEST_AUTO_KIT=1` so each client fires
`/test-kit` once on its first in-game frame. Both windows boot with
hatchet + pickaxe + workbench + furnace + 100 of every resource,
flagged admin so `/tp` and the rest of the admin commands are
available without manual setup.

`/tp` (added for PvP testing): teleports every other connected
player to the issuer's position. Useful for engineering a fight
without walking the two clients into the same chunk manually. The
server pushes `ServerMessage::Correction` so client predictors snap
to the new pose — the runtime's 1 m position-delta threshold makes
that snap happen.

## Code map

Server-side:
- `src/combat.rs` — `DamageKind`, `DamageInstance`, `DamageSource`,
  `damage_after_armor`, `tool_player_damage`, per-tool tuning.
- `src/server/combat.rs` — `apply_attack_player_command`,
  `apply_respawn_command`, `kill_player`, line-of-sight check,
  knockback impulse builder, safe-spawn picker.
- `src/server/loot_bag.rs` — bag state struct, command handlers
  (`Open`/`Close`/`Move`/`QuickTransfer`), gravity tick,
  destroy-when-empty cleanup.
- `src/server/loot_bag_ecs.rs` — `LootBag`, `LootBagTransform`,
  `LootBagContents`, `LootBagIndex` components.
- `src/server/player_ecs.rs` — `PlayerArmor`, `PlayerLifecycle`
  components.
- `src/server/tests/combat.rs` — full anti-cheat + death + respawn
  test suite (16 cases). LOS unit tests in `src/server/combat.rs`.

Client-side:
- `src/app/systems/items/pickup.rs` — `best_player_target`,
  `best_loot_bag_target`, ray-AABB intersection, body extents.
- `src/app/systems/input/inventory_shortcuts.rs` —
  `dispatch_player_swing`, target-priority block, E-open routing.
- `src/app/systems/network.rs` — `PlayerImpact` /
  `Knockback` / `PlayerKilled` / `Correction` handlers, floating
  damage text spawning.
- `src/app/systems/players.rs` — `DyingPlayer` component, fall
  axis picker, death animation tick.
- `src/app/ui/death_splash.rs` — fade-in/out backdrop + Respawn
  button.
- `src/app/ui/floating_text.rs` — billboard damage numbers (cone
  drift + pop scale).
- `src/app/ui/loot_bag.rs` — bag transfer UI.
- `src/app/ui/inventory/drag.rs` — unified drag-release pipeline
  routing player↔player / player↔furnace / player↔bag.

Wire protocol:
- `src/protocol.rs` — `AttackPlayerCommand`, `Respawn`,
  `LootBagCommand`, `LootBagSlotRef`, `OpenLootBagView`,
  `PlayerImpact`, `Knockback`, `PlayerKilled`.

## Extending the model

The damage path is built around a `DamageInstance` that doesn't know
or care whether it came from a melee swing. Adding a new damage
source is a server-side change only:

- **Projectile damage** (bow/gun): a future `ProjectileTickCommand`
  raycasts against players → builds
  `DamageInstance { kind: Projectile, source: DamageSource::Projectile { … } }`
  → same `damage_after_armor` + `kill_player` path. No wire change
  needed.
- **Environment damage** (fall, fire): same shape with
  `DamageSource::Environment`. No attacker means the death splash's
  `killer_name` is `None` and renders "The world claimed you."
- **Critical hits**: extend `DamageInstance` with `crit: bool`.
  Floating text colour/font can switch on it; protocol unchanged.
- **Multi-piece armor** (head / chest / legs): replace
  `PlayerArmor { value: u8 }` with a struct of per-slot values, sum
  at damage time. Replication path is per-component so the existing
  pipeline picks up the new shape with no changes.
- **Hit-direction indicator** (incoming chevron on screen edge): the
  client already receives `PlayerImpact` with the attacker's id, and
  the attacker's `PlayerPublic.position` is replicated — the chevron
  reads angles off both. No additional server work.

## Out of scope today

- **Armor items.** `PlayerArmor` ships and replicates but no item
  references it. Adding one is a server-side mutation.
- **Combat log / scoreboard.** Killer name shown only on the death
  splash.
- **Healing / bandages.** Health regenerates on respawn only.
- **Teams / factions.** Friendly fire is always on.
- **Combat music.** PvP feedback stays in the SFX bus.
- **Decals on world / model.** Particles only.
- **Anti-griefing safe zones / PvP toggle per zone.**
- **Respawn cooldown timer.** Respawn is instant.
