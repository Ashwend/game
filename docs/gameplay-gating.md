---
title: Gameplay-never-pauses and control gating
owns: The control-vs-simulation split in input gating; what each gate blocks and how to add a new overlay without pausing the world.
when_to_read: Before adding any UI overlay, pause/modal, or a system you are tempted to "pause" while a menu is open.
sources:
  - src/app/systems/input/gating.rs - the three gate fns + no_blocking_modal + tests
  - src/app/state/menu.rs - MenuState overlay flags + dialog_modal_open()
  - src/app/systems/input/movement.rs - client_input_system, the only consumer that gates movement separately
  - src/app/systems/world_map.rs - world_map_input_system, the navigable-overlay special case
related:
  - docs/architecture.md - the Update schedule and where gating systems sit
  - docs/ui-and-client.md - the overlay stack and MenuState flag bag
  - docs/pvp-combat.md - why knockback/death must integrate while a menu is open
  - docs/movement.md - the client-authoritative predictor that must keep ticking
---

# Gameplay-never-pauses and control gating

> When to read this: before adding any UI overlay, pause/modal, or a system you are tempted to "pause" while a menu is open. Source of truth: `src/app/systems/input/gating.rs` and `src/app/state/menu.rs`. Canonical invariants live in CLAUDE.md (the "gameplay never pauses" rule).

Ashwend runs an authoritative server (loopback singleplayer or dedicated) at all times. Opening a menu must never stop the world: simulation, local prediction, and network ticks keep running for the whole time the local screen is in-game. Overlays gate only local **input** (movement, look, swing, cursor capture, hotkeys). This doc is the source of truth for that control-vs-simulation split and the recipe for adding a new overlay. The invariant itself is owned by CLAUDE.md; do not restate the rationale there, link to it.

## The three gates

All three live in `src/app/systems/input/gating.rs` and take a `&MenuState` (the per-frame UI flag bag, `src/app/state/menu.rs` - `MenuState`).

### 1. `gameplay_simulation_allowed(menu) -> bool`

`src/app/systems/input/gating.rs` - `gameplay_simulation_allowed`. The whole body is:

```rust
menu.screen == Screen::InGame
```

True whenever the local screen is in-game, regardless of any overlay (pause, inventory, chat, crafting, furnace, loot bag, death splash, world map, anything). It is the floor for the other two gates.

**It is not a Bevy `run_if`.** Grep confirms it is never attached to a system as a run condition; it is used only (a) internally by the other two gates and (b) as an early-return guard inside the per-frame input systems (`client_input_system` at `src/app/systems/input/movement.rs` - the `if !gameplay_simulation_allowed(...) { return; }` at the top, and `world_map_input_system` which checks `menu.screen != Screen::InGame` directly). Simulation systems run unconditionally; this gate exists to short-circuit local-input collection when there is no in-game session to feed, not to pause anything. If you find yourself wanting to `.run_if(gameplay_simulation_allowed)` a gameplay system, stop: that would pause the world. See the add-an-overlay recipe below.

### 2. `gameplay_accepts_controls(menu, window_focused) -> bool`

`src/app/systems/input/gating.rs` - `gameplay_accepts_controls`. True when the local player should accept look, swing, cursor-capture, and gameplay hotkeys. Strictly narrower than simulation:

```
window_focused
  && gameplay_simulation_allowed(menu)   // screen == InGame
  && !menu.world_map_open                 // map frees the cursor for marker clicks
  && no_blocking_modal(menu)
```

Consumers (early-return guards, not run conditions):
- `src/app/systems/input/cursor.rs` - cursor capture (`should_capture`).
- `src/app/systems/input/look.rs` - mouse look.
- `src/app/systems/input/wheel.rs` - the hold-to-open radial wheel.
- `src/app/systems/input/inventory_shortcuts/mod.rs` - inventory/crafting/chat hotkeys.

### 3. `gameplay_accepts_movement(menu, window_focused) -> bool`

`src/app/systems/input/gating.rs` - `gameplay_accepts_movement`. Identical to `gameplay_accepts_controls` **except it does not check `world_map_open`**:

```
window_focused
  && gameplay_simulation_allowed(menu)
  && no_blocking_modal(menu)
```

Sole consumer: `src/app/systems/input/movement.rs` - `client_input_system`, which feeds the WASD direction into the local predictor. This is the only place movement is gated separately from the other controls, because the world map is a navigable overlay (see below).

### The shared core: `no_blocking_modal`

`src/app/systems/input/gating.rs` - `no_blocking_modal`. Both `gameplay_accepts_controls` and `gameplay_accepts_movement` AND-in this. It is true when nothing modal (other than the world map) is in the way:

```
!menu.pause_open
  && !menu.inventory_open
  && !menu.crafting_open
  && !menu.furnace_open
  && !menu.loot_bag_open
  && !menu.chat_open
  && !menu.dialog_modal_open()   // text prompt OR confirmation OR notice
  && menu.death_splash.is_none()
  && !menu.world_entry_splash_active()   // loading splash: world still streaming in
```

`dialog_modal_open()` (`src/app/state/menu.rs` - `MenuState::dialog_modal_open`) is `text_prompt.is_some() || confirmation.is_some() || notice.is_some()`. It is centralized so the keybind and UI open-paths cannot drift. The single-slot text prompts cover door codes, sleeping-bag rename, and world-map marker naming; the death splash freezes controls so a stray click does not drive the player while they pick a respawn point. `world_entry_splash_active()` (`src/app/state/menu.rs`) freezes controls while the world-entry loading splash streams the initial world in: the screen is already `InGame` underneath it (so simulation runs), but no look/swing/movement may leak through the opaque overlay; the held-item viewmodel is hidden for the same window (`src/app/systems/items/held.rs`), and the entity reconcilers switch to their aggressive `*_LOADING` spawn budgets while it is up (frame hitches behind the overlay are invisible).

Note `world_map_open` is deliberately **absent** from `no_blocking_modal`. It is checked only in `gameplay_accepts_controls`, which is what lets the map block look/swing but not movement.

## Why simulation is never gated

The full rationale is in CLAUDE.md (the "Gameplay never pauses" invariant); the short version is that this is an authoritative-server game with client prediction, so server-pushed effects arrive on the network tick whether or not a menu is open. If you halt the simulator behind an overlay, those effects (knockback impulses, replication diffs, death/respawn corrections, world-time advancement) pile up and then fire all at once the moment the menu closes.

The tests in `src/app/systems/input/gating.rs` (the `#[cfg(test)] mod tests` block) pin this for every overlay: each asserts `gameplay_simulation_allowed` stays true while the relevant control gate goes false. The PvP case is called out explicitly in `chat_blocks_controls_without_blocking_simulation`: a knockback impulse arriving while chat is open must integrate into the predictor in real time, or the accumulated velocity discharges when chat closes. See `docs/pvp-combat.md` for the knockback/death path and `docs/movement.md` for the predictor.

This is also why systems that animate or advance state are wired without a `run_if` gate. For example the remote-player rig systems (`reconcile_player_rigs_system`, `apply_remote_player_appearance_system`, `animate_remote_players_system`, registered in `src/app.rs`) carry an inline comment: "No `run_if` gate: gameplay never pauses, remotes keep walking/swinging while a local overlay is open."

## The world map special case

The world map is the one overlay that blocks look/swing but **not** movement. Opening it (`menu.world_map_open`, toggled by `src/app/systems/world_map.rs` - `world_map_input_system`) frees the cursor so the player can click their own map markers, and freezes look/swing through `gameplay_accepts_controls`. But WASD stays live through `gameplay_accepts_movement`, so the player can keep running while checking their coordinates against the map.

The mechanism is exactly the asymmetry between the two control gates: `world_map_open` is checked in `gameplay_accepts_controls` (so look/swing freeze) but is excluded from `no_blocking_modal` (so movement does not freeze). The `world_map_blocks_look_but_not_movement_or_simulation` test asserts all three outcomes at once: simulation true, controls false, movement true.

If a real modal opens **on top of** the map (e.g. the name-a-marker text prompt, or the delete-marker confirm), movement freezes too, because that modal trips `no_blocking_modal` via `dialog_modal_open()`. The keyboard then belongs to the text field, not the player. Tests `a_real_modal_over_the_map_freezes_movement_too` and `an_in_game_confirm_dialog_freezes_controls_and_movement` cover this.

An unfocused window freezes everything, including movement, because every gate AND-s `window_focused`. Focus is resolved by `src/app/systems/input/gating.rs` - `primary_window_focused` (defaults to `true` if the window query is empty).

## Add-an-overlay recipe

To make a new full-screen or blocking overlay freeze input without pausing the world:

1. **Add a `bool` flag to `MenuState`** in `src/app/state/menu.rs` (e.g. `my_overlay_open`), default `false`, and reset it in `MenuState::enter_in_game` if it should clear on session entry. For a single-field text dialog, reuse the existing `text_prompt: Option<TextPrompt>` slot instead; for a yes/no, reuse `confirmation: Option<ConfirmationDialog>`; for a one-button notice, reuse `notice`. Those three already route through `dialog_modal_open()`, so they need no gating change.
2. **OR it into `no_blocking_modal`** in `src/app/systems/input/gating.rs` (add `&& !menu.my_overlay_open`). This freezes look, swing, cursor, hotkeys, and movement while it is open.
3. **Decide the movement exception.** If your overlay should keep WASD live (a navigable overlay like the world map), do NOT add it to `no_blocking_modal`; instead add it only to `gameplay_accepts_controls` alongside the `!menu.world_map_open` check. This is the rare case; default to freezing everything via `no_blocking_modal`.
4. **Never** gate `gameplay_simulation_allowed` (or any gameplay/simulation system's `run_if`) on the new flag. Controls only.
5. **Add a test** to the `tests` module in `gating.rs` mirroring the existing ones: construct a `MenuState` with `screen: InGame` and your flag set, then assert `gameplay_simulation_allowed` is still true while the appropriate control gate is false.

The render side of the overlay (where the egui panel and its open/close keybind live) is owned by `docs/ui-and-client.md`; this doc owns only the gating wiring.

## Related docs

- `docs/architecture.md` - the `Update` schedule, `ClientSystemSet` ordering, and where the input/gating systems sit.
- `docs/ui-and-client.md` - the in-game overlay stack, `MenuState` flag bag, and where each overlay's UI lives.
- `docs/pvp-combat.md` - knockback, death, and respawn: the effects that must keep applying while a menu is open.
- `docs/movement.md` - the client-authoritative predictor that `client_input_system` feeds and that must keep ticking.
