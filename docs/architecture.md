---
title: Architecture map and Bevy app wiring
owns: whole-crate module map, the client/server/loopback topology, and the Bevy app assembly in src/app.rs
when_to_read: Before touching app.rs scheduling, adding a ClientSystemSet or a Bevy plugin, or when you need the big picture of how a subsystem connects to the rest.
sources:
  - src/lib.rs - the 23 top-level crate modules
  - src/cli.rs - Command enum, four subcommands, dispatch
  - src/net/client.rs - ClientSession struct (start_singleplayer, connect, connect_inner)
  - src/app.rs - CLIENT_UPDATE_ORDER / CLIENT_MENU_ORDER, configure_client_schedule, add_third_party_plugins, every_system_set_is_ordered_exactly_once
  - src/app/systems.rs - ClientSystemSet enum
  - src/app/systems/input/gating.rs - gameplay_simulation_allowed vs the control gates
related:
  - docs/gameplay-gating.md - the gameplay-never-pauses gate this doc only points at
  - docs/networking.md - transport, channels, handshake that the topology rides on
  - docs/replication.md - the host mirror sync that net/host/ implements
  - docs/server-authority.md - GameServer, the shared authoritative state
  - docs/ui-and-client.md - the egui UI surfaces under app/ui/
  - docs/build-and-dev.md - the ./cli subcommands that invoke this binary
---

# Architecture map and Bevy app wiring

> When to read this: before touching `src/app.rs` scheduling, adding a `ClientSystemSet` or a Bevy plugin, or when you need the big picture of how a subsystem connects to the rest. Source of truth: `src/lib.rs`, `src/cli.rs`, `src/app.rs`, `src/net/client.rs`. Canonical invariants live in CLAUDE.md.

This is the navigation index. It maps the crate, the runtime topology, and the client app assembly, then defers every subsystem's mechanics to its linked doc.

## Topology: one binary, four subcommands

One Rust binary, `ashwend`. `src/cli.rs - Command` dispatches four subcommands (`src/cli.rs - run`):

- `Client { connect }` is the default when no subcommand is given. Runs the Bevy client (`app::run_app`). `--connect <addr>` skips the menu and auto-dials.
- `Server` runs the dedicated Lightyear host.
- `Admin { socket, command }` sends one-shot admin requests (announce, shutdown, set-time) to a running dedicated server over a Unix socket.
- `MultiplayerTest { port, names }` spawns a dedicated server plus two auto-connecting client windows for two-client testing (see [docs/multiplayer-testing.md](multiplayer-testing.md)).

### Singleplayer == multiplayer, the concrete mechanism

`src/net/client.rs - ClientSession` is a plain **struct**, not an enum. There is no `ClientSession::Network` / `Offline` variant anywhere; if you see that phrasing, it is stale. The struct holds a `ClientNetwork` handle plus an optional loopback `GameServerHandle`.

Both play modes converge on the same private `connect_inner`:

- `ClientSession::start_singleplayer` spawns an in-process loopback host (`spawn_loopback_server`) with `AuthMode::NoAuth`, `singleplayer_host: Some(account_id)`, and an `AutoSaveSink` onto the local `WorldStore`, then calls `connect_inner` with the loopback addr and `PrivateKeyContext::Loopback`.
- `ClientSession::connect` calls `connect_inner` with no local server and `PrivateKeyContext::NetworkExposed`.

After that point both paths are identical: send `ClientMessage`, receive `ServerMessage`, consume per-component replicated entity state room-gated to the chunk ring. The shared resource is `ClientNetwork` (the loopback host runs inside the client process, which is also why the client's log filter mutes Lightyear handshake noise; see the `LogPlugin` setup in `add_window_and_default_plugins`). Singleplayer-only code is limited to save selection, loopback lifecycle, local-host admin assignment, and shutdown save. Multiplayer-only code is limited to remote address/discovery, auth mode, dedicated lifecycle, and the admin socket. This split is a hard invariant; see CLAUDE.md.

The dedicated server (`Command::Server`) runs `MinimalPlugins` (no Bevy `LogPlugin`), so `src/cli.rs` installs its own tracing subscriber and crash reporter before anything logs. Do not assume `LogPlugin` exists server-side. The client gets logging via `DefaultPlugins`.

## Module map (all 23)

`src/lib.rs` declares 23 top-level modules. Directories are flagged.

| Module | Role | Doc |
| --- | --- | --- |
| `app` (dir) | The whole Bevy client: scene, input, prediction, session polling, egui UI, audio, voice. Assembled in `src/app.rs`. | [ui-and-client.md](ui-and-client.md) |
| `server` (dir) | Shared authoritative `GameServer` state for loopback and dedicated. `HashMap`s are authoritative; `net/host/` mirrors them into replicated ECS entities. | [server-authority.md](server-authority.md) |
| `net` (dir) | Lightyear client + host adapters; `net.rs` is the module root. | [networking.md](networking.md), [replication.md](replication.md) |
| `protocol` | `ClientMessage` / `ServerMessage` wire variants, replicated component types, `PROTOCOL_VERSION`. | [networking.md](networking.md) |
| `controller` (dir) | Shared client-authoritative movement simulation, collision, block spatial grid. | [movement.md](movement.md) |
| `items` | Item registry, tool profiles, dropped-item helpers. | [items-and-resources.md](items-and-resources.md) |
| `resources` | Resource-node definitions and gather-rule logic. | [items-and-resources.md](items-and-resources.md) |
| `inventory` | Shared inventory model used by client UI and server. | [items-and-resources.md](items-and-resources.md) |
| `crafting` | Recipe queue and crafting domain rules. | [crafting-and-deployables.md](crafting-and-deployables.md) |
| `building` | Base-building taxonomy, socket-snap geometry, costs/HP. | [base-building-and-claims.md](base-building-and-claims.md) |
| `combat` | Shared PvP combat math and validation primitives. | [pvp-combat.md](pvp-combat.md) |
| `game_balance` | Every gameplay tuning constant. New balance values live here, not inline. | [game-design.md](game-design.md) |
| `world` (dir) | `MapType`, perimeter geometry, chunk-based generation under `world/chunk/`. | [worlds-and-saves.md](worlds-and-saves.md) |
| `world_time` | Authoritative day/night clock, broadcast every minute, integrated locally between snapshots. | [worlds-and-saves.md](worlds-and-saves.md) |
| `save` (dir) | `WorldStore`, `WorldSave`, atomic writes, `GAMESAVE` format version. | [worlds-and-saves.md](worlds-and-saves.md) |
| `auth` (dir) | WorkOS identity: JWT verify against JWKS, stable `u64` account id, PKCE login, sealed token store. `AuthMode` is `Workos` or `NoAuth`. | [networking.md](networking.md) |
| `local_crypto` | At-rest obfuscation (deliberately not a security boundary) for local client files: the settings file and the WorkOS refresh-token store. | [networking.md](networking.md) |
| `update` | Boot-time GitHub version check + self-update (`UpdatePlugin`). | [updates-and-distribution.md](updates-and-distribution.md) |
| `analytics` | Optional PostHog client (`AnalyticsPlugin`), client-only, off by default. | [updates-and-distribution.md](updates-and-distribution.md) |
| `console` | Windows dual-subsystem console reattachment (`attach_parent_console`), reattaching the GUI-subsystem binary to the launching shell's console so `server`/`admin`/`multiplayer-test` CLI output is visible. | [build-and-dev.md](build-and-dev.md) |
| `logging` | Shared file-layer install + per-crate noise filter for client and dedicated. | [build-and-dev.md](build-and-dev.md) |
| `util` | Small shared helpers. | (none) |
| `cli` | The four-subcommand entry point and dedicated-world load/backup logic. | [build-and-dev.md](build-and-dev.md) |

### net directory layout

`src/net.rs` is the module root (not a `mod.rs`). Children:

- `client.rs` - `ClientSession`, `ClientNetwork`, `ClientNetworkPlugin`, `client_plugins()`.
- `channels.rs` - `LightyearProtocolPlugin`: channel registration plus every `register_component::<T>()` call.
- `host.rs` + `host/` (dir) - the loopback/dedicated host. The `host/` split is the core of the replication architecture: `mirror.rs` (HashMap -> ECS mirror sync), `rooms.rs` (chunk-room AoI subscription), `routing.rs` (message routing), `handle.rs`, `admin.rs` (Unix admin socket, Unix-only).
- `dedicated/` (dir) - `mod.rs` (CLI-facing host wrapper) and `admin.rs` (the `DedicatedAdminRequest` / `DedicatedAdminResponse` JSON contract).

### app directory layout (the parts you will touch most)

- `app/state/` - client resources split by concern (`MenuState`, dialogs, runtime session state, `LookState`, inventory UI state, toasts, settings, test_mode).
- `app/systems/` - scheduled client systems by concern: `camera`, `input` (dir, owns gating), `network`, `players`, `items` (dir), `deployables` (dir), `effects`, `node_death`, `display`, `graphics`, `world_map`, `auth`, `auto_connect`, `analytics`, `update`, `settings`, `quit`, plus dev-only `agent_window` / `control_socket` / `headless_capture` and feature-gated `replication_trace`. Audio is **not** here.
- `app/audio/` (dir) - the entire audio bus: `mod.rs` plus `ambient`, `music`, `footsteps`, `impact`, `fader`, `transitions`, `scheduled`, `library`, `manifest`, `surface`, `category`. `AudioPlugin` is registered in `src/app.rs`.
- `app/scene/` - first-person scene setup plus three custom client-only render materials: `terrain.rs` (`TerrainMaterial`, biome splat), `toon.rs` (`ToonMaterial`, cel shading), `grass/` (`GrassInstancingPlugin`, one GPU-instanced pipeline). Also `sky.rs`, `assets.rs` (shared `StandardMaterial`s), `mesh/`, `world.rs`. See [art-direction.md](art-direction.md), [toon-shading.md](toon-shading.md), [rendering-materials.md](rendering-materials.md).
- `app/ui/` - all egui surfaces. See [ui-and-client.md](ui-and-client.md).
- `app/voice/` - voice chat (cpal worker threads, Opus). See [voice.md](voice.md).

## Third-party plugin stack

Assembled in `src/app.rs - add_third_party_plugins` (the `DefaultPlugins` + `WindowPlugin` install lives in `add_window_and_default_plugins`). The full client plugin set, in registration order:

- `FrameTimeDiagnosticsPlugin::new(480)` - 480-sample frame-time history; the perf HUD pulls p99/max from it (default 120 is too short to catch periodic stalls).
- `client_plugins()` + `LightyearProtocolPlugin` + `ClientNetworkPlugin` - the Lightyear client stack: tick duration, channel/message/component registration, and connection lifecycle against `ClientNetwork`.
- `EmbeddedAssetsPlugin` - embeds shipped sounds/shaders so no sibling `assets/` folder ships. Must come after `DefaultPlugins` (needs `AssetPlugin`).
- `MaterialPlugin::<TerrainMaterial>` - biome-splat ground material (`shaders/terrain.wgsl`).
- `MaterialPlugin::<ToonMaterial>` - shared cel/toon material for ore + deployables (`shaders/toon.wgsl`).
- `GrassInstancingPlugin` - the one custom GPU-instanced render pipeline (`shaders/grass_instanced.wgsl`).
- `AudioPlugin` - registers `PlaySound`, `SoundLibrary`, `FootstepState`, ambient-zone resource. After `EmbeddedAssetsPlugin`.
- `EguiPlugin` - the UI layer.
- `FramepacePlugin` - software frame pacing (see macOS note below).
- `AnalyticsPlugin` - optional PostHog, client-only, off by default.
- `UpdatePlugin` - boot-time GitHub update check + self-updater, client-only.

Feature-gated additions: `replication-trace` adds `log_replicated_storage_changes_system`; `profile` adds `LogDiagnosticsPlugin` + `EntityCountDiagnosticsPlugin` + `SystemInformationDiagnosticsPlugin`. The dedicated server and admin CLI never load `AnalyticsPlugin` / `UpdatePlugin` / the render materials.

## Schedule mechanism: two arrays, one tripwire

The `Update` schedule order for client systems is built from two flat arrays in `src/app.rs`:

- `CLIENT_UPDATE_ORDER` - 47 sets, the in-game gameplay flow.
- `CLIENT_MENU_ORDER` - 3 sets (`MainMenuMusic`, `MenuBackdropCamera`, `AutoConnect`), an independent menu-phase chain that reads menu state, not snapshots.

`src/app.rs - configure_client_schedule` walks each array with `.windows(2)` and emits one `app.configure_sets(Update, window[1].after(window[0]))` edge per adjacent pair. So the array literal **is** the run order; the declaration order of the `ClientSystemSet` enum in `src/app/systems.rs` is **not**.

Total enum size is 50 variants (47 + 3). `src/app/systems.rs - ClientSystemSet` is the shared vocabulary; systems attach via `.in_set(...)`. `src/app.rs - every_system_set_is_ordered_exactly_once` is the tripwire: a compile-time exhaustive `match` over all variants (so a new variant fails to compile until listed) plus a runtime assert that each variant appears in **exactly one** array and the array lengths sum to the enum size. Skipping the array slot fails the test suite.

### Adding a ClientSystemSet or a system (3 steps)

1. Add the variant to `ClientSystemSet` in `src/app/systems.rs`.
2. Slot it into `CLIENT_UPDATE_ORDER` (or `CLIENT_MENU_ORDER`) in `src/app.rs` at the position matching its **data dependency**, not a side chain. Then add it to the exhaustive `match` and `ALL` list in `every_system_set_is_ordered_exactly_once`.
3. Register the system with `.in_set(ClientSystemSet::Yours)`.

If a system only needs to run after another set without owning a slot, use `.after(ClientSystemSet::Other)` against the **set**, not the function (naming the function can trigger a transitive-cycle panic; `tick_dying_players_system` and the remote-player rig chain do this).

## Non-Update phases

Not all client work lives in the `Update` arrays. `src/app.rs` also registers:

- **Startup**: `setup_scene`, `setup_voice_system`, `setup_item_icons`.
- **PreUpdate**: `install_egui_fonts_system`, ordered `.after(EguiPreUpdateSet::ProcessInput).before(EguiPreUpdateSet::BeginPass)` so the title font is bound before the first layout (running it inside the context pass panics layout for one frame).
- **PostUpdate**: `camera_follow_system.before(TransformSystems::Propagate)`, and `EguiPostUpdateSet::EndPass.before(TransformSystems::Propagate)`. Camera follow runs **only** here, never in `Update`; running it in both phases would double-advance the impact-kick timer and write a stale camera transform that other `Update` systems would read. Respect existing phase choices; they carry load-bearing comments.
- **Last**: `flush_settings_on_exit_system`. In `Last` (not `Update`) because the debounced settings save never fires while the options panel is open (egui marks settings dirty every frame), so quitting from that screen would otherwise drop the change.
- **EguiPrimaryContextPass**: the UI chain `(ui_system, button_sound_system, inventory_sound_system).chain()`.

Other gotchas:

- `WinitSettings::continuous()` (not `desktop_app()`): the menu backdrop camera pans continuously and needs steady frames; reactive update would chop the animation.
- `FramepacePlugin` with `PresentMode::Immediate` everywhere: a CPU-side sleep caps the frame rate because `Fifo` / `AutoVsync` are unreliable on macOS Metal (flicker, no cap respectively). `apply_display_settings_system` keeps the limiter in sync when the user toggles vsync.

## Gating, simulation never pauses

`src/app/systems/input/gating.rs` is the boundary. `gameplay_simulation_allowed(menu)` is `true` whenever `menu.screen == Screen::InGame`, regardless of any overlay. It is a building block, **not** itself a `run_if` gate anywhere; simulation (network ticks, prediction, replication application) runs unconditionally in-game. Only **controls** are gated, through `gameplay_accepts_controls` (look/swing/cursor) and `gameplay_accepts_movement` (WASD), both built on `no_blocking_modal`. The world map is the one overlay that frees the cursor and blocks look/swing but **not** movement.

To make a new overlay freeze input: add its bool to `MenuState` and OR it into `no_blocking_modal`. Never gate a system behind `gameplay_simulation_allowed` to "pause" it. This is a CLAUDE.md invariant; the full model is in [gameplay-gating.md](gameplay-gating.md).

## Dev-only agent-automation surface

`src/app.rs - install_dev_agent_wiring` adds automation hooks. The capture/socket/focus blocks are gated on `cfg(debug_assertions)` (and `unix` / `macos` where relevant), so they compile out of release; a bot cannot drive the shipped game. They cover:

- Off-screen headless capture (`HeadlessCapture`): renders the primary camera into an image, window comes up hidden.
- The `GAME_CONTROL_SOCKET` Unix control socket (`drain_control_socket`) for screenshot/command/state-dump.
- macOS focus relinquish for agent-driven launches.
- Agent-mute: agent-driven runs insert `VoiceDisabled` and a muted `GlobalVolume` (this block runs in every build).

See [headless-agent-testing.md](headless-agent-testing.md).

The auth gate is also bypassed for automation. A normal `client` launch must sign in through WorkOS before the title screen (`AuthFlow::Verifying` / `LoggedOut`). Only the test / `--connect` path (`auto_connect.is_some()`) bypasses it by inserting `CurrentUser(bypass_identity_from_env())` and `AuthFlow::Authenticated` from `GAME_ACCOUNT_ID` / `GAME_PLAYER_NAME`.

## Related docs

- [docs/gameplay-gating.md](gameplay-gating.md) - the gameplay-never-pauses gate and how overlays freeze controls.
- [docs/networking.md](networking.md) - transport, channels, handshake, message inventory.
- [docs/replication.md](replication.md) - per-component replication and the `net/host/` mirror sync.
- [docs/server-authority.md](server-authority.md) - `GameServer` authoritative state and tick subsystems.
- [docs/ui-and-client.md](ui-and-client.md) - the egui UI surfaces under `app/ui/`.
- [docs/build-and-dev.md](build-and-dev.md) - the `./cli` subcommands and dedicated-server load/backup.
- [docs/headless-agent-testing.md](headless-agent-testing.md) - driving the running client for verification.
- [docs/multiplayer-testing.md](multiplayer-testing.md) - the two-client `multiplayer-test` helper.
