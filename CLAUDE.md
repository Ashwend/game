# CLAUDE.md

Always-loaded agent root for the Ashwend repo. This file routes you to the right doc, states the cross-subsystem invariants once (this is their canonical home), and gives the corrected `src/` module map. It does not re-explain mechanics; follow the links.

## Global rules (apply to every task)

- Never create markdown summary, report, or findings files unless the user explicitly asks. Return findings in chat.
- No em dashes anywhere: UI copy, website text, docs, comments, commit messages. The long dash used as an aside or clause break is banned. Use a comma, period, semicolon, colon, parentheses, or reword. (The hyphen in `src/path.rs - symbol` citations is fine.)
- No monolithic files. If a file mixes transport, domain rules, UI layout, persistence, and tests, split by concern before extending it. Keep module boundaries; prefer small modules with clear ownership.
- Add tests near the module that owns the behavior, especially for protocol changes, server authority, persistence, and layout/state helpers.
- Update the relevant existing doc when you change architecture. Do not invent new top-level docs casually; fit the existing map below.

## What is Ashwend

Ashwend is a first-person Rust/Bevy multiplayer survival prototype (gather, craft, build, raid, PvP). Singleplayer and multiplayer run the exact same authoritative-server code path: a Lightyear-backed `GameServer` reached through `src/net/client.rs - ClientSession`. Singleplayer differs only by starting an in-process loopback host, marking the local player admin, and persisting the save locally; multiplayer points the same client at a remote dedicated host. The singleplayer entry is a dev/test convenience, gated out of release builds (`#[cfg(debug_assertions)]` on the main-menu button); shipped players enter through Multiplayer. Worlds are compressed binary `.save` files (postcard + zstd, versioned `GAMESAVE` header). The art direction is mid-transition from PBR toward a cel/anime look: ore nodes, trees, deployables, and grass are cel-shaded, and held items and armor now carry their own material families (held items on a PBR-baked path per the tools rework, armor on the rig's material family); building pieces and doors are still PBR.

## Corrected `src/` module map

Top-level modules declared in `src/lib.rs` (23 total). Some are a single `.rs` file, some are a `mod.rs`-style directory, and a few front a `.rs` file plus a sibling directory of submodules. The `(dir)` flag below means "open the directory, not just the file."

| Module | Kind | Role |
| --- | --- | --- |
| `analytics` | dir | Client-only PostHog analytics plugin (one OS worker thread, no async runtime). Not loaded by the dedicated server or admin CLI. |
| `app` | file + `app/` dir | Bevy client app: wiring, `ClientSystemSet` schedule ordering (`src/app/systems.rs - ClientSystemSet`), `run_app` entry. Subdirs `app/state/`, `app/systems/` (`camera/`, `control_socket/`, `deployables/`, `input/`, `items/`, ...), `app/ui/`, `app/scene/`, `app/audio/`, `app/voice/`. The app also runs client systems for ranged input (`systems/input/inventory_shortcuts/ranged.rs`), own-arrow projectiles (`systems/items/projectiles/`), the placed-charge fuse VFX (`systems/deployables/charge_fuse.rs`), explosion VFX (`systems/explosion_vfx.rs`), the workbench upgrade overlay (`ui/workbench.rs`), the paperdoll equipment column (`ui/inventory_panel.rs`), and the meteor shower sky/ground/HUD (`scene/sky.rs`, `scene/meteor_shower.rs`, `ui/hud.rs`). |
| `auth` | dir | Player auth. `AuthMode::Workos` (real, verifies a WorkOS JWT offline against JWKS) vs `AuthMode::NoAuth` (loopback/localhost only, used by singleplayer and `multiplayer-test`). |
| `building` | file | Shared base-building domain rules: piece/tier taxonomy, socket-snap geometry, multi-box colliders, cost/HP tables. Read by both client preview and server authority. |
| `cli` | file + `cli/` dir | `clap` entry. Subcommands: `client`, `server`, `admin`, `multiplayer-test` (the last fronted by the sibling `cli/multiplayer_test.rs` submodule). |
| `combat` | file | Damage primitives shared by every damage source (PvP melee today, projectiles/environment later). |
| `console` | file | Windows-only dual-subsystem console reattachment (shipped builds are GUI-subsystem so no console flashes on launch). |
| `controller` | dir | Movement simulation, movement tuning/math, collision, and the server-side block spatial grid (`BlockGrid`). |
| `crafting` | file + `crafting/` dir | Static server-authoritative crafting recipe registry, exposed by id (recipes never travel on the wire). The recipe array lives in `crafting/registry.rs` (per-category rows), with shared shapes in `crafting/types.rs` (`RecipeDefinition`, `RecipeStation::Workbench { min_tier }`, `RecipeId` interning). |
| `game_balance` | file | Every gameplay tuning constant lives here (combat ranges, gather windows, deployable damage/placement, building HP/costs/raid balance, furnace timings, loot-bag and interact ranges, respawn radius). New balance values go here, not inline. |
| `inventory` | file | Pure inventory math shared by the server and the client-side optimistic prediction overlay. Operates on `PlayerInventoryState`. |
| `items` | file + `items/` dir | Item registry and profiles, organized under `items/`: `registry.rs` (the `ItemDefinition` rows), `ids.rs` (id string consts), `materials.rs` (`DestructibleMaterial`, `tool_effectiveness_pct`, `explosive_effectiveness_pct`), `tools.rs`/`weapons.rs`/`armor.rs`/`ranged.rs`/`explosives.rs` (the `ToolProfile`/`WeaponProfile`/`ArmorProfile`/`RangedProfile`/`ExplosiveProfile` component structs and their enums, e.g. `ExplosiveKind`, `EquipmentSlot` addressing), `deployables.rs` (`DeployableKind` incl. `Workbench { tier }`, `Explosive { kind }`, `RuinCache`), `upgrades.rs` (the generic `DEPLOYABLE_UPGRADES` table), `visual.rs` (`ItemModel`/`HeldMesh`/`ArmorMesh`), `pickup.rs`. Re-exported flat, so call sites still say `crate::items::X`. Dropped-item shapes live in `protocol/world.rs` and `server/dropped_item_ecs.rs`. |
| `local_crypto` | file | At-rest obfuscation for local client files (settings, WorkOS refresh token). Deliberately not a security boundary. |
| `logging` | file | On-disk app log for client and dedicated server (Bevy's default `LogPlugin` only hits stderr, invisible in a packaged release). |
| `net` | file + `net/` dir | Lightyear transport adapters. `net/client.rs` (`ClientSession`), `net/host.rs` + `net/host/` (loopback/dedicated host, mirror sync, room/AoI subscription, admin socket), `net/channels.rs` (channel + `register_component` registration), `net/dedicated/` (CLI dedicated entry + admin requests). |
| `protocol` | dir | Wire protocol: `ClientMessage`/`ServerMessage` surface, channel delivery prefs, and shared shapes both sides serialise (`ResourceNodeState`, `DroppedWorldItem`, `DeployedEntityState`, `OpenFurnaceView`). Split by concern, re-exported flat. |
| `resources` | file | Resource-node definitions (`ResourceNodeDefinition`/`ResourceNodeModel`, in `RESOURCE_NODE_DEFINITIONS`) and gather rules. (`NodeKind` is the world/chunk pipeline's enum, not this module's.) |
| `save` | dir | World persistence: `WorldStore`, `WorldSave`, atomic writes, binary codec, listing/recovery, name validation, format version. |
| `server` | file + `server/` dir | Shared authoritative game state (`GameServer`) for loopback and dedicated alike. `server/` splits connection/auth, inventory, movement, dropped-item, resource-node, deployable, door, sleeping-bag, furnace (`furnace/state.rs`+`tick.rs`+`commands.rs`), and `chunk_manager/`. It also splits out `workbench.rs` (+ `workbench/`, the tier-upgrade command via the generic upgrade table), `fuse.rs` (placed-charge fuse ticking), `explosion.rs` (`resolve_explosion` AoE), `defuse.rs` (claim-authorized defuse + refund), `projectiles.rs` + `projectile_ecs.rs` (server ballistic sim + replicated projectile mirror), `meteor_shower.rs` (meteor scheduler/siting/impact), and `ruin_cache.rs` (world-spawned loot cache refill). The `HashMap`s on `GameServer` are authoritative; `*_ecs.rs` modules define the replicated mirror components. |
| `update` | dir | Client-only in-game update checker + self-updater (boot-time GitHub version check on a background thread). |
| `util` | file + `util/` dir | Tiny dependency-light cross-module helpers (`pub mod fs; pub mod hash; pub mod platform; pub mod variation;`, backed by `src/util/`). |
| `world` | dir | `MapType`, world block geometry, perimeter walls, and the chunk-based generation pipeline under `world/chunk/` (classification, value noise, Poisson-disk spawn generator). The world module also includes `world/ruins.rs` (seed-pure ruin POI scatter + prefab layouts + cache footprints, shared by server worldgen and the client map) and `world/meteor_shower.rs` (the pure-of-seed meteor trajectory both sides evaluate against the world clock). |
| `world_time` | file | Day/night clock shared by server and client. Server owns authoritative time and ships periodic `WorldTimeSnapshot`. |

`ClientSession` (`src/net/client.rs - ClientSession`) is a plain struct holding a `ClientNetwork` handle and the optional loopback `GameServerHandle`. It exposes `start_singleplayer(...)` and `connect(...)`. There is no `ClientSession::Network` enum variant; both modes drive the same struct.

## Invariants (canonical home, stated once)

These are the load-bearing, cross-subsystem rules. Other docs link here instead of restating them.

1. **Singleplayer == multiplayer.** Both consume the same `GameServer` through `ClientMessage`/`ServerMessage`. Do not add a separate singleplayer gameplay implementation, an in-process transport bypass, or duplicate movement/inventory/chat rules. Singleplayer-specific code stays limited to selecting/loading a save, starting the loopback host, marking the local host admin, and saving on shutdown. Multiplayer-specific code stays limited to remote address/discovery, auth mode, transport setup, and dedicated-host lifecycle. New features: make it work through `GameServer` first, then let both paths consume it. The singleplayer main-menu entry is dev/test-only, gated behind `#[cfg(debug_assertions)]` (`src/app/ui/menu.rs - main_menu_ui`), so it never appears in a shipped release; the loopback path remains as the dev/test mirror of the live multiplayer path. Do not surface a release-build singleplayer entry.

2. **Gameplay never pauses.** An authoritative server (loopback or dedicated) runs at all times, so simulation, local prediction, and network ticks keep advancing as long as the local screen is in-game, regardless of which overlay is up (pause, inventory, chat, crafting, furnace, death splash, map, anything). Overlays gate only local **controls** (movement, look, swing). Knockback, replication diffs, death/respawn, and world-time advancement must keep applying while a menu is open, otherwise effects pile up and fire en masse on close. Gate any new overlay through `gameplay_accepts_controls` in `src/app/systems/input/gating.rs`, never through `gameplay_simulation_allowed` (which only flips when you leave the in-game screen entirely). See [docs/gameplay-gating.md](docs/gameplay-gating.md).

3. **Movement is client-authoritative by design.** Clients send `PlayerMovement` produced by local prediction; the server validates sequence/finite values and writes the result onto the player's mirror entity for replication. Do not convert to server-authoritative input simulation unless explicitly asked. See [docs/movement.md](docs/movement.md).

4. **Balance constants live in `src/game_balance.rs`,** never inline in a subsystem.

5. **Replicated state (six rules).** Every networked entity ships through Lightyear per-component replication, room-gated to the AoI chunk ring around each player. Authoritative `HashMap` on `GameServer` + ECS mirror entity, kept in sync by exclusive systems in `src/net/host/mirror.rs` (`sync_resource_node_entities`, `sync_dropped_item_entities`, `sync_deployable_entities`, `sync_projectile_entities`, `sync_player_entities`, `sync_loot_bag_entities`). The easy-to-break constraints:
   - **Use `ServerMessage` for events, replication for per-entity state.** New networked entities (renderable or queryable per-entity state) go through replication, not a snapshot `ServerMessage` variant.
   - **Every spawn attaches `ReplicationGroup::new_from_entity()`.** The two host spawn helpers (`attach_room_gated_replication`, `attach_player_replication`) already do. Do not bypass them with a bare `Replicate::to_clients(...)`: without an explicit group Lightyear lumps the entity into `ReplicationGroupId(0)` and the per-group ack tick can advance past a slow entity's local `Changed` mark, silently dropping the diff (upstream bug [#740](https://github.com/cBournhonesque/lightyear/issues/740)).
   - **Split each entity into one identity component (immutable post-spawn) + one component per mutable field** that changes at a distinct cadence. Diffs are per-component, so this minimizes wire traffic.
   - **Client reconcilers are event-driven, not polling.** React to `Added<T>` and `RemovedComponents<T>`; do not iterate the full replicated query every frame (1-4 ms/frame noop cost at AoI scale ~1800 nodes). Canonical pattern in `src/app/systems/items/resource_nodes/mod.rs` (pending-spawn `VecDeque`, reverse Entity->Id map, one-time catch-up scan after connect).
   - **`Ref::is_changed()` lies for Lightyear-touched components:** it fires on every replication tick even when the value is identical (the receive path uses `insert_by_ids`, which always bumps the change tick). Never gate work behind it.
   - **Never reintroduce a periodic full-state broadcast.** The old `WorldSnapshot` wire was deleted on purpose; fix the replication path, don't replace it.
   - Add `replication-trace` coverage for any new post-spawn-mutating component (`MUTATE` log in mirror sync + `RECV` log on client; run `--features replication-trace` with `RUST_LOG=replication_trace=info`). `MUTATE` without `RECV` = replication failure (usually a missing group); both firing but UI stale = a consumer bug in the `Query<&Component>` reader. Full architecture in [docs/replication.md](docs/replication.md).

## Task -> doc routing

Find your intent, open the doc. Full index and reverse code-path map in [docs/README.md](docs/README.md).

| I want to... | Read |
| --- | --- |
| Understand the whole architecture / Bevy app wiring | [docs/architecture.md](docs/architecture.md) |
| Add a UI overlay, pause, or anything I want to "pause" | [docs/gameplay-gating.md](docs/gameplay-gating.md) |
| Add a `ClientMessage`/`ServerMessage`, channel, handshake, or admin socket | [docs/networking.md](docs/networking.md) |
| Add a replicated entity/component or debug stale replication | [docs/replication.md](docs/replication.md) + [docs/playbooks/add-replicated-entity.md](docs/playbooks/add-replicated-entity.md) |
| Change server-side authoritative game rules or a tick subsystem | [docs/server-authority.md](docs/server-authority.md) |
| Touch movement, feel, or the trust boundary; debug rubber-banding | [docs/movement.md](docs/movement.md) |
| Add/edit a tool, ore, resource node, or gather rule | [docs/items-and-resources.md](docs/items-and-resources.md) |
| Add a tool / ore / recipe / smeltable / deployable (step-by-step) | [docs/playbooks/add-content.md](docs/playbooks/add-content.md) |
| Touch crafting, furnaces, deployable placement/damage, or loot bags | [docs/crafting-and-deployables.md](docs/crafting-and-deployables.md) |
| Touch workbench tiers or a station upgrade | [docs/crafting-and-deployables.md](docs/crafting-and-deployables.md) |
| Touch explosives, placed charges, fuses, defuse, or raid cost | [docs/crafting-and-deployables.md](docs/crafting-and-deployables.md) + [docs/pvp-combat.md](docs/pvp-combat.md) |
| Change building geometry/costs/HP/stability, doors, or Tool Cupboard claims | [docs/base-building-and-claims.md](docs/base-building-and-claims.md) |
| Touch combat validation, weapon feel, knockback, death/respawn, loot bags | [docs/pvp-combat.md](docs/pvp-combat.md) |
| Touch armor, ranged weapons, projectiles, or bow/crossbow feel | [docs/pvp-combat.md](docs/pvp-combat.md) |
| Change world generation, biomes, a persisted struct, or save format | [docs/worlds-and-saves.md](docs/worlds-and-saves.md) |
| Touch ruins, ruin caches, or meteorite worldgen | [docs/worlds-and-saves.md](docs/worlds-and-saves.md) + [docs/crafting-and-deployables.md](docs/crafting-and-deployables.md) |
| Touch the meteor shower event (scheduler, trajectory, crater, HUD) | [docs/meteor-shower.md](docs/meteor-shower.md) |
| Touch chunk membership, AoI ring math, node regrow, or room subscription | [docs/chunks-and-aoi.md](docs/chunks-and-aoi.md) |
| Make a prop cel-shaded or plan an art-direction shift | [docs/art-direction.md](docs/art-direction.md) + [docs/toon-shading.md](docs/toon-shading.md) |
| Add/tune a `StandardMaterial` or the PBR/atmosphere lighting | [docs/rendering-materials.md](docs/rendering-materials.md) |
| Model a held item or icon-matched prop, or generate an icon/texture | [docs/playbooks/art-pipeline.md](docs/playbooks/art-pipeline.md) |
| Touch voice (mic, Opus, voice channel, spatial mix, device UI) | [docs/voice.md](docs/voice.md) |
| Add a screen, modal, or egui UI; find where a UI surface lives | [docs/ui-and-client.md](docs/ui-and-client.md) |
| Profile a frame spike or add O(live-entities)-per-tick work | [docs/profiling.md](docs/profiling.md) |
| Change release-asset names, self-update, changelog modals, or installers | [docs/updates-and-distribution.md](docs/updates-and-distribution.md) |
| Launch, drive, screenshot, or assert on the running game | [docs/headless-agent-testing.md](docs/headless-agent-testing.md) |
| Run or modify the two-client multiplayer-test helper | [docs/multiplayer-testing.md](docs/multiplayer-testing.md) |
| Build, run, test, profile, or release (which `./cli` does what) | [docs/build-and-dev.md](docs/build-and-dev.md) |
| Check a naming/structure convention before committing | [docs/code-style.md](docs/code-style.md) |
| Understand intent, scope, the core loop, or what is deliberately absent | [docs/game-design.md](docs/game-design.md) |

## Build, test, lint

`./cli check` (cargo check), `./cli test`, `./cli lint` (fmt check + clippy), `./cli ci` (all three). Run the game with `./cli dev` (or `./cli dev-fast` for faster incremental rebuilds) and the two-client visual harness with `./cli multiplayer-test`. Fuller `./cli` surface and the rest of the doc index are in [docs/README.md](docs/README.md).
