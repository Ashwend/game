---
title: Client UI architecture and flow
owns: The client UI stack: the single egui pass, the boot/login/loading flow, the in-game overlay stack and its draw order, and the shared theme/modal toolkit.
when_to_read: Before adding a screen, overlay, modal, or any egui UI, or to find where a UI surface lives.
sources:
  - src/app/ui.rs - ui_system, UiResources, world_ready_for_play, apply_ui_scale_system, button_sound_system
  - src/app/ui/in_game.rs - in_game_ui (load-bearing draw order)
  - src/app/ui/modal.rs - modal_shell, ModalShellOutput, backdrop_layer
  - src/app/state/menu.rs - Screen, MenuState, enter_in_game
  - src/app/ui/login.rs - login_overlay_ui (WorkOS gate)
  - src/app/ui/theme/buttons.rs - game_button, record_button_sounds, take_button_sounds
  - src/app/state/options_ui.rs - OptionsTab (7 tabs)
related:
  - docs/gameplay-gating.md - the simulation-vs-controls gating helpers every overlay must register with
  - docs/architecture.md - app.rs ClientSystemSet scheduling that runs ui_system
  - docs/networking.md - ClientMessage/ServerMessage the render fns push onto
  - docs/voice.md - the Voice options tab and device-picker bridge
  - docs/worlds-and-saves.md - the dev/test singleplayer worlds screen and save loading
  - docs/updates-and-distribution.md - the changelog/update modals overlaid by ui_system
---

# Client UI architecture and flow

> When to read this: before adding a screen, overlay, modal, or any egui UI, or to find where a UI surface lives. Source of truth: `src/app/ui.rs`, `src/app/ui/in_game.rs`, `src/app/ui/modal.rs`, `src/app/state/menu.rs`. Canonical invariants (gameplay-never-pauses, singleplayer==multiplayer) live in CLAUDE.md.

## One egui pass: ui_system + UiResources

The entire client UI is a single immediate-mode `bevy_egui` system: `ui_system` in `src/app/ui.rs - ui_system`, run once per frame. There is no second egui pass. To add a screen or overlay you wire a render fn into this system (or into `in_game_ui` for in-game overlays); adding a parallel egui system risks double-draw and ordering bugs.

`ui_system` does, in order: grab the primary context, `theme::apply_game_style`, draw the backdrop cover (`theme::backdrop_cover` with a per-screen fade alpha), gate on auth, dispatch on `Screen`, draw global overlays (update pill/modals, confirmation, notice, loading splash), then drain deferred UI sounds.

All UI dependencies funnel through one `UiResources` SystemParam (`src/app/ui.rs - UiResources`), a bundle of ~40 resources, queries, and message writers (menu, runtime, settings, options state, voice, inventory/crafting state, peer/deployable overlay params, local player, prediction, world map, analytics, and more). Render fns receive borrowed slices of this struct, never the whole thing. That is how rendering stays decoupled from ECS wiring: to expose new state to UI you add a field to `UiResources`, and render fns take plain `&`/`&mut` arguments. The one exception is `in_game_ui`, which takes `&mut UiResources` because it dispatches the entire in-game stack.

## Screen is an enum; MenuState is the per-frame flag bag

`Screen` is its own enum, not a set of `MenuState` variants:

```
enum Screen { MainMenu, Options, Worlds, Multiplayer, InGame }   // src/app/state/menu.rs - Screen
```

`MenuState` (`src/app/state/menu.rs - MenuState`) holds `screen: Screen` as one field among ~29. The rest are the per-frame UI flag bag: overlay bools (`pause_open`, `pause_options_open`, `inventory_open`, `crafting_open`, `furnace_open`, `loot_bag_open`, `world_map_open`, `chat_open`), dialog slots (`create_world`, `edit_world`, `direct_connect`, `world_start`, `confirmation`, `notice`, `text_prompt`, `loading_splash`, `death_splash`), and auth-flow request flags (`sign_out_requested`, `cancel_auth_requested`, `force_sign_out`). `inventory_open` and `crafting_open` are kept mutually exclusive by the toggle systems, not by the panel itself.

`ui_system` dispatches on `resources.menu.screen` (`src/app/ui.rs - ui_system`): each arm calls one per-screen render fn (`main_menu_ui`, `worlds_ui`, `options_ui`, `multiplayer_ui`, `in_game_ui`).

`Screen::Worlds` (the singleplayer world picker) is a dev/test-only destination. The Singleplayer main-menu button that routes to it is gated behind `#[cfg(debug_assertions)]` (`src/app/ui/menu.rs - main_menu_ui`), so a shipped release main menu offers only Multiplayer, Options, and Quit. The `Screen::Worlds` variant and `worlds_ui` stay compiled (still reached in dev/test, and by the headless control socket) and fully functional; they are simply unreachable from a release main menu. The singleplayer==multiplayer invariant still holds: the loopback session it starts runs the identical `GameServer` players hit over the network, which is exactly why the dev/test path is worth keeping.

## Hard rule: render fns push outward, never touch the bus directly

Every UI render fn takes plain `&mut` state and pushes work outward. None opens a socket, mutates `GameServer`, or plays a sound inline. The outbound channels are:

- Mutate `MenuState` flags (e.g. flip `menu.screen`, open an overlay bool, stash a dialog).
- Push commands/messages onto `ClientRuntime` (the active session) or write `MessageWriter<ClientErrorToast>` / `MessageWriter<PlaySound>`.
- For session start, call the worlds/multiplayer session helpers (e.g. `worlds_ui` flips `menu.screen` and calls `session::poll_singleplayer_start` in `src/app/ui/worlds/session.rs - poll_singleplayer_start` rather than starting the session inline).

### Deferred UI sound

Render fns must not call the audio bus during the draw pass. UI sound is recorded into egui temp memory and drained after drawing:

- Theme buttons record events via `record_button_sounds` (`src/app/ui/theme/buttons.rs - record_button_sounds`): `ButtonSound::Click` on `response.clicked()`, `ButtonSound::Hover` only on hover-entry (tracked with a persisted per-button bool so it does not fire every hovered frame).
- `ui_system` drains them with `theme::take_button_sounds` into the `ButtonSoundRequests` resource, and `button_sound_system` (`src/app/ui.rs - button_sound_system`) converts each to a `PlaySound` event on the central audio bus.
- The same pattern carries inventory cues: render code pushes onto `InventorySoundRequests`, drained by `inventory_sound_system`. Audio-tab test buttons queue `(delay, sound)` pairs the same way, drained into `ScheduledSounds`.

Use the shared widgets (`theme::game_button` / `theme::compact_button`, the `modal_shell` family). Re-rolling a raw `egui::Button` or a bespoke modal drops the sound and animation contract.

## Boot/client flow: login gate -> enter_in_game -> loading splash

### WorkOS login gate

`ui_system` early-returns before any screen whenever the player is not signed in:

```
if !resources.auth.is_authenticated() {           // src/app/ui.rs - ui_system
    login::login_overlay_ui(...);                 // src/app/ui/login.rs - login_overlay_ui
    return Ok(());
}
```

Until WorkOS sign-in completes the title screen is unreachable; the login splash (and the verifying/authenticating spinner) renders in its place. `CurrentUser` is absent until authenticated, so the menu arms only `.expect()` it after the gate. The `--connect` / test bypass injects an authenticated identity and never inserts the WorkOS config, so it never hits the login branch. Auth-state machinery lives in `src/app/state/auth.rs`; the spinner is advanced by `drive_auth_flow_system`.

### enter_in_game funnel

Both session-start paths (singleplayer loopback, multiplayer join) MUST funnel through `MenuState::enter_in_game` (`src/app/state/menu.rs - enter_in_game`) so the two flows cannot drift in which overlays they reset. It sets `screen = Screen::InGame` and clears `pause_open`, `pause_options_open`, `crafting_open`, `furnace_open`, `loot_bag_open`, `chat_open`, `chat_focus_pending`, `text_prompt`, and `status`. It deliberately does NOT mark the loading splash ready; readiness is gated separately.

### Loading splash held on world_ready_for_play

The world-entry splash (`LoadingSplashKind::EnteringWorld` / `JoiningServer`) holds until the joined world is actually playable. `world_ready_for_play` (`src/app/ui.rs - world_ready_for_play`) requires all four:

1. `runtime.client_id.is_some()` (the `Welcome` arrived).
2. `runtime.world.is_some()` (world data present).
3. `local_player.entity.is_some()` (the local player's replicated entity arrived).
4. `scene_state.applied_live_version() == Some(runtime.world_version)` (scene geometry for that world spawned).

The crossfade reveals a populated, rendered scene rather than a half-streamed one. A throttled (~1/sec) diagnostic in `ui_system` logs which of the four conditions is still missing while a world-entry splash is stuck, written to `<data_dir>/logs/ashwend.log`. The `LoadingSplashKind::Startup` splash (app-launch "Authenticating" warmup) is driven by the menu, not this gate. `loading_splash_ui` sits on top of every screen and modal.

## Unified inventory + crafting panel

Inventory and crafting are ONE tabbed panel, not two modals. The shell is `inventory_panel_ui` in `src/app/ui/inventory_panel.rs`, fixed at `PANEL_WIDTH = 786.0` / `PANEL_HEIGHT = 500.0`. The `inventory_open` / `crafting_open` `MenuState` bools select the active tab and are kept mutually exclusive by the toggle systems. Tab bodies:

- Inventory tab: `src/app/ui/inventory.rs` (slot grid + hotbar) with `inventory/slot.rs` (slot rendering, icons, tooltips, drag start), `inventory/drag.rs` (drag release, move/drop dispatch, drag preview), `inventory/pickup.rs` (world-item pickup tooltip).
- Crafting tab: `src/app/ui/crafting/` (`mod.rs`, `recipes.rs`, `rows.rs`, `filter.rs`).

## Options: 7 tabs rendered in two contexts

```
enum OptionsTab { General, Display, Graphics, Audio, Voice, Controls, Keybindings }   // src/app/state/options_ui.rs - OptionsTab
```

Seven tabs (`OptionsTab::ALL` drives the tab strip). `options_body_contents` (`src/app/ui/options/mod.rs - options_body_contents`) branches per tab; each tab is its own module under `src/app/ui/options/`. Adding a tab is a new `OptionsTab` variant plus a new branch. Cross-frame options state (selected tab, in-flight rebind capture) lives in `OptionsUiState` (`src/app/state/options_ui.rs`), kept off `MenuState` because reopening should restore the tab but reset the capture.

Options renders in two contexts via `OptionsBackTarget` (`src/app/ui/options/mod.rs - OptionsBackTarget`): standalone (`Screen::Options`, back -> MainMenu, dispatched from `ui_system`) and embedded in the pause menu (`menu.pause_options_open`, back -> PauseMenu, dispatched from `in_game_ui`). The Voice tab's device pickers and mic test ride through a `VoiceTabIo` bridge built at each call site (see [docs/voice.md](voice.md)).

## Multiplayer: one confirm-to-join prompt

`multiplayer_ui` (`src/app/ui/multiplayer.rs - multiplayer_ui`) is a single confirm-to-join `modal_shell` for one official server (`JOIN_PROMPT_BODY`). The default address is `DEFAULT_MULTIPLAYER_ADDR = "46.224.101.205:7777"` (`src/app/state/menu.rs - DEFAULT_MULTIPLAYER_ADDR`). A real server browser is not built yet. The direct-connect plumbing still exists (`src/app/ui/multiplayer/connect.rs` orchestration and `src/app/ui/multiplayer/connect/target.rs` host:port + `ToSocketAddrs` DNS parsing) but is not surfaced as a server-browser UI.

## In-game overlay stack and its load-bearing draw order

`in_game_ui` (`src/app/ui/in_game.rs - in_game_ui`) renders the whole `Screen::InGame` stack in a fixed sequence. The egui draw order is significant and the inline comments are the source of truth; preserve the sequence when inserting anything. Drawn order (when `pause_options_open` is false):

1. `hud_ui` (hotbar, health, voice PTT chip), gated by `show_hud`.
2. `peer_overlay_ui` (nameplates, chat bubbles), gated by `world_overlays_visible && show_hud`.
3. `floating_damage_ui`, `deployable_overlay_ui`, `building_cost_overlay`, same gate.
4. `world_map_ui` (when `world_map_open && show_hud`).
5. Tutorial step computed here (before the panel) so the crafting list can pin focused recipes; the step result is stashed in egui temp memory.
6. `inventory_panel_ui` (the unified inventory/crafting panel).
7. `tutorial_ui` overlay (drawn after the panel, because it outlines tab/recipe rects the panel stashed this frame) and `tutorial::completion_banner`.
8. `furnace_ui`, then `loot_bag_ui`.
9. `handle_drag_release` + `draw_drag_preview` run AFTER every slot-drawing surface (inventory, furnace) so a release sees this frame's `hovered_slot`; otherwise an inventory drag with the furnace open releases on a `None` slot and falls through to drop-on-ground.
10. `crafting_queue_hud` (persistent while jobs exist; `show_hud` hides it for screenshots).
11. `chat_ui` (gated by `show_chat`, independent of `show_hud`), `toast_ui`.
12. `wheel_ui` (radial build/hammer/door/bag wheel), then `text_prompt_ui` (drawn above the wheel so a wheel-spawned prompt lands on top).
13. `death_splash_ui` (above world UI, below modal dialogs / loading splash; renders only while `menu.death_splash` is set).

After the else-branch, `pause_ui` draws when `pause_open && !pause_options_open`. When `pause_options_open` is set, the whole stack is replaced by the embedded options panel.

### Visibility gates

- `world_overlays_visible` is false whenever any full-screen modal is up (`inventory_open || crafting_open || furnace_open || loot_bag_open || world_map_open`). It suppresses nameplates, floating damage, and structure labels so they do not poke through panels.
- `show_hud` (`settings.hud.show_hud`) is the master switch for all always-on chrome; `show_chat` (`settings.hud.show_chat`) additionally hides just the chat box. Both are screenshot-clean toggles: neither pauses the game, they only gate what is painted.

### Registering a new overlay

Any new full-screen/blocking overlay must be registered with the control-gating helpers in `src/app/systems/input/gating.rs` so local controls freeze while it is up: add its `*_open` bool to `no_blocking_modal` (or stash it as a dialog slot covered by `MenuState::dialog_modal_open`). NEVER gate `gameplay_simulation_allowed` on it. That helper is `menu.screen == Screen::InGame` and nothing else; gating simulation on an overlay violates the gameplay-never-pauses invariant and causes server-pushed effects (knockback, deaths, replication diffs) to pile up and fire en masse on close. The full split (`gameplay_simulation_allowed` vs `gameplay_accepts_controls` vs `gameplay_accepts_movement`, where the world map is movement-permissive) is documented in [docs/gameplay-gating.md](gameplay-gating.md).

## Shared toolkit: theme and modal shell

### theme

`src/app/ui/theme/` centralizes the look: `apply_game_style` (frames), `backdrop_cover`, color/frame/text helpers, the `game_button` / `compact_button` builders (which record click/hover SFX), and tooltip styling. Per-screen code consumes these rather than redefining colors or button styles.

The custom Cinzel title typeface must be installed before any `theme::title(...)` text lays out, or egui panics rebuilding the atlas. `install_egui_fonts_system` (`src/app/ui.rs - install_egui_fonts_system`) calls `theme::install_title_font` once (latched on a `Local` flag) ahead of `ui_system` so the first frame already has the font. Tests that render title-screen text call `install_title_font` manually (see `src/app/ui/menu.rs:150`).

### modal shell

`modal_shell<T>` (`src/app/ui/modal.rs - modal_shell`) is the animated dialog primitive. It animates open/close with `ctx.animate_bool_with_time` over `MODAL_ANIMATION_SECS = 0.16`s, ease-out-cubic on an 18px slide + 0.94->1.0 scale, behind a backdrop scrim that peaks at alpha 190. It returns `ModalShellOutput { choice, finished_closing, confirm_shortcut_pressed, clicked_outside }`, giving every dialog free open/close animation, outside-click, and Enter confirmation. The `confirmation` / `notice` dialogs (`src/app/ui/confirm.rs`, dispatched as `confirmation_ui` / `notice_ui`) and `multiplayer_ui` are thin callers over this shell; the `backdrop_layer` helper provides the standalone full-screen scrim used under pause/inventory.

### UI scale: never set_zoom_factor

User UI scale goes through `EguiContextSettings::scale_factor`, written by `apply_ui_scale_system` (`src/app/ui.rs - apply_ui_scale_system`) only when it changes, clamped to `MIN_UI_SCALE = 0.75` .. `MAX_UI_SCALE = 1.5`. Do NOT call `ctx.set_zoom_factor()` in the UI pass. The comment in `src/app/ui.rs - ui_system` explains: bevy_egui 0.39 bakes the display scale factor into egui's zoom every frame, so a competing zoom makes the two ping-pong and egui lays the whole UI out in its ~5000x5000 default, rendering centered menus off-screen on HiDPI.

## State modules (src/app/state/)

Persistent client/session/UI state lives in `src/app/state/`. Modules: `auth.rs`, `backdrop.rs`, `combat_feedback.rs`, `connection.rs`, `crafting.rs`, `deployable.rs`, `dialogs.rs`, `gather` + `gather.rs` (swing impacts and tool-swap animation state live here, re-exported from `mod.rs`), `inventory.rs`, `local_player.rs`, `look.rs`, `menu.rs`, `options_ui.rs`, `prediction.rs`, `runtime` + `runtime.rs`, `settings` + `settings.rs`, `test_mode.rs`, `toasts.rs`, `wheel.rs`, `world_map.rs`. The `settings/` submodules are `data.rs` (the bulk: display, audio, voice, input, HUD flags), `display.rs`, `keybindings.rs`, `store.rs`. Settings persist encrypted at rest (`settings.dat`); keybindings serialize as stable string identifiers so the file survives a Bevy `KeyCode` reshuffle. See [docs/worlds-and-saves.md](worlds-and-saves.md) for the save/settings persistence detail.

Input systems live in `src/app/systems/input/`: `gating.rs`, `cursor.rs`, `look.rs`, `movement.rs`, `menu_toggles.rs`, `wheel.rs`, and the `inventory_shortcuts` module. Movement and look read through `settings.keybindings` so rebinds take effect immediately.

## Related docs

- [docs/gameplay-gating.md](gameplay-gating.md) - register every new overlay with the control-gating helpers; do not pause simulation.
- [docs/architecture.md](architecture.md) - `app.rs` `ClientSystemSet` scheduling that runs `ui_system`.
- [docs/networking.md](networking.md) - the `ClientMessage` / `ServerMessage` variants render fns push onto.
- [docs/voice.md](voice.md) - the Voice options tab, mic test, and `VoiceTabIo` device bridge.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - the dev/test singleplayer worlds screen, save loading, and settings persistence.
- [docs/updates-and-distribution.md](updates-and-distribution.md) - the update pill and changelog modals `ui_system` overlays.
