# UI And Client Flow

`src/app.rs` wires the Bevy app through named `ClientSystemSet`s. `src/app/ui` draws menus, worlds, HUD, chat, pause, confirmation, inventory, toasts, and multiplayer views.

Screens live in `MenuState`: `MainMenu`, `Worlds`, `Multiplayer`, `Options`, `InGame`. The multiplayer screen supports direct UDP connect through the same `ClientSession` runtime used by singleplayer.

Client resources live in `src/app/state/`:
- `menu.rs`: screen selection and menu flags.
- `dialogs.rs`: confirmation, create-world, edit-world, direct-connect, world-start, and notice dialog data.
- `runtime.rs`: active `ClientSession`, local prediction state, client log messages, and shutdown task tracking. Per-entity authoritative state lives on Lightyear-replicated ECS entities (resource nodes, dropped items, deployables, peer players); the local player's replicated components are mirrored into `state/local_player.rs::LocalPlayerState` once per frame for UI helpers.
- `look.rs`: camera yaw/pitch and sensitivity.
- `backdrop.rs`: menu backdrop fade state.
- `inventory.rs`: inventory UI state, drag state, pickup target, swing impacts, tool-swap animation state.
- `options_ui.rs`: which options tab is selected and any in-flight rebind capture state (cross-frame UI state that shouldn't leak into `MenuState`).
- `toasts.rs`: queued toasts plus fade/visible timing constants.
- `settings.rs` + `settings/`: persisted client settings (display, audio, voice, input, keybindings, HUD flags) and the platform-aware `ClientSettingsStore`. Keybindings serialise as stable string identifiers (not raw `KeyCode` ordinals) so `settings.json` survives a Bevy `KeyCode` reshuffle.
- `test_mode.rs`: `TestModeConfig` + env-var parsing for the `./cli multiplayer-test` helper. Production builds see `TestModeConfig::default()` and the apply-once systems no-op. See [Multiplayer testing](multiplayer-testing.md).

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
- `src/app/ui/hud.rs`: in-game heads-up display, including the actionbar strip, health bar, and the "Voice On" PTT chip (top-center pulsing dot + label drawn from painter primitives so it's font-fallback safe).
- `src/app/ui/chat.rs`: chat panel, history, and input box; Enter and `T` open it.
- `src/app/ui/toast.rs`: stacked transient notifications with the timing from `state/toasts.rs`.
- `src/app/ui/peer_overlay.rs`: floating nameplate + chat bubble per remote player. While a peer is actively transmitting voice, a small pulsing green dot renders immediately to the left of their name (anchored to the rendered text rect so the gap stays consistent across name lengths). The "is talking" flag comes from `VoiceState::is_peer_talking`, which uses a per-peer envelope that decays at 5/sec — that ~200 ms carry-over masks normal between-packet gaps.

Theme:
- `src/app/ui/theme/` provides shared egui colors, frames, text helpers, button builders that emit click/hover sound requests, and tooltip styling. Per-screen code should consume these instead of redefining colors or button styles.

Starting singleplayer should only select/load a save and call `ClientSession::start_singleplayer`; the resulting runtime must behave like multiplayer after connection. Do not add UI-side gameplay branches that treat local worlds differently after the session starts.

Input systems live in `src/app/systems/input/`:
- `gating.rs`: rules for when input is allowed (cursor capture, paused, chat open, dialog open).
- `menu_toggles.rs`: chat-open / inventory-toggle shortcuts (defaults: Enter/T for chat, Tab for inventory). Escape always toggles pause and is intentionally not rebindable.
- `cursor.rs`: cursor capture and centering on focus.
- `look.rs`: mouse-look integration into `LookState`/`PlayerController`.
- `movement.rs`: directional + jump + run input into predicted `PlayerInput`. Reads through `settings.keybindings` so rebound keys take effect immediately.
- `inventory_shortcuts.rs`: actionbar slot keys, scroll-wheel offset selection, drop / pickup / swing. Slot keys are bound through the keybindings system rather than a hardcoded digit table.

Keybindings (`src/app/state/settings/keybindings.rs`):
- One `KeyAction` enum lists every rebindable gameplay action (movement, jump, run, drop, pickup, chat, inventory, push-to-talk, actionbar 1–9).
- Each action has a primary and optional secondary slot, queryable via `KeyBindings::pressed` / `just_pressed`. Input systems should always go through these helpers rather than touch `KeyCode` directly so the rebind UI stays authoritative.
- Defaults live with each action (`KeyAction::default_slots`) and are also used to drive the per-row "Reset" button on the keybindings tab.
- On disk, slots serialise as stable string identifiers (`"KeyW"`, `"ShiftLeft"`, …) — survives a Bevy `KeyCode` reshuffle. Missing actions on load are backfilled with their defaults via `KeyBindings::sanitized`.

Options panel (`src/app/ui/options/`):
- Tabbed shell — General, Display, Audio, Voice, Controls, Keybindings — with a per-tab body module each. Adding a tab is a new variant in `OptionsTab` plus a new branch in `options_body_contents`.
- The keybindings tab uses `egui::Grid` with explicit `min_size` widgets so every cell in a column aligns even when key labels differ in length (`KeyW` vs `ShiftLeft`). Clicking a slot enters capture mode (Escape cancels, right-click clears, any other key binds and clears conflicts).
- The voice tab carries the player-settable voice volumes; the audible range is shown as read-only because it's a game rule, not a preference (see [Voice](voice.md)).

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
