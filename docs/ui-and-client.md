# UI And Client Flow

`src/app.rs` wires the Bevy app. `src/app/ui` draws menus, worlds, HUD, pause, chat, confirmation, and multiplayer views.

Screens live in `MenuState`: `MainMenu`, `Worlds`, `Multiplayer`, `InGame`. Multiplayer UI exists, but the main-menu entry is gated as coming soon.

Input systems:
- Enter/T opens chat.
- Escape toggles pause.
- In-game cursor capture drives mouse look.
- WASD, shift, and space feed predicted movement.

Scene rendering uses a first-person camera, generated floor/block geometry, and replicated player capsules.
