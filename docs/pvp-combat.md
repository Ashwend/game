---
title: PvP combat, death, respawn, and loot bags
owns: The server-authoritative PvP melee path (hit validation, damage, knockback), the death/respawn lifecycle, and corpse loot bags.
when_to_read: Before touching combat validation, weapon feel/damage, knockback, the death/respawn flow, or loot bags.
sources:
  - src/server/combat.rs - apply_attack_player_command / apply_player_damage / kill_player / pick_safe_spawn / line_of_sight_clear
  - src/combat.rs - AttackProfile / DamageInstance / resolve_attack_profile / damage_after_armor / effective_armor_after_pierce / player_body_ray_entry
  - src/items/armor.rs - ArmorProfile, ArmorProtection, equipped_protection
  - src/server/projectiles.rs - server ballistic sim, arrow hit resolution (DamageKind::Projectile)
  - src/server/explosion.rs, src/server/meteor_shower.rs - resolve_explosion / blast resolution (DamageKind::Blast)
  - src/game_balance.rs - COMBAT_* / RESPAWN_MIN_DISTANCE_M / *_PVP_DAMAGE / *_KNOCKBACK_SPEED / weapon / armor / explosive constants
  - src/protocol/messages.rs - AttackPlayer / Ranged / Explosive / Respawn / RespawnAtBag / PlayerImpact / Knockback / PlayerKilled / Explosion / Correction + delivery()
  - src/app/state/gather.rs - swing_duration_seconds / COMBAT_MISS_RECOVERY_SECONDS cadence
  - src/server/loot_bag.rs - LootBag state, spawn_loot_bag, container command path
related:
  - docs/crafting-and-deployables.md - the shared loot-bag/OpenContainer wire path and spill-on-destroy
  - docs/server-authority.md - GameServer command dispatch and tick subsystems
  - docs/replication.md - per-component replication of player HP/lifecycle and loot bags
  - docs/movement.md - knockback applied to the client-authoritative predictor
  - docs/items-and-resources.md - ToolProfile/WeaponProfile/ArmorProfile/RangedProfile rows, per-tool tiers, durability wear
  - docs/crafting-and-deployables.md - the placed charges, fuse ticking, defuse, and explosive raid math
---

# PvP combat, death, respawn, and loot bags

> When to read this: before touching combat validation, weapon feel/damage, knockback, the death/respawn flow, or loot bags. Source of truth: `src/server/combat.rs`, `src/combat.rs`, `src/game_balance.rs` (COMBAT_*), `src/server/loot_bag.rs`. Canonical invariants live in CLAUDE.md.

PvP is server-authoritative across three damage families now: melee (tools and weapons), ranged (bow/crossbow arrows), and blast (explosives and the meteor shower). A left-click melee swing sends `ClientMessage::AttackPlayer(target_player_id)`; the server re-validates the whole chain, applies armor-reduced damage to the target's `controller.health`, and ships the consequences (`Correction`, `Knockback`, `PlayerImpact`, and on a kill `PlayerKilled`). Ranged and explosives ride their own intent messages (`Ranged` / `Explosive`) but converge on the same shared post-hit tail. The client predicts only the swing visuals/audio (and its own arrow arc) for responsiveness; it never decides whether a hit landed, how much it dealt, who died, or where a respawn lands. Singleplayer loopback and direct multiplayer both run this identical path (see CLAUDE.md: singleplayer == multiplayer).

## Hit chain and validation order

Entry point: `GameServer::apply_attack_player_command` in `src/server/combat.rs - apply_attack_player_command`. Every rejection bails before any state mutation, so a forged `AttackPlayer` for an out-of-range or wall-hidden target gets no damage and no feedback. Actual order:

1. **Self-attack** reject: `target == attacker` silently dropped.
2. **Cooldown**: `self.tick < attacker.next_attack_tick` rejects (the profile's `cooldown_ticks`).
3. **Attack profile resolve**: read the attacker's active actionbar item and derive an `AttackProfile` via `src/combat.rs - resolve_attack_profile`, which reads the item's `WeaponProfile` first and falls back to its `ToolProfile` (`attack_profile_from_tool` returns `None` for `ToolKind::Hands`/`Hammer` and any tool whose `player_damage == 0`). A `None` short-circuits with no cooldown touched. The resolved profile carries `damage`, `reach_m`, `cooldown_ticks`, `knockback_speed`, `armor_pierce_pct`, and `kind`, so the rest of the chain is profile-driven rather than branching per weapon.
4. **Attacker alive**: `attacker.lifecycle.is_dead()` rejects (a dying-frame race could otherwise let a corpse fire one last swing).
5. **Target alive**: `target.lifecycle.is_dead() || target.controller.health <= 0.0` rejects (no chain damage on a corpse).
6. **Range**: feet-to-feet horizontal distance within the profile's `reach_m` (the 3.5 m `COMBAT_ATTACK_RANGE_M` standard, or the spear's 4.5 m). Horizontal-only so a target on a one-block step is still meleeable.
7. **Aim (live targets only)**: the look ray must pass through the target's body box, tested with `crate::combat::player_body_ray_entry`. Sleeping bodies waive this step.
8. **Line of sight**: `line_of_sight_clear` against the world/structure block grid between the attacker's eye and the target's chest anchor.

On success the shared post-hit tail (`apply_player_damage`, the helper factored out in Phase 1 so projectiles and explosions reuse it) runs: apply `armor_pierce_pct` to the target's per-kind protection, subtract post-armor damage, push a `Correction` to the victim, a private `Knockback` to the victim, and a range-gated `PlayerImpact` broadcast to peers (within `IMPACT_MESSAGE_RANGE_M` = 80 m, excluding the attacker). If HP hit zero, `kill_player` runs inline before returning. Finally `consume_active_tool_durability` wears the item and `set_attack_cooldown` stamps `next_attack_tick`.

Note the validation comment in `src/server/combat.rs` still lists the order as cooldown -> self -> ... but the code runs self-attack first; behavior is otherwise as numbered above.

### Shared ray-AABB body box (client targeting == server accept)

The aim test is **not** a view cone. It is a slab-method ray-AABB intersection against the target's body box, `src/combat.rs - player_body_ray_entry`. The **same function** is called by:

- the client to pick which player a swing targets and to predict the impact (`src/app/systems/items/pickup/targets.rs - best_player_target`, gated by a client `ATTACK_RANGE_M` = 3.0 m), and
- the server to validate the incoming `AttackPlayer` (`src/server/combat.rs - apply_attack_player_command`).

Because both sides test the same volume with the same ray math, "my crosshair was on them" and "the server accepted the hit" cannot disagree. This replaced an older server-only eye-to-chest cone test that rejected point-blank hits: at close range the eye (1.62 m) sits well above the chest point, tilting the eye-to-chest vector below the cone, so the server dropped hits the attacker had already shown predicted feedback for. `COMBAT_ATTACK_CONE_COS` (0.92) still exists in `game_balance.rs` but is now orphaned: it is no longer used by the PvP aim gate, and nothing else references it either. The deployable-interact look cone is a separate constant, `DEPLOYABLE_LOOK_CONE_COS` = 0.91 in `src/app/ui/deployable_overlay.rs`.

Standing box (`COMBAT_PLAYER_BODY_*`): half-width 0.40, half-height 0.95, centre-Y 0.95 (spans y ~= 0 .. 1.9). Sleeping box (`COMBAT_SLEEPING_BODY_*`): low and wide, half-width 0.9, half-height 0.4, centre-Y 0.35.

**Invariant:** if you change the player hit volume, change `player_body_ray_entry` and the `COMBAT_PLAYER_BODY_*` / `COMBAT_SLEEPING_BODY_*` constants only. Never give the client and server separate hit boxes; diverging them re-introduces the point-blank-rejection class of bug.

### Sleeping bodies are PvP targets

A logged-out player (`!target.online`) is a hittable low/wide body box. The aim test is **waived** for sleepers (range + LOS only); a helpless body should be hittable without precise aim. The LOS/impact anchor drops from chest height (`COMBAT_TARGET_CHEST_HEIGHT` = 0.95) to `COMBAT_SLEEPING_BODY_CENTRE_Y` = 0.35 (from `src/game_balance.rs`, imported in `src/server/combat.rs` as `SLEEPING_HIT_HEIGHT`) so a standing attacker looking down has a clear line. When changing the hit logic, exercise both the standing and sleeping paths.

## Damage primitives (wire-invisible)

In `src/combat.rs`, never serialized to the wire:

- `DamageKind { Blunt, Projectile, Blast }`, each reduced by its own armor column. Melee (tools and weapons) produces `Blunt`; the bow/crossbow projectile path (`src/server/projectiles.rs`) produces `Projectile`; explosives (`src/server/explosion.rs`) and the meteor shower (`src/server/meteor_shower.rs`) produce `Blast`. All three converge on the shared `apply_player_damage` post-hit tail with the matching armor column.
- `AttackProfile { damage, reach_m, cooldown_ticks, knockback_speed, armor_pierce_pct, kind }`: the unified profile resolved at the top of the melee chain from the item's `WeaponProfile` (weapon first) or `ToolProfile` (`resolve_attack_profile` / `attack_profile_from_tool`). Reach and pierce are fields, not special cases, so a new weapon is a registry row, not a new code branch.
- `DamageInstance { raw: u32, kind, knockback_speed: f32, source }`. Built on the server, lives on the stack while the damage path runs.
- `DamageSource`. **Still only the `Player { client_id, model }` variant.** Projectiles and explosions do not add a `DamageSource` variant; they feed the shared post-hit tail through a `PlayerDamageHit` that carries the attacker id and `DamageKind` directly. There is no `Environment` variant (the meteor attributes damage the same way).
- `effective_armor_after_pierce(armor, armor_pierce_pct) -> u8`: applies a weapon's pierce to the target's per-kind armor before mitigation (`armor * (100 - pierce) / 100`). The iron mace's 50% pierce is what makes it the anti-armor answer.
- `damage_after_armor(raw, armor) -> u32`: armor clamped to `<= 100`, then `raw * (100 - armor) / 100` with saturating math (clamped armor can never heal).

Armor is now populated. `src/items/armor.rs - ArmorProtection { melee, projectile, blast }` is the per-kind mitigation, recomputed by `equipped_protection(&equipment_slots)` on every equipment change and cached on `ServerClient.protection` (`src/server.rs`). The damage tail reads `protection.for_kind(profile.kind)`, applies pierce, then `damage_after_armor`. The set totals are capped at `ARMOR_TOTAL_CAP_PCT = 60` per kind, so a player always takes at least 40% of every hit. The replicated `PlayerArmor(u8)` (`src/server/player_ecs.rs`) survives as the HUD readout, fed the melee column; the per-kind protection itself never ships (it is re-derived server-side from the worn pieces).

The client never learns the raw damage. The server picks damage from the attacker's active item profile, so `AttackPlayer` carries only `target_player_id` and a modified client can't lie about how hard it hit.

### Per-weapon and per-tool tuning (in `src/game_balance.rs`)

Tools remain viable desperation weapons: their PvP damage scales per tier, and knockback is a per-kind trait (upgrading a tool changes its damage, not the feel of getting hit).

| Tool | PvP damage | Knockback | Swing animation |
|---|---:|---:|---:|
| Stone hatchet | `STONE_HATCHET_PVP_DAMAGE` = 8 | `HATCHET_KNOCKBACK_SPEED` = 1.8 m/s | `AXE_SWING_SECONDS` = 0.78 s |
| Iron hatchet | `IRON_HATCHET_PVP_DAMAGE` = 12 | 1.8 m/s | 0.78 s |
| Stone pickaxe | `STONE_PICKAXE_PVP_DAMAGE` = 15 | `PICKAXE_KNOCKBACK_SPEED` = 4.0 m/s | `PICKAXE_SWING_SECONDS` = 1.60 s |
| Iron pickaxe | `IRON_PICKAXE_PVP_DAMAGE` = 22 | 4.0 m/s | 1.60 s |
| Hands / Hammer | none (`resolve_attack_profile` returns `None`) | n/a | hands 0.42 s |

The dedicated melee weapons (a `WeaponProfile`, read before the tool) widen the light-to-heavy axis. Damage/cooldown/pierce constants are the `WOODEN_CLUB_*`, `STONE_SPEAR_*`, `IRON_SWORD_*`, `IRON_MACE_*` families in `game_balance.rs`:

| Weapon | PvP damage | Cooldown ticks | Reach | Knockback | Notable |
|---|---:|---:|---:|---:|---|
| Wooden club | 12 | 7 | 3.5 m | 2.4 m/s | Fast, cheap, hand-crafted starter. |
| Stone spear | 16 | 11 | 4.5 m | 1.2 m/s | Reaches past the standard 3.5 m; slow, controls space. |
| Iron sword | 20 | 9 | 3.5 m | 3.0 m/s | The medium-speed workhorse (bench t1). |
| Iron mace | 26 | 14 | 3.5 m | 5.0 m/s | Slowest, biggest knockback, `IRON_MACE_ARMOR_PIERCE_PCT = 50` (the anti-armor answer). |

Weapons do hands-tier damage to structures (they are not raid tools) and gather nothing. Cooldown ordering is club < sword < spear < mace, so the mace's payoff is paid for in commitment. Iron hits ~1.5x stone among the tools. Knockback also gets a small vertical pop: `COMBAT_KNOCKBACK_VERTICAL_FRACTION` = 0.25 of horizontal magnitude, so the target slides instead of grinding into the floor. The co-located edge case (zero horizontal separation) shoves straight up.

All combat tuning lives in `src/game_balance.rs` under `COMBAT_*` / `RESPAWN_*` / `*_PVP_DAMAGE` / `*_KNOCKBACK_SPEED` / the weapon and armor and explosive families, re-imported with aliases by the combat modules. Never inline a combat magic number (see CLAUDE.md: balance-in-game_balance.rs).

### Armor and per-kind mitigation

Four equipment slots (`EquipmentSlot { Head, Chest, Legs, Feet }`) take three armor sets. Each worn piece contributes protection per damage kind; the four pieces sum, clamped at `ARMOR_TOTAL_CAP_PCT = 60` per kind. Set totals (melee / projectile / blast), distributed across slots roughly chest 40% / head 25% / legs 25% / feet 10% and pinned by `full_*_set_sums_to_the_spec_totals` tests in `src/items/armor.rs`:

| Set | Station | Melee | Projectile | Blast | Durability/piece |
|---|---|---:|---:|---:|---:|
| Padded (cloth) | hand | 12% | 10% | 4% | 100 |
| Lamellar (wood slats) | bench t1 | 24% | 20% | 10% | 200 |
| Iron (plate) | bench t2 | 40% | 36% | 20% | 300 |

Armor pieces have durability: a hit a piece protects against wears it by 1, and a piece at 0 stops protecting until re-crafted (repair is craft-cost only, no repair bench). Worn armor is visible on the third-person rig via the replicated `PlayerEquipmentVisual` (four `ArmorMesh` selectors), driven exactly like `PlayerHeldItem`. Equipping rides the existing inventory drag/drop and `InventoryCommand` plumbing through the `ItemContainer::Equipment(slot)` addressing, with slot-type validation (a helmet only enters the head slot) shared by server and client prediction so the paperdoll and the authoritative worn set agree. Iron armor carries no movement penalty for now (there is no stamina meter to trade against).

### Ranged combat (bow and crossbow)

Ranged is fully server-simulated. The client sends `ClientMessage::Ranged(RangedCommand::{DrawStart, DrawCancel, Fire { aim_dir }})`; the server owns draw-time tracking, ammo/possession checks, the ballistic sim, and hit resolution. The bow's damage AND launch speed scale with observed draw ticks (`WOODEN_BOW_DAMAGE_MIN` = 15 to `WOODEN_BOW_DAMAGE_MAX` = 40, speed from `BOW_MIN_RELEASE_SPEED_FRACTION` of full up to `WOODEN_BOW_PROJECTILE_SPEED_MPS`, both over `WOODEN_BOW_DRAW_TICKS`), and a release below `BOW_MIN_DRAW_FRACTION_TO_FIRE` never fires at all: the server drops it before touching ammo, and the client sends `DrawCancel` instead, so a tapped trigger can't loose an arrow. The crossbow is flat `CROSSBOW_DAMAGE` = 55 at full speed, gated behind a ~3.5 s reload (`CROSSBOW_COOLDOWN_TICKS`). Each projectile is a replicated entity (identity + transform components, room-gated, per-entity `ReplicationGroup`); `src/server/projectiles.rs` integrates gravity and sweep-tests blocks, deployables, and player AABBs (the shared `player_body_ray_entry`), then feeds `apply_player_damage` with `DamageKind::Projectile`. The client predicts its own arrow visual (`PredictedArrowEvent`) and reconciles against the replicated projectile. The flat world floor (y = 0) counts as a world solid in that sweep, so an arrow arcing into open ground lodges at the surface exactly like a tree hit. Every world-rest arrow parks as a STUCK projectile (near-zero replicated velocity, its final flight direction kept as an epsilon so clients orient the shaft into the impact) that any player can pull back out with E (`InventoryCommand::RecoverProjectile`, validated for reach server-side) until the stuck TTL despawns it. No separate dropped-item entity spawns, the visible stuck arrow IS the recoverable item; hits consume the arrow outright. Bow movement is slowed while drawn (`BOW_DRAW_MOVE_MULTIPLIER`).

### Explosives and blast damage

Explosions are server-resolved spheres (`src/server/explosion.rs - resolve_explosion`): full `Blast` damage inside the center, linear falloff to the charge's radius, hitting players (self included), building pieces, doors, and deployables. Players take it through the shared tail with the blast armor column; structures take `base_damage * explosive_effectiveness_pct(kind, material) * falloff` through the existing building-damage path (so stability collapse just works). The four charges and their raid math live in [docs/crafting-and-deployables.md](crafting-and-deployables.md); the effectiveness matrix and its raid-cost tests (`src/server/tests/explosives.rs`) are the balance source of truth. A cosmetic `ServerMessage::Explosion { position, kind }` cue fans out within `EXPLOSION_CUE_RANGE_M` = 120 m for VFX/SFX only. The meteor shower reuses the same blast-on-players resolution at its impact.

## Server-authoritative HP and the mandatory Correction

Health is server-authoritative; the client never predicts its own damage. The victim renders their HP bar from their **local prediction**, not from their replicated mirror. So a server path that lowers a player's health must also push a `ServerMessage::Correction(PlayerState)` carrying the new `health` to that player, or their bar silently stays full even as the server records every hit.

The PvP path does this in `src/server/combat.rs - apply_attack_player_command`: after writing `new_health` onto the target's controller it builds a `Correction` with the full controller state and the new health, pushed **before** the `Knockback` envelope so the knockback impulse is applied last on the client and survives even if the correction snaps position on a high-latency link (`apply_non_movement_correction` only snaps past a 1 m divergence; a normal hit just overwrites health). Peers learn the new HP through the replicated player mirror's `health` diff; only the victim needs the `Correction`.

**Rule for any new damage source** (projectiles, fall, fire, anything): every path that lowers a player's HP must send that player a `Correction`. Route through `tool_player_damage` / `damage_after_armor` / `kill_player` and keep the damage server-side. See the comment block around the `Correction` push in `src/server/combat.rs`.

## Wire shapes and reliability

In `src/protocol/messages.rs`, `delivery()` decides the channel:

| Message | Direction | Delivery |
|---|---|---|
| `ClientMessage::AttackPlayer(AttackPlayerCommand { target_player_id })` | client -> server | Reliable |
| `ClientMessage::Ranged(RangedCommand::{DrawStart, DrawCancel, Fire})` | client -> server | Reliable |
| `ClientMessage::Explosive(ExplosiveCommand::{Throw, Defuse})` | client -> server | Reliable |
| `ClientMessage::Respawn` | client -> server | Reliable |
| `ClientMessage::RespawnAtBag { id }` | client -> server | Reliable |
| `ServerMessage::Knockback { impulse }` | server -> victim only | **Reliable** |
| `ServerMessage::PlayerKilled { killer, killer_name, respawn_bags }` | server -> victim only | **Reliable** |
| `ServerMessage::PlayerImpact { attacker, target, position, attacker_position, model, damage_dealt }` | server -> peers (not attacker) | **Unreliable** |
| `ServerMessage::Explosion { position, kind }` | server -> clients in cue range | **Unreliable** |
| `ServerMessage::Correction(PlayerState)` | server -> affected client | **Unreliable** |

The projectiles themselves are not events on this table: they are replicated entities (per-entity mirror + `ReplicationGroup`), following the six replication rules. `Explosion` is a pure cosmetic cue; the authoritative blast already landed via the player/deployable mirrors.

`PlayerImpact` and `Correction` ride the unreliable channel on purpose: impact effects are pure cosmetic feedback (the authoritative damage already lands via the replicated `PlayerHealth` component, and the next swing queues another effect), and a `Correction` is self-superseding (the next one carries fresher state). `Knockback` and `PlayerKilled` are gameplay-affecting and stay reliable. Do not flip these without reading the `delivery()` rationale comments in `messages.rs`.

`PlayerImpact` has **six** fields. `attacker_position` is the attacker's world position at impact, used by the victim client to point an on-screen hit-direction arrow at the source; peers ignore it. HP itself never ships as a dedicated message; it rides the replicated `PlayerHealth(pub f32)` component (`src/server/player_ecs.rs`, registered via `register_component::<PlayerHealth>()`). `PlayerPublic` is not a real symbol; it survives only in stale code comments.

## Death and respawn

`PlayerLifecycle` (in `src/server/player_ecs.rs`) is authoritative: `Alive` (default) or `Dead { since_tick, killer }`, replicated to peers in the chunk room. It drives the remote corpse animation and gates the local owner's input.

### Kill chain (`kill_player`)

When post-armor HP hits zero, `apply_attack_player_command` calls `src/server/combat.rs - kill_player` inline. It:

1. Snapshots the death position.
2. Drains every actionbar slot then every inventory slot into a `Vec<ItemStack>` and spawns **one** loot bag via `spawn_loot_bag` (only if non-empty).
3. Calls `close_sleeper_views(target_id)` so if this body was a sleeper someone had open, a stale `Move` can't reach into the now-dead body.
4. Flips lifecycle to `Dead { since_tick, killer }`, zeroes velocity, clamps health to 0.
5. Returns `PlayerKilled { killer, killer_name, respawn_bags }`, where `respawn_bags` is `respawn_bag_options(account_id)`, the dying player's own placed sleeping bags.

Back in the attack path, after the kill, `consume_active_tool_durability` still runs, so the killing blow lands even if it is also the swing that breaks the tool.

While dead:
- Movement is dropped: `src/server/dispatch.rs` only calls `accept_client_movement` when `client.lifecycle.is_alive()`, so a corpse can't slide.
- The attack handler rejects any swing whose attacker or target is `Dead`.
- The client gates swing/held-item on the local `PlayerLifecycle::Dead` check (`src/app/systems/input/inventory_shortcuts.rs`).

Gameplay never pauses while dead: the death splash and respawn UI gate **controls** only (via `gameplay_accepts_controls`), not simulation. See docs/gameplay-gating.md.

### Respawn

Two server-validated paths, both rejected unless the caller `is_dead()`:

- **`apply_respawn_command`** (random spawn): `pick_safe_spawn(Some(client_id))` then reset controller (health = `MAX_HEALTH` = 100, zero velocity, grounded), clear `next_attack_tick` / `next_gather_tick`, re-anchor chunk membership, flip lifecycle to `Alive`, and send a `Correction` so the predictor snaps onto the new pose.
- **`apply_respawn_at_bag_command`** (sleeping-bag spawn, `src/server/sleeping_bag.rs`): same lifecycle gate; additionally rejected when the bag is gone, belongs to someone else, or is on its respawn cooldown, in which case the client can still pick the random respawn. Fully wired. `PlayerKilled.respawn_bags` feeds one button per bag onto the death screen, each carrying `cooldown_seconds` so a cooling bag renders disabled with a locally-ticking timer.
- **Sleeping-bag respawn cooldown.** A successful bag respawn puts the used bag on a 5-minute cooldown (`SLEEPING_BAG_RESPAWN_COOLDOWN_TICKS`) and shares it with every same-owner bag within `SLEEPING_BAG_SHARED_COOLDOWN_RADIUS_M` (100 m) of it, so a carpet of bags inside a base is one re-entry per fight, not a respawn chain; a genuinely remote bag stays ready. Transient (`GameServer::bag_respawn_cooldowns`, cleared on restart, dropped with the bag). Pinned by the two `bag_*cooldown*` tests in `src/server/tests/sleeping_bag.rs`.

`pick_safe_spawn` (`src/server/combat.rs - pick_safe_spawn`) samples **64 random points across the full `PlayableBounds`** (with a 4 m edge margin, not a ring around origin), rejecting any that overlap a collider (wall, tree, ore, or placed structure) and requiring `>= RESPAWN_MIN_DISTANCE_M` (12 m) from every alive peer (anti spawn-camp). Falls back to the first collider-free sample, then to world origin only if every sample landed in geometry. The **same picker serves fresh joins** (pass `None`), so initial spawn and respawn behave identically.

### Death splash and corpse animation (client)

- `src/app/ui/death_splash.rs`: `BLACK_FADE_SECS` = 4.0 (slow fade to black), then `TITLE_FADE_SECS` = 0.6 (YOU DIED title + "Killed by {name}" subline + Respawn button fade in). `CLOSE_FADE_SECS` = 0.45 fades back out cleanly when the respawn `Correction` lands. `killer_name` lives on the splash state.
- `src/app/systems/players/death_anim.rs - DyingPlayer`: a remote player flipping to `Dead` keeps its visual entity and gains a `DyingPlayer` component driving a kick (`DEATH_UPWARD_KICK_S` = 0.12 s), a feet-pivot fall (`DEATH_FALL_DURATION_S` = 0.65 s, `DEATH_FALL_ANGLE_RAD` = `FRAC_PI_2 + 0.12` rad ~= 96.9 deg), a damped bounce, a hold, then a fade (`DEATH_FADE_DURATION_S` = 0.9 s) via a per-spawn cloned `StandardMaterial { alpha_mode: Blend }`. Not a true ragdoll (the mesh is a single baked mesh, no skeleton); the pivot + roll + bounce read as a collapse. Dead players are filtered out of the peer nameplate overlay.

## Feel and client feedback

- **Swing cadence is gated by the swing animation duration, not `cooldown_ticks`.** `src/app/state/gather.rs - swing_duration_seconds` returns 0.78 s (axe/hammer), 1.60 s (pickaxe), 0.42 s (hands). `cooldown_ticks` (6 stone / 5 iron) is the server's anti-spam floor, not the cadence; `items.rs` explicitly notes the tier upgrade is felt as bigger payouts, not faster swings. The DPS math follows the animation duration, so a stone hatchet is ~8 dmg / 0.78 s.
- **Miss-recovery (whiff penalty):** `COMBAT_MISS_RECOVERY_SECONDS` = 0.25 s. A swing whose impact frame connects with nothing (no player, node, or structure) pays a 0.25 s lockout before the next swing (`src/app/state/gather.rs`, set on `recovery_remaining`); a landed swing rolls straight into the next while LMB is held. Purely client-side cadence; it punishes spam-clicking in PvP. Only **landed** swings consume tool durability; whiffs are free.
- **Hit-direction arrow is implemented.** When the local player is the target, `src/app/systems/network.rs` calls `combat_feedback.push_damage_from(attacker_position)`; `src/app/state/combat_feedback.rs` keeps a `damage_arrows` vec (capped `MAX_DAMAGE_ARROWS` = 6) that the HUD points at the attacker.
- **Hit marker:** `combat_feedback.trigger_hit_marker(is_player)` flashes a crosshair marker, distinct from the floating damage text and the direction arrows.
- **Floating damage text** (`src/app/ui/floating_text.rs`): the attacker predicts an orange `Dealt` number sized by `tool_player_damage`; the victim shows a red `Taken` number; third-party observers show an orange number, no camera kick.
- **Camera kick:** attacker gets `camera_kick.trigger(tool)`; the victim gets a sharper `trigger_from_hit(tool)`.
- **Knockback** (`src/app/state/runtime.rs`, `ServerMessage::Knockback` handler): adds the server-authored impulse to `predicted.velocity` and forces `grounded = false` for a frame so the upward fraction actually carries.
- Chip burst + impact audio reuse the resource-impact plumbing (`ImpactEffectKind::FleshHit` in `src/app/state/gather.rs`, `is_player_hit` routing).

## Loot bags

A kill drops **one** container at the death position holding every stack the corpse carried, so looting is one E-open. `LOOT_BAG_SLOT_COUNT` = `INVENTORY_SLOT_COUNT` (60) + `ACTIONBAR_SLOT_COUNT` (9) = **69** (`src/protocol.rs`). Bags spawn at chest height (`BAG_SPAWN_HEIGHT_M` = 1.0), gravity-settle (`BAG_GRAVITY` = 18.0) to the highest support under their XZ (`BAG_RESTING_Y` = 0.05 above it), and resting bags skip per-tick integration. Interact range is `LOOT_BAG_INTERACT_RANGE_M` = 4.5 m (looser than the 3.5 m melee swing range, `COMBAT_ATTACK_RANGE_M`). There is **no lifetime/expiry despawn**; an empty bag with no one watching is GC'd by `close_container`.

Loot bags are a replicated networked entity (HashMap on `GameServer` + ECS mirror + per-entity `ReplicationGroup`, anchored to a chunk for AoI), following the resource-node/dropped-item pattern. The open/move/quick-transfer command path and the unified `OpenContainer` view (`LootBag` / `Sleeper` / `StorageBox`) are owned by the crafting-and-deployables doc, since storage boxes and sleeping-body loot share the exact same wire path. See docs/crafting-and-deployables.md for the container command path, spill-on-destroy, and the loot-bag UI.

## Extending the model

The damage path is built around a shared post-hit tail (`apply_player_damage`) that does not know which family called it, so a new damage source is a server-side change that reuses it. Mark which pieces are **shipped** vs **aspirational**:

- **Shipped:** melee PvP, the four dedicated weapons (`WeaponProfile` via `AttackProfile`), armor items and per-kind mitigation (`ArmorProtection` + `damage_after_armor` + pierce), ranged (`DamageKind::Projectile` through the sim), explosives and the meteor shower (`DamageKind::Blast`), the hit-direction arrow, sleeping-body targeting, sleeping-bag respawn.
- **Aspirational (not in code today):** `DamageSource` still has only the `Player` variant (projectiles/explosions attribute the attacker through `PlayerDamageHit`, not a new source variant); no healing/bandages, no teams/factions (friendly fire is always on, and explosive self-damage is on), no respawn cooldown (respawn is instant), no combat log/scoreboard, no tier-3 gear. To add a further damage source: build a `PlayerDamageHit` server-side with the right `DamageKind`, route it through `apply_player_damage` (which does armor, `Correction`, knockback, impact fan-out, and `kill_player`), and no wire change is required for the damage itself.

Adding a new melee weapon is now a registry row plus balance constants: give the item a `WeaponProfile` in `src/items/weapons.rs` (damage, reach, cooldown, knockback, pierce) and `resolve_attack_profile` picks it up ahead of any tool. No new match arm in the validation chain. A gather tool still routes through its `ToolProfile`. Remember `cooldown_ticks` is the anti-spam floor, not the swing cadence; `swing_duration_seconds` in `gather.rs` sets the animation pace.

## Tests and tooling

- `src/server/tests/combat.rs`: 23 `#[test]` cases covering the anti-cheat validation chain, death, and respawn.
- LOS unit tests are inline in `src/server/combat.rs`; `player_body_ray_entry` has its own unit tests in `src/combat.rs` (including the point-blank-level-aim regression).
- `/tp` (`"tp"` | `"teleport"`) teleports every other connected player to the issuer and is **admin-only** (`src/server/commands/world.rs - command_teleport_all` returns "admin only" for non-admins); it sends `Correction`s so predictors snap. The moved client plays a teleport whoosh (`SoundId::TeleportWhoosh`, detected in `src/app/systems/network.rs` as an alive-player `Correction` jumping farther than any rubber-band snap; a respawn is filtered out by the open death splash). `./cli multiplayer-test` pre-seeds both test clients as admins and auto-fires `/test-kit`, so two windows can fight without manual setup. See docs/multiplayer-testing.md.

## Related docs

- [docs/crafting-and-deployables.md](crafting-and-deployables.md) - the shared loot-bag / `OpenContainer` wire path, the container UI, and spill-on-destroy.
- [docs/server-authority.md](server-authority.md) - `GameServer` command dispatch and where the combat/loot subsystems sit.
- [docs/replication.md](replication.md) - per-component replication of player HP, lifecycle, and loot bags.
- [docs/movement.md](movement.md) - the client-authoritative predictor that knockback and respawn `Correction`s feed.
- [docs/items-and-resources.md](items-and-resources.md) - `ToolProfile`, per-tier tools, and durability wear.
- [docs/gameplay-gating.md](gameplay-gating.md) - why the death splash gates controls, not simulation.
