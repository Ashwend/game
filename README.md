# Game

[![Quality Gate](https://github.com/danniehansen/game/actions/workflows/quality-gate.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/quality-gate.yml)
[![Coverage Gate](https://github.com/danniehansen/game/actions/workflows/coverage.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/coverage.yml)
[![Dependency Audit](https://github.com/danniehansen/game/actions/workflows/audit.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/audit.yml)

A Rust + Bevy first-person prototype. One authoritative server, one shared
gameplay path, two ways in: loopback singleplayer and direct UDP multiplayer.

## What you can do today

- Drop into a world from the main menu, or host one and join from another
  client. Both paths run the same simulation through the same `GameServer`.
- Move around with a responsive client-predicted first-person controller —
  walk, sprint, jump, coyote-time, step-up on low ledges, AABB block
  collision against the world geometry.
- Gather resources. Stone hatchets chop pine, birch, and dead trees (three
  size variants each) for wood; stone pickaxes mine coal, iron, and sulfur
  ore nodes. Tools have tiers, cooldowns, and per-swing yields; the world
  remembers which nodes are spent.
- Carry it home. A 40-slot inventory plus a 9-slot actionbar with hotkey
  switching, scroll-wheel cycling, click-drag and split-stack moves,
  drop-on-floor, line-of-sight pickup tooltips, and a held-item swap
  animation when you change tools.
- Talk to other players. Enter or `T` opens chat; messages run through the
  same reliable channel as auth and inventory commands.
- See feedback. Toasts surface pickup totals, gather events, and other
  server-driven notifications; the HUD shows health and the actionbar.
- Save and resume. Each world is a compressed binary save that captures
  player positions, inventories, dropped items, resource node state, and
  admin lists. Disconnecting writes a snapshot; a returning player picks
  up where they left off.
- Run a dedicated server with optional auth mode, custom world file, and
  a Unix admin socket for announcements and graceful shutdown from the CLI.

## Tech

- **Bevy 0.18** for the client app, scene rendering, ECS scheduling, and
  windowing.
- **bevy_egui** for menus, HUD, chat, inventory, world list, and modals.
- **Lightyear 0.26** (netcode + UDP) for the wire transport. Singleplayer
  spawns a loopback host on an ephemeral port and connects the same client
  to it; dedicated multiplayer reuses the exact same host wrapper.
- **Rapier3D** for dropped-item physics bodies.
- **postcard + zstd** for compact, versioned world saves on disk.
- **Custom first-person controller** with substep collision, jump
  buffering, coyote time, and reconciliation hooks.
- **Tick-based authoritative server** at 20 Hz with per-client snapshots,
  reliable/unreliable channel selection per message type, and stale-client
  timeout.
- **Platform-aware persistence** via `directories` for client settings and
  world saves under the OS app-data directory.
- **Cargo workspace conventions**: `cargo check`, `cargo test`, rustfmt,
  clippy with `-D warnings`, and optional `cargo-llvm-cov` for coverage,
  all wrapped behind the `./cli` script.

## Run

- `./cli dev` — close any stale client and launch the Bevy client.
- `./cli server --bind 127.0.0.1:7777 --auth offline` — run a dedicated
  authoritative server. Add `--world <path>` to host from a specific save,
  or `--admin-socket <path>` to enable the admin Unix socket.
- `./cli admin --socket <path> announce <message...>` — push a chat
  announcement to a running dedicated server.
- `./cli admin --socket <path> shutdown [--reason ...]` — kick all
  clients and gracefully stop a dedicated server.
- `./cli check` / `./cli test` / `./cli lint` — `cargo check --all-targets`,
  tests, and rustfmt + clippy.
- `./cli audit` — run `cargo audit` against the RustSec advisory database
  (installs `cargo-audit` on first run). Also runs daily on CI at 06:00 UTC.
- `./cli coverage` — text coverage via `cargo-llvm-cov` when installed.
- `./cli publish` — release builds for macOS (aarch64 + x86_64), Linux,
  and Windows into `builds/`.

## Shape

- **Client** (`src/app/`): Bevy scene, scheduled `ClientSystemSet`s,
  first-person camera, egui screens, audio, inventory UI, toasts, and
  local prediction.
- **Shared server** (`src/server/`): authoritative state for both
  singleplayer loopback and dedicated multiplayer. Auth, sessions,
  movement acceptance, inventory, dropped items, resource nodes, chat,
  admin actions, snapshots — all split by concern.
- **Networking** (`src/net/`): one client wrapper, one host wrapper,
  one shared protocol module, plus a thin dedicated-server entrypoint.
- **Controller** (`src/controller/`): the movement simulation that both
  prediction and server-side checks live against.
- **Items + resources** (`src/items.rs`, `src/resources.rs`): item
  definitions, tool profiles, resource node definitions, and gather
  rules.
- **World + save** (`src/world.rs`, `src/save.rs`): map types, blocks,
  resource spawns, and the on-disk save format.
- **Steam shim** (`src/steam.rs`): offline auth backend now; the `steam`
  cargo feature is the integration hook when a live verifier lands.

## Status notes

- Movement is intentionally client-authoritative for responsiveness;
  the server validates sequence/finite values and republishes snapshots.
- Steam auth and the Steam server browser are placeholders — `AuthMode::Steam`
  currently rejects until a real `SteamGameServer` verifier is wired in.
- Procedural maps are a sized flat floor for now; the test world has the
  full obstacle course, perimeter walls, ore clusters, and tree groves.

See `CLAUDE.md` and the `docs/` folder for deeper context.
