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
  - src/app/ui/workbench.rs - workbench_ui (bench upgrade overlay)
  - src/app/ui/inventory/paperdoll.rs - draw_paperdoll_column (worn-armor slots + character preview)
  - src/app/systems/paperdoll_preview.rs - setup_paperdoll_preview, sync_paperdoll_preview_system (off-screen preview rig/camera)
  - src/app/ui/inventory/drag.rs - handle_drag_release, resolve_quick_equip (drag-to-equip + shift-click)
  - src/app/state/inventory.rs - InventoryUiState (equipment_rects, pending_quick_transfer)
  - src/protocol/items.rs - EquipmentSlot, ItemContainer, ItemContainerSlot
  - src/app/ui/crafting/list.rs - draw_recipe_list (recipe list rows + status dots)
  - src/app/ui/crafting/details.rs - draw_recipe_details, station_met_label, station_requirement (detail card + tier-gate line)
  - src/app/ui/crafting/stations.rs - StationContext, station_satisfied (client station proximity)
  - src/app/ui/hud/meteor_shower.rs - meteor_shower_hud
  - src/app/ui/hud/ranged.rs - ranged_hud, ranged_hud_view
  - src/app/state/ranged.rs - RangedDrawState
  - src/app/systems/input/inventory_shortcuts/ranged.rs - drive_ranged_input
  - src/app/state/wheel.rs - WheelAction::DefuseCharge, PickupHoldKind::Explosive
  - src/app/systems/input/wheel.rs - charge_wheel, DefuseCharge send path
  - src/app/systems/deployables/charge_fuse.rs - spawn_charge_fuse_rig, animate_charge_fuse_system
  - src/app/systems/explosion_vfx.rs - ExplosionEvent, spawn_explosion_burst
  - src/app/systems/network.rs - ServerMessage::Explosion / meteor shower handling
  - src/app/ui/world_map.rs - draw_meteor_shower_marker
  - src/app/scene/meteor_sky.rs - MeteorVisual, MeteorTrailSegment
  - src/app/scene/meteor_shower.rs - MeteorShowerCrater, update_meteor_shower_ground_system
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

The world-entry splash (`LoadingSplashKind::EnteringWorld` / `JoiningServer`) holds until the joined world is actually playable. `world_ready_for_play` (`src/app/ui.rs - world_ready_for_play`) requires all six:

1. `runtime.client_id.is_some()` (the `Welcome` arrived).
2. `runtime.world.is_some()` (world data present).
3. `local_player.entity.is_some()` (the local player's replicated entity arrived).
4. `scene_state.applied_live_version() == Some(runtime.world_version)` (scene geometry for that world spawned).
5. `entity_spawn_queues_drained` (`src/app/ui.rs - entity_spawn_queues_drained`): every budgeted entity-spawn queue has drained its initial backlog. Each reconciler exposes an `is_caught_up()` accessor: `ResourceNodeEntities` and `DeployedEntityVisuals` (first connected pass ran + `pending_spawns` empty), `DroppedItemEntities` (last pass spawned everything it saw without exhausting the budget), and `GrassState` (a full streaming scan completed at the current camera tile with no fill pending; skipped when grass density is `Off`).
6. `initial_stream_settled`: the server's initial replication stream has gone quiet. The client-side queues alone are NOT enough: the host budgets its own mirror spawns per sync tick (`MAX_RESOURCE_NODE_SPAWNS_PER_SYNC` in `src/net/host/mirror.rs`) and Lightyear paces delivery on top, so the queues can drain to empty while more of the world is still on the wire. `WorldStreamState` (`src/app/state/world_stream.rs`) records the most recent replicated-entity arrival (every reconciler reports its per-frame arrivals: nodes, deployables, dropped items, loot bags); the stream counts as settled after `STREAM_QUIET_SECS` (1 s) without a single arrival, or `STREAM_START_GRACE_SECS` (2 s) after connect if nothing ever arrives.

The crossfade reveals a fully populated, rendered scene, not one still streaming in around the player. The readiness condition must additionally hold for `WORLD_ENTRY_SETTLE_FRAMES` (12) consecutive frames before the splash fades (`src/app/state/dialogs.rs - note_world_ready`), and the 20 s `WORLD_ENTRY_READY_TIMEOUT_SECONDS` valve still guarantees nobody is stranded. While the gate holds, the splash shows a STEADY status line, "Placing N objects…" (fed by `entity_spawn_backlog`, nodes + deployables) or "Settling the world…" when the queues are momentarily empty; the line renders every not-ready frame because keying its visibility on `backlog > 0` made it blink as the queue emptied and refilled between packets. The splash also locks the world down while it is up: `world_entry_splash_active()` freezes controls/movement (see [gameplay-gating.md](gameplay-gating.md)), hides the held viewmodel (it composites after egui and would float on the overlay), and switches the node/deployable reconcilers to their `*_LOADING` spawn budgets (64/frame vs 8/16) since hitches behind the opaque overlay are invisible. A throttled (~1/sec) diagnostic in `ui_system` logs which of the conditions is still missing (including seconds since the last replicated arrival) while a world-entry splash is stuck, written to `<data_dir>/logs/ashwend.log`. The `LoadingSplashKind::Startup` splash (app-launch "Authenticating" warmup) is driven by the menu, not this gate. `loading_splash_ui` sits on top of every screen and modal.

## Unified inventory + crafting panel

Inventory and crafting are ONE tabbed panel, not two modals. The shell is `inventory_panel_ui` in `src/app/ui/inventory_panel.rs`, fixed at `PANEL_WIDTH = 1018.0` / `PANEL_HEIGHT = 500.0` (sized as paperdoll column + gap + 12-column bag grid + margins; a width test pins the arithmetic). The `inventory_open` / `crafting_open` `MenuState` bools select the active tab and are kept mutually exclusive by the toggle systems. Tab bodies:

- Inventory tab: `src/app/ui/inventory.rs` (slot grid + hotbar) with `inventory/slot.rs` (slot rendering, icons, tooltips, drag start), `inventory/drag.rs` (drag release, move/drop dispatch, drag preview), `inventory/pickup.rs` (world-item pickup tooltip).
- Crafting tab: root `src/app/ui/crafting.rs` + `src/app/ui/crafting/` (`recipes.rs`, `filter.rs`, `list.rs`, `details.rs`, `icon.rs`), a master/detail browser: the left column is the searchable, category-filtered recipe list (real item icons, a craftable-status dot per row, click to select), the right column is a detail card for the selected recipe (description, per-ingredient have/need lines, batch quantity stepper, Craft button). `CraftingUiState::selected_recipe` stores the selection; a hidden or unset id falls back to the top visible entry (`effective_selection_index` in `crafting.rs`), so the card is never empty. The Max button fills the quantity field with the largest affordable batch rather than enqueueing instantly.
- Admin tab (admins only): `src/app/ui/admin_items.rs`, a scrollable grid of every obtainable item where a click grants it through the server's `/give` command path (left / right click for small / large amounts, scaled to the item's stack size). It is a pure VIEW of the inventory-open state: `inventory_open` stays the panel's source of truth and `InventoryUiState::admin_tab` selects the body, so every overlay/control gate works unchanged. Non-admins never see the tab, and the server re-validates every grant.

## Options: 7 tabs rendered in two contexts

```
enum OptionsTab { General, Display, Graphics, Audio, Voice, Controls, Keybindings }   // src/app/state/options_ui.rs - OptionsTab
```

Seven tabs (`OptionsTab::ALL` drives the tab strip). `options_body_contents` (`src/app/ui/options.rs - options_body_contents`) branches per tab; each tab is its own module under `src/app/ui/options/`. Adding a tab is a new `OptionsTab` variant plus a new branch. Cross-frame options state (selected tab, in-flight rebind capture) lives in `OptionsUiState` (`src/app/state/options_ui.rs`), kept off `MenuState` because reopening should restore the tab but reset the capture.

Options renders in two contexts via `OptionsBackTarget` (`src/app/ui/options.rs - OptionsBackTarget`): standalone (`Screen::Options`, back -> MainMenu, dispatched from `ui_system`) and embedded in the pause menu (`menu.pause_options_open`, back -> PauseMenu, dispatched from `in_game_ui`). The Voice tab's device pickers and mic test ride through a `VoiceTabIo` bridge built at each call site (see [docs/voice.md](voice.md)).

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
8. `furnace_ui`, then `workbench_ui`, then `loot_bag_ui` (the "press E on a station" family; see [combat and event surfaces](#combat-and-event-surfaces) for the workbench).
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

## Combat and event surfaces

These surfaces add client UI across the in-game stack: a workbench upgrade overlay, worn-armor and ranged-weapon HUD, an explosives defuse wheel action, and the meteor shower event's HUD/map/scene chrome. All of it obeys the two invariants: nothing pauses simulation, and every gameplay path runs through `GameServer`. The render fns push commands outward exactly like the rest of the UI; the sections below note where each surface lives in the draw order and which state it reads.

### Workbench upgrade overlay

`workbench_ui` (`src/app/ui/workbench.rs - workbench_ui`) is the "press E on a bench" station overlay, drawn right after `furnace_ui` and before `loot_bag_ui` in `in_game_ui` (`src/app/ui/in_game.rs`). Unlike the furnace it has no item slots: it shows the bench's current tier, a one-sentence unlock blurb per tier, and, when the shared upgrade table lists a next tier, a cost list plus an Upgrade button.

- **Source of truth is replicated state, not a `ServerMessage`.** The overlay opens off `PlayerPrivate.open_workbench`, an `OpenWorkbenchView { id, tier }` pointer on the local player's private replication; absent means no bench is open. `sync_workbench_open_flag_system` (`src/app/systems/input/menu_toggles.rs - sync_workbench_open_flag_system`) mirrors that presence into `MenuState::workbench_open` so the control-gating and Escape paths key off a plain bool.
- **Costs never travel the wire.** They come from the compile-time upgrade table (`crate::items::upgrade_for`), keyed by the bench's current `DeployableKind::Workbench { tier }`. Affordability is checked against the client's own replicated inventory; the Upgrade button disables when short, but that is only a courtesy since the server re-validates on `WorkbenchCommand::Upgrade`. A bench at its ceiling (no table row) shows a quiet "Fully upgraded." line instead.
- Clicking outside the panel or the Close button sends `WorkbenchCommand::Close`; Escape closes it through `close_workbench_on_escape_system` (`src/app/systems/input/menu_toggles.rs - close_workbench_on_escape_system`). Like the other station modals it gates controls via `workbench_open` in `src/app/systems/input/gating.rs`, never simulation.

### Crafting tier gates

The crafting browser marks recipes gated behind `RecipeStation::Workbench { min_tier }`. This is not a static disabled state but a live proximity check: the detail card's category/time meta line appends the station requirement, and the line's colour, the list row's status dot, and the Craft button's enablement all track whether a satisfying bench is currently in range.

- `StationContext` (`src/app/ui/crafting/stations.rs - StationContext`) resolves once per panel render from the replicated `(Deployable, DeployableTransform)` set already in the player's AoI. `station_satisfied` mirrors the server's `station_in_range` loop exactly (same `RecipeStation::satisfied_by` and the deployable profile's `station_radius`), so the UI never green-lights a craft the server then rejects. A pre-Welcome unknown position reads every workbench requirement as unmet, the safe default.
- On the list (`src/app/ui/crafting/list.rs - draw_recipe_list`): each row carries a status dot, green when craftable now, amber when blocked only by a station, dim when materials are missing; a blocked row's hover tooltip names which.
- On the detail card (`src/app/ui/crafting/details.rs - draw_recipe_details`): when a qualifying bench is in range, `meta_layout_job` appends a subdued "Workbench Tier N" chunk (`station_met_label`) and the Craft button is live. When it is not, it appends a red "Requires Workbench Tier N" chunk (`station_requirement`, in `BLOCKED_COLOR`) and the button greys to a "No bench" secondary with the same requirement in its hover tooltip. Hand recipes (`RecipeStation::None`) append nothing. The station gate outranks the materials check, so an unmet bench reads as "reach a bench," not "gather more."

### Paperdoll: the worn-armor slots and character preview

The Inventory tab's left column, drawn by `draw_paperdoll_column` (`src/app/ui/inventory/paperdoll.rs - draw_paperdoll_column`), is a four-slot equipment stack (Head, Chest, Legs, Feet) beside a live 3D preview of the character wearing the equipped set and holding the active item. It reuses the shared `draw_slot` widget, so a piece dragged onto a paperdoll slot rides the identical unified drag pipeline as a bag-to-bag move.

- **Character preview.** `src/app/systems/paperdoll_preview.rs` spawns a dedicated copy of the player rig far below the world on render layer 2 (0 = world, 1 = first-person viewmodel) with its own fixed studio lights and an off-screen camera rendering to an `Image` registered with `bevy_egui` (`paperdoll_preview_texture`, a write-once `OnceLock` mirroring the item-icon registry). `sync_paperdoll_preview_system` activates the camera only while the plain Inventory tab is showing (zero cost otherwise), applies a gentle idle yaw sway, and rebuilds the worn-armor / held-item layers on change using the SAME derivations as remote players (`PlayerEquipmentVisual::from_equipment_slots` over the local predicted inventory, `armor_layers`, `held_item_layers` + the carry-pose arm rotations), so what the preview shows is exactly what peers see. Every spawned part/layer carries `RenderLayers::layer(2)` explicitly; render layers do not propagate to children.

- **Addressing.** Equipment slots are `UnifiedSlotRef::Player(ItemContainerSlot { container: ItemContainer::Equipment, slot })` where `slot` is `EquipmentSlot::index()` (`src/protocol/items.rs - ItemContainer`, `EquipmentSlot`). `ItemContainer::Equipment` is a unit variant, not a tuple; `ItemContainerSlot::equipment(EquipmentSlot)` builds the ref. The shared move validation (armor-only, slot-matched, swap-never-merge) silently rejects an invalid drop, so no paperdoll-specific gating is needed.
- **Drop targets.** The column registers each slot's rect into `InventoryUiState::equipment_rects` (`src/app/state/inventory.rs - InventoryUiState`), indexed by `EquipmentSlot::index`, reset each frame in `begin_frame`. `pointer_is_outside_inventory_surfaces` (`src/app/ui/inventory/drag.rs`) consults those rects so a release over a paperdoll slot counts as landing on an inventory surface rather than dropping on the ground.
- **Two ways to equip.** A drag onto a slot goes through `handle_drag_release`; a Shift+click on a bag/actionbar armor piece records a `pending_quick_transfer` intent that `resolve_quick_equip` (`src/app/ui/inventory/drag.rs - resolve_quick_equip`) turns into a predicted equip `InventoryCommand::Move` to the piece's matching slot (resolved from its `armor_profile`). A non-armor piece leaves the intent unresolved and the click is a no-op. Both paths predict the empty-destination case optimistically.
- **Protection readout.** Below the slots, `draw_protection_summary` shows a per-kind Melee / Ranged / Blast mitigation summary computed client-side via the shared `equipped_protection` over the worn slots, so the numbers match the server's mitigation. Note: the replicated `PlayerArmor` component exists (fed from the melee protection value) and rides per-player replication, but the in-game HUD does not paint it; the paperdoll's summary is where a player reads their protection.

### Ranged weapon HUD and feel

A held bow or crossbow swaps the melee swing loop for the draw/fire/reload state machine in `RangedDrawState` (`src/app/state/ranged.rs`), driven by `drive_ranged_input` (`src/app/systems/input/inventory_shortcuts/ranged.rs - drive_ranged_input`). A press begins a draw and a release fires (`RangedCommand::DrawStart` / `Fire`); a crossbow fires the instant it is pressed off cooldown and dry-clicks otherwise; item swaps and overlays cancel an in-flight draw with `DrawCancel`. The reload clock keeps burning while an overlay is up (the server's cooldown never pauses), so the local mirror does not fall behind.

- **HUD.** `ranged_hud` (`src/app/ui/hud/ranged.rs - ranged_hud`) paints a quiet crosshair-adjacent readout whenever the active item resolves a ranged profile (`ranged_hud_view`): a small ammo count down-right of center (warming to red when the quiver is empty) plus a thin arc that fills clockwise from 12 o'clock with the draw fraction (bow, brightening toward full) or the reload progress (crossbow, cooler and dimmer). It is silent for melee, tools, and bare hands. It is a HUD element, not just viewmodel plus audio.
- **Feel.** Full draw pinches the world camera FOV (`advance_ranged_pinch` in `src/app/systems/camera/follow.rs`), and `sync_viewmodel_fov_system` (`src/app/systems/camera/viewmodel_fov.rs`) applies a proportional pinch to the viewmodel camera so the held bow tracks the world pinch. Audio rides straight off the draw state: a draw creak retriggers as tension ramps, the release plays a thunk plus arrow whoosh (bow) or a heavier fire thunk (crossbow), and the crossbow reload ratchet clicks are scheduled across the reload window as its fraction crosses each threshold.

### Explosives: defuse wheel, fuse VFX, and blast feedback

Placed explosive charges add a defuse action and cosmetic feedback, none of it a new UI panel.

- **Defuse wheel.** A live charge has no useful tap action, so `PickupHoldKind::Explosive` (`src/app/state/wheel.rs - PickupHoldKind`) opens a one-option hold-E wheel (`charge_wheel` in `src/app/systems/input/wheel.rs`) whose `WheelAction::DefuseCharge(id)` sends `ClientMessage::Explosive(ExplosiveCommand::Defuse { id })`. The server gates on reach plus claim authorization and refunds half the materials. Like every wheel it freezes the camera and suppresses swings but never touches `gameplay_accepts_controls`.
- **Fuse VFX/SFX.** Every `DeployableKind::Explosive` charge is always armed while it exists, so the deployable reconciler attaches a fuse rig (`spawn_charge_fuse_rig` in `src/app/systems/deployables/charge_fuse.rs`) at spawn and it tears down with the charge. Within `CHARGE_FUSE_NEAR_M` the rig sheds sparks at the kind's fuse-tip anchor and re-fires a spatial fuse hiss (`SoundId::FuseHiss`, a real sizzle recording) on a fixed cadence so a defender can hear a live charge. The thrown bomb's projectile visual carries the same rig, so a lit bomb sparks and hisses through its flight and roll. Far charges still render but pay for no particles or audio.
- **Detonation feedback.** `ServerMessage::Explosion { position, kind }`, handled in `src/app/systems/network.rs`, drives purely cosmetic feedback (the authoritative damage/destruction already landed via mirrors): it raises `ExplosionEvent` for the flash + debris + smoke burst (`spawn_explosion_burst` in `src/app/systems/explosion_vfx.rs`, scaled by charge kind), a proximity-scaled camera shake (`trigger_from_explosion`), a low thump at the blast, and a distance-delayed far rumble. The server only fans the cue to clients inside its cue range, so receiving it means the local player witnessed the blast.

### Meteor shower: countdown HUD, map marker, and sky/ground VFX

The meteor shower event is announced by `ServerMessage::MeteorShower` and stored on `runtime.meteor_shower`; all of its chrome is computed client-side from that payload plus the authoritative clock estimate, so it costs nothing on the wire and cannot desync.

- **HUD.** `meteor_shower_hud` (`src/app/ui/hud/meteor_shower.rs - meteor_shower_hud`) draws a CENTER_TOP countdown pill (`format_meteor_shower_countdown`: `M:SS`, switching to "Impact imminent" under 30 s) that reddens and pulses as impact nears, and, only while the player's own position is inside `METEOR_SHOWER_DANGER_RADIUS_M` of the impact point, an escalating "Evacuate the area" warning (`meteor_shower_danger_intensity` ramps it over the final 60 s). Both go quiet after impact, when the crater visual takes over.
- **World map.** While the event is live, `draw_meteor_shower_marker` (`src/app/ui/world_map.rs - draw_meteor_shower_marker`) draws a temporary pulsing ember pip at the impact position, sourced from the live event state (not the per-account marker store), so a player can navigate to or away from the strike. It vanishes when the event cleans up.
- **Scene VFX.** A `MeteorVisual` fireball with a `MeteorTrailSegment` streak (`src/app/scene/meteor_sky.rs - MeteorVisual`) rides the sky dome on approach, evaluated against the FRACTIONAL clock estimate (`ClientRuntime::server_tick_precise`): truncating to whole 20 Hz ticks quantises the plunge into 50 ms steps that stutter at render frame rates. At impact `update_meteor_shower_ground_system` (`src/app/scene/meteor_shower.rs - update_meteor_shower_ground_system`) spawns an `MeteorShowerCrater` rig: the dug-in crater as two meshes sharing a seam ring (`build_crater_mesh`: a SOLID opaque bowl+rim body plus a translucent vertex-alpha burn skirt fading out over the grass), with geometry sampled from the shared `crate::world::crater_surface_height` profile that also drives the movement collider's analytic floor (players walk over the mound, see docs/movement.md) and the server's shard-node seating, plus scattered `MeteorShowerSiteFire` particle-fire emitters (furnace-style flame puffs + embers with flickering lights, driven by `animate_meteor_shower_site_fire_system`) that burn for `METEOR_SHOWER_SITE_FIRE_SECONDS`, fade out, and despawn, leaving the crater for the rest of the window. The one-time rock blast at the strike is fixed-size matte `ImpactChip` physics debris on seeded `irregular_rock_mesh` variants; its tumble bleeds off with ground friction and bounces so settled chunks lie still, and everything self-despawns. The module also owns the strike cues: the impact boom is pre-armed `IMPACT_BOOM_LEAD_S` early so the file's baked lead-in ends on the impact frame, the flyby bed starts at a fixed `FLYBY_LEAD_S` so its silent tail lands on the strike (fade-out, never a mid-waveform cut), and the strike fires a distance-scaled `CameraImpactKick::trigger_meteor_impact`. Entirely client-side and derived from the event state with no replicated crater entity or save bump. A late joiner who connects during the crater phase gets the resent announce and draws the site too (fires only while the burn window is open, never the blast replay).

## Shared toolkit: theme and modal shell

### theme

`src/app/ui/theme/` centralizes the look: `apply_game_style` (frames), `backdrop_cover`, color/frame/text helpers, the `game_button` / `compact_button` builders (which record click/hover SFX), and tooltip styling. Per-screen code consumes these rather than redefining colors or button styles.

The custom Cinzel title typeface must be installed before any `theme::title(...)` text lays out, or egui panics rebuilding the atlas. `install_egui_fonts_system` (`src/app/ui.rs - install_egui_fonts_system`) calls `theme::install_title_font` once (latched on a `Local` flag) ahead of `ui_system` so the first frame already has the font. Tests that render title-screen text call `install_title_font` manually (see `src/app/ui/menu.rs:150`).

### modal shell

`modal_shell<T>` (`src/app/ui/modal.rs - modal_shell`) is the animated dialog primitive. It animates open/close with `ctx.animate_bool_with_time` over `MODAL_ANIMATION_SECS = 0.16`s, ease-out-cubic on an 18px slide + 0.94->1.0 scale, behind a backdrop scrim that peaks at alpha 190. It returns `ModalShellOutput { choice, finished_closing, confirm_shortcut_pressed, clicked_outside }`, giving every dialog free open/close animation, outside-click, and Enter confirmation. The `confirmation` / `notice` dialogs (`src/app/ui/confirm.rs`, dispatched as `confirmation_ui` / `notice_ui`) and `multiplayer_ui` are thin callers over this shell; the `backdrop_layer` helper provides the standalone full-screen scrim used under pause/inventory.

### UI scale: never set_zoom_factor

User UI scale goes through `EguiContextSettings::scale_factor`, written by `apply_ui_scale_system` (`src/app/ui.rs - apply_ui_scale_system`) only when it changes, clamped to `MIN_UI_SCALE = 0.75` .. `MAX_UI_SCALE = 1.5`. Do NOT call `ctx.set_zoom_factor()` in the UI pass. The comment in `src/app/ui.rs - ui_system` explains: bevy_egui 0.39 bakes the display scale factor into egui's zoom every frame, so a competing zoom makes the two ping-pong and egui lays the whole UI out in its ~5000x5000 default, rendering centered menus off-screen on HiDPI.

## State modules (src/app/state/)

Persistent client/session/UI state lives in `src/app/state/`. Modules: `auth.rs`, `backdrop.rs`, `combat_feedback.rs`, `connection.rs`, `crafting.rs`, `deployable.rs`, `dialogs.rs`, `gather` + `gather.rs` (swing impacts and tool-swap animation state live here, re-exported from the `state.rs` root), `inventory.rs`, `local_player.rs`, `look.rs`, `menu.rs`, `options_ui.rs`, `prediction.rs`, `runtime` + `runtime.rs`, `settings` + `settings.rs`, `test_mode.rs`, `toasts.rs`, `wheel.rs`, `world_map.rs`. The `settings/` submodules are `data.rs` (the bulk: display, audio, voice, input, HUD flags), `display.rs`, `keybindings.rs`, `store.rs`. Settings persist encrypted at rest (`settings.dat`); keybindings serialize as stable string identifiers so the file survives a Bevy `KeyCode` reshuffle. See [docs/worlds-and-saves.md](worlds-and-saves.md) for the save/settings persistence detail.

Input systems live in `src/app/systems/input/`: `gating.rs`, `cursor.rs`, `look.rs`, `movement.rs`, `menu_toggles.rs`, `wheel.rs`, and the `inventory_shortcuts` module. Movement and look read through `settings.keybindings` so rebinds take effect immediately.

## Related docs

- [docs/gameplay-gating.md](gameplay-gating.md) - register every new overlay with the control-gating helpers; do not pause simulation.
- [docs/architecture.md](architecture.md) - `app.rs` `ClientSystemSet` scheduling that runs `ui_system`.
- [docs/networking.md](networking.md) - the `ClientMessage` / `ServerMessage` variants render fns push onto.
- [docs/voice.md](voice.md) - the Voice options tab, mic test, and `VoiceTabIo` device bridge.
- [docs/worlds-and-saves.md](worlds-and-saves.md) - the dev/test singleplayer worlds screen, save loading, and settings persistence.
- [docs/updates-and-distribution.md](updates-and-distribution.md) - the update pill and changelog modals `ui_system` overlays.
