# UI And Client Flow

`src/app.rs` wires the Bevy app through named `ClientSystemSet`s. `src/app/ui` draws menus, worlds, HUD, chat, pause, confirmation, inventory, toasts, and multiplayer views.

Screens live in `MenuState`: `MainMenu`, `Worlds`, `Multiplayer`, `Options`, `InGame`. The multiplayer screen supports direct UDP connect through the same `ClientSession` runtime used by singleplayer.

Client resources live in `src/app/state/`:
- `menu.rs`: screen selection and menu flags.
- `dialogs.rs`: confirmation, create-world, edit-world, direct-connect, world-start, and notice dialog data.
- `runtime.rs`: active `ClientSession`, snapshots, local prediction, client log messages, and shutdown task tracking.
- `look.rs`: camera yaw/pitch and sensitivity.
- `backdrop.rs`: menu backdrop fade state.
- `inventory.rs`: inventory UI state, drag state, pickup target, swing impacts, tool-swap animation state.
- `toasts.rs`: queued toasts plus fade/visible timing constants.
- `settings.rs` + `settings/`: persisted client settings (display resolution, present mode, window mode) and the platform-aware `ClientSettingsStore`.

The singleplayer worlds UI lives in `src/app/ui/worlds/`:
- `mod.rs`: screen shell and Escape handling.
- `table.rs`: worlds list layout and row actions.
- `dialogs/`: create/edit world modals and shared form helpers.
- `session.rs`: refresh world list and start singleplayer.

Reusable modal behavior lives in `src/app/ui/modal.rs`. Screen-specific modals should use the modal shell for animation, backdrop, outside-click handling, and Enter confirmation, then keep only form contents and choice mapping in the screen module.

Inventory UI is split by responsibility:
- `src/app/ui/inventory.rs`: inventory/actionbar screen shell.
- `src/app/ui/inventory/slot.rs`: slot rendering, item icons, stack tooltips, and drag start.
- `src/app/ui/inventory/drag.rs`: drag release, move/drop command dispatch, and drag preview.
- `src/app/ui/inventory/pickup.rs`: world-item pickup tooltip.

Multiplayer direct connect is split from the screen shell:
- `src/app/ui/multiplayer.rs`: multiplayer panel, header actions, and Escape behavior.
- `src/app/ui/multiplayer/direct_connect.rs`: direct-connect modal orchestration and background connection attempts.
- `src/app/ui/multiplayer/direct_connect/target.rs`: address/port parsing and DNS/IP resolution.

HUD, chat, and toasts:
- `src/app/ui/hud.rs`: in-game heads-up display, including the actionbar strip and health.
- `src/app/ui/chat.rs`: chat panel, history, and input box; Enter and `T` open it.
- `src/app/ui/toast.rs`: stacked transient notifications with the timing from `state/toasts.rs`.

Theme:
- `src/app/ui/theme/` provides shared egui colors, frames, text helpers, button builders that emit click/hover sound requests, and tooltip styling. Per-screen code should consume these instead of redefining colors or button styles.

Starting singleplayer should only select/load a save and call `ClientSession::start_singleplayer`; the resulting runtime must behave like multiplayer after connection. Do not add UI-side gameplay branches that treat local worlds differently after the session starts.

Input systems live in `src/app/systems/input/`:
- `gating.rs`: rules for when input is allowed (cursor capture, paused, chat open, dialog open).
- `menu_toggles.rs`: Enter/T opens chat, Escape toggles pause, Tab toggles inventory.
- `cursor.rs`: cursor capture and centering on focus.
- `look.rs`: mouse-look integration into `LookState`/`PlayerController`.
- `movement.rs`: WASD/shift/space into predicted `PlayerInput`.
- `inventory_shortcuts.rs`: actionbar number keys and scroll-wheel offset selection.

Scene rendering uses a first-person camera, generated floor/block geometry, and replicated player capsules. Resource-node and held-item meshes live under `src/app/scene/mesh/` (`bag.rs`, `ore.rs`, `trees.rs`, `tools.rs`, `impact.rs`, `builder.rs`). Gameplay camera anti-aliasing must stay non-temporal: use MSAA in gameplay and keep temporal AA/depth-of-field out of the in-game camera. Camera position and rotation must come from the same source each frame — `camera_follow_system` reads `predicted.yaw/pitch` straight from `PlayerController`, and `simulate` integrates the substep loop with that same yaw fixed for the whole frame. Splitting the two (e.g., interpolating yaw across substeps while the camera reads the final value) causes object jitter when strafing while turning.

Audio:
- `assets/main-screen/ambient-music.wav` loops across main-menu, worlds, and multiplayer menu screens.
- Main-menu ambience is managed by `main_menu_music_system` and fades out when the user loads into a world.
- Runtime audio should stay WAV unless there is a specific reason to add another decoder feature; earlier MP3/OGG experiments exposed decoder and seek reliability problems.

UI audio:
- Button click and hover sounds live at `assets/ui/button-click.wav` and `assets/ui/button-hover.wav`.
- `theme::game_button` and `theme::compact_button` record button sound requests while drawing egui widgets.
- Click sounds fire from `Response::clicked()`.
- Hover sounds fire only on hover entry, not every hovered frame.
- `button_sound_system` uses preloaded handles and spawns `PlaybackSettings::DESPAWN` one-shots, so rapid hover/click events can overlap without reusing a paused audio timeline.
- Keep hover SFX subtle and trimmed to the audible transient. Perceptual delay is very noticeable on hover, even when the scheduler is correct.
