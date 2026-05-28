# Game

[![Quality Gate](https://github.com/danniehansen/game/actions/workflows/quality-gate.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/quality-gate.yml)
[![Coverage Gate](https://github.com/danniehansen/game/actions/workflows/coverage.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/coverage.yml)
[![Dependency Audit](https://github.com/danniehansen/game/actions/workflows/audit.yml/badge.svg)](https://github.com/danniehansen/game/actions/workflows/audit.yml)

A Rust + Bevy first-person prototype. One authoritative server, one shared
gameplay path, two ways in: a loopback host launched in-process for
singleplayer, or a remote dedicated host for multiplayer. Both are reached
through the same Lightyear/UDP client — the only thing that changes is
where the host runs.

## What you can do today

- Drop into a world from the main menu, or host one and join from another
  client. Both paths run the same simulation through the same `GameServer`.
- Move around with a responsive client-predicted first-person controller —
  walk, sprint, jump, coyote-time, step-up on low ledges, AABB block
  collision against the world geometry.
- Gather resources. Stone hatchets chop pine, birch, and dead trees (three
  size variants each) for wood; stone pickaxes mine coal, iron, and sulfur
  ore nodes. Surface stones, branch piles, and hay grass scattered across
  the world give early-game stone, sticks, and fibre. Tools have tiers,
  cooldowns, and per-swing yields; the world remembers which nodes are
  spent and respawns them on a 5–15 min jittered timer.
- Explore a chunk-generated world. The map is partitioned into 64 m grids;
  each grid is independently classified as forest, rocky outcrop, ore vein,
  plains, or mixed from a seeded noise stack, and resource nodes are
  populated by Poisson-disk sampling against the classification's base
  capacity. Same `(seed, dims)` always generates the same world.
- Carry it home. A 40-slot inventory plus a 9-slot actionbar with hotkey
  switching, scroll-wheel cycling, click-drag and split-stack moves,
  drop-on-floor, line-of-sight pickup tooltips, and a held-item swap
  animation when you change tools.
- Talk to other players. Enter or `T` opens chat; messages run through the
  same reliable channel as auth and inventory commands.
- Hear other players. Hold `V` (rebindable) for push-to-talk; Opus-encoded
  voice is server-proxied and attenuates with distance so peers within
  ~50 m can hear you with proper stereo panning. A pulsing dot beside
  each speaker's nameplate shows who's talking right now. See
  [docs/voice.md](docs/voice.md).
- See feedback. Toasts surface pickup totals, gather events, and other
  server-driven notifications; the HUD shows health and the actionbar.
- Save and resume. Each world is a compressed binary save that captures
  player positions, inventories, dropped items, resource node state,
  deployables, and admin lists. Disconnecting persists the live state;
  a returning player picks up where they left off.
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
- **cpal + libopus** for the voice chat pipeline. Mic capture and speaker
  output run on dedicated worker threads (cpal `Stream` is `!Send` on
  macOS) and ship Opus-encoded 20 ms frames over a dedicated
  `UnorderedUnreliable` Lightyear channel. Requires the system libopus
  (`brew install opus` on macOS / `apt install libopus-dev` on Linux).
- **postcard + zstd** for compact, versioned world saves on disk.
- **Custom first-person controller** with substep collision, jump
  buffering, coyote time, and reconciliation hooks.
- **Tick-based authoritative server** at 20 Hz. Per-entity state ships
  through Lightyear's per-component replication, room-gated to the
  AoI chunk ring around each player. Reliable/unreliable channel
  selection per message type. Stale-client timeout. See
  [docs/networking.md](docs/networking.md) for the replication
  architecture and the load-bearing reliable side-channel pattern
  for Lightyear 0.26.4's known post-spawn-diff bug.
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
- `./cli multiplayer-test [Alpha Bravo]` — one-shot helper that spawns a
  dedicated server plus two client windows tiled side-by-side on the
  primary monitor with the players already facing each other and the
  inventory open. The fastest way to eyeball voice, interpolation, and
  shared-state behaviour. See [docs/multiplayer-testing.md](docs/multiplayer-testing.md).
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
  movement acceptance, inventory, dropped items, resource nodes,
  deployables, chat, admin actions — all split by concern. The
  authoritative state lives in `HashMap`s on `GameServer`; exclusive
  sync systems mirror those into ECS entities carrying the components
  Lightyear replicates.
- **Networking** (`src/net/`): one client wrapper, one host wrapper,
  one shared protocol module, plus a thin dedicated-server entrypoint.
- **Controller** (`src/controller/`): the movement simulation that both
  prediction and server-side checks live against.
- **Items + resources** (`src/items.rs`, `src/resources.rs`): item
  definitions, tool profiles, resource node definitions, and gather
  rules.
- **World + save** (`src/world/`, `src/save/`): map types, perimeter
  geometry, the chunk-based generation pipeline (classification, noise,
  spawn generator) under `world/chunk/`, and the on-disk save format.
- **Chunk manager** (`src/server/chunk_manager.rs`): server-side owner of
  the chunk grid — anchors every networked entity (resource nodes, dropped
  items, eventually buildings) to its containing chunk, drives AoI
  streaming on a per-view-tier ring around each player, schedules node
  regrow events, and persists per-chunk live counts plus pending regrows.
- **Steam shim** (`src/steam.rs`): offline auth backend now; the `steam`
  cargo feature is the integration hook when a live verifier lands.

## Status notes

- Movement is intentionally client-authoritative for responsiveness;
  the server validates sequence/finite values and writes the accepted
  pose onto the player's mirror entity so Lightyear replicates it to
  peers in the same chunk room.
- Steam auth and the Steam server browser are placeholders — `AuthMode::Steam`
  currently rejects until a real `SteamGameServer` verifier is wired in.
- Worlds are chunk-generated against a seed: each 64 m grid gets a biome
  classification and Poisson-disk-sampled resource nodes. Perimeter walls
  enclose the playable area. Visible content is streamed per-player via a
  Chebyshev AoI ring around the player's current chunk (tunable per view
  tier). See [docs/worlds-and-saves.md](docs/worlds-and-saves.md).
- Material/PBR conventions for new meshes (reflectance, roughness,
  metallic) live in [docs/materials.md](docs/materials.md) — consult before
  adding a new `StandardMaterial`.

See `CLAUDE.md` and the `docs/` folder for deeper context.
