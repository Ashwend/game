---
title: docs/ navigation index (agent router + reverse code-path map)
owns: routing an agent from a task or subsystem to the one doc that is its source of truth, plus re-resolving stale code paths
when_to_read: CLAUDE.md's routing table is not specific enough, you need to find which doc owns a concern, or a doc cited a path that 404s and you need to re-resolve it.
sources:
  - src/lib.rs - crate module roots (the top-level subsystem list)
  - src/cli.rs - Command enum (client / server / admin / multiplayer-test)
  - docs/ - the doc tree this index maps
related:
  - CLAUDE.md - authoritative entry point; this index expands its link list and never overrides it
  - docs/architecture.md - the next hop for app wiring and the big picture
---

# docs/ navigation index

> When to read this: CLAUDE.md's routing table is not specific enough, you need to discover which doc owns a concern, or a doc cited a path that no longer exists and you need to re-resolve it. Source of truth: this index plus the live tree under `src/`. Canonical invariants live in `CLAUDE.md`, not here.

This is a flat map of every agent doc in `docs/` and `docs/playbooks/`, keyed by both subsystem and task intent. `CLAUDE.md` is loaded first and is the real entry point; this index is the fallback router for when you land in `docs/` cold, plus the tie-breaker for a citation whose path has drifted. It does not restate the invariants `CLAUDE.md` owns (singleplayer equals multiplayer, gameplay never pauses, the replicated-state rules, balance lives in `game_balance.rs`, no monolithic files, no em dashes); follow the link to `CLAUDE.md` for those.

## Task-intent index

Find your intent, jump to the doc. This mirrors `CLAUDE.md`'s routing table; if the two disagree, `CLAUDE.md` wins and this row is stale.

- Understand app wiring / add a `ClientSystemSet` or Bevy plugin: [architecture.md](architecture.md)
- Add a UI overlay / modal / anything you want to "pause": [gameplay-gating.md](gameplay-gating.md)
- Add a `ClientMessage`/`ServerMessage`, change a channel, touch handshake/auth or the admin socket: [networking.md](networking.md)
- Add a replicated entity/component, a mirror-sync system, or debug stale replicated state: [replication.md](replication.md) (procedure: [playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md))
- Add/change a `ClientMessage` handler, a tick subsystem, or server authoritative state: [server-authority.md](server-authority.md)
- Touch the character controller, movement feel, or the client-authoritative trust boundary: [movement.md](movement.md)
- Add or edit an item / tool / ore / resource node / gather rule: [items-and-resources.md](items-and-resources.md) (step-by-step: [playbooks/add-content.md](playbooks/add-content.md))
- Touch crafting, furnaces, deployable placement/damage, loot bags, or add a deployable kind: [crafting-and-deployables.md](crafting-and-deployables.md)
- Change building geometry/cost/HP/stability, doors, or Tool Cupboard claims: [base-building-and-claims.md](base-building-and-claims.md)
- Touch combat validation, weapon feel, knockback, death/respawn, or loot bags: [pvp-combat.md](pvp-combat.md)
- Change world generation, biome classification, a persisted struct, or save format: [worlds-and-saves.md](worlds-and-saves.md)
- Touch chunk membership, AoI ring math, node regrow, or the room-subscription system: [chunks-and-aoi.md](chunks-and-aoi.md)
- Make a prop cel-shaded / change palette/lighting mood / plan an art shift: [art-direction.md](art-direction.md)
- Edit the cel shader, add a prop to the `ToonMaterial` family, retune bands/edges: [toon-shading.md](toon-shading.md)
- Add a `StandardMaterial` or tune reflectance/roughness/metallic or atmosphere/IBL: [rendering-materials.md](rendering-materials.md)
- Touch mic capture, Opus codec, the voice channel, spatial mixing, or the voice UI: [voice.md](voice.md)
- Add a screen/overlay/modal/egui surface, or find where a UI surface lives: [ui-and-client.md](ui-and-client.md)
- Optimize, chase a frame spike, or add O(live-entities)-per-tick work: [profiling.md](profiling.md)
- Change release-asset names, the self-update flow, changelog modals, or installers: [updates-and-distribution.md](updates-and-distribution.md)
- Change player auth, or look up Steamworks package/store IDs: [steam.md](steam.md)
- Launch/drive/screenshot/assert on the running game to verify a change: [headless-agent-testing.md](headless-agent-testing.md)
- Run or modify the two-client multiplayer-test helper: [multiplayer-testing.md](multiplayer-testing.md)
- Run/build/test/profile/release, or find a `./cli` subcommand: [build-and-dev.md](build-and-dev.md)
- Commit code / add a lint or dependency / check a naming convention: [code-style.md](code-style.md)
- Design a new gameplay feature, tune the loop, or understand intent/scope: [game-design.md](game-design.md)
- Model a held item/prop, derive a sibling tool, author a glb, generate an icon/texture: [playbooks/art-pipeline.md](playbooks/art-pipeline.md)
- Add armor, a ranged weapon, or touch arrows/projectiles: [pvp-combat.md](pvp-combat.md)
- Add an explosive / placed charge, touch the fuse/defuse flow, or workbench tiers: [crafting-and-deployables.md](crafting-and-deployables.md)
- Change ruins (POI scatter, prefab layouts) or meteorite worldgen: [worlds-and-saves.md](worlds-and-saves.md)
- Touch the meteor shower event (announce, trajectory, scheduler, impact, VFX/HUD): [meteor-shower.md](meteor-shower.md)
- Touch the cinematic stage map, `/cinematic` playback, or record the hero/trailer footage: [cinematic.md](cinematic.md)
- Add a weapon / armor set / explosive / ranged weapon, or touch workbench tiers, ruins, meteorite, or the meteor shower event: the shipped behavior lives in the same subsystem docs (weapons/armor/ranged/explosives in [pvp-combat.md](pvp-combat.md), charges + workbench tiers + ruin caches in [crafting-and-deployables.md](crafting-and-deployables.md), the item rows in [items-and-resources.md](items-and-resources.md), ruins/meteorite worldgen in [worlds-and-saves.md](worlds-and-saves.md), the meteor shower event in [meteor-shower.md](meteor-shower.md)); the design intent behind them lives in [game-design.md](game-design.md).

## Per-doc index

One row per agent doc. `owns` is the single concern the doc is source of truth for; `when to read` is the trigger that should send you there.

| path | owns | when to read |
| --- | --- | --- |
| [architecture.md](architecture.md) | app wiring, `ClientSystemSet` schedule ordering, how subsystems connect | before touching `app.rs` scheduling, adding a set/plugin, or for the big picture |
| [gameplay-gating.md](gameplay-gating.md) | the gameplay-never-pauses contract and control gating | before adding any overlay/modal or a system you want to "pause" |
| [networking.md](networking.md) | transport, channels, handshake/auth, message inventory, admin socket | before adding a wire message, changing a channel, or touching the handshake |
| [replication.md](replication.md) | per-component Lightyear replication and the host mirror | before adding a replicated entity/component/mirror-sync, or debugging stale state |
| [server-authority.md](server-authority.md) | `GameServer` authoritative state and message handlers | before adding a handler, a tick subsystem, or server-side state |
| [movement.md](movement.md) | the character controller and the client-authoritative trust boundary | before touching movement feel/tuning or debugging rubber-banding |
| [items-and-resources.md](items-and-resources.md) | item/tool/resource registries and gather rules | before adding/editing an item, tool, ore, node, or gather rule |
| [crafting-and-deployables.md](crafting-and-deployables.md) | crafting queue, furnace state machine, unified deployable system | before touching crafting, furnaces, deployable placement/damage, or loot bags |
| [base-building-and-claims.md](base-building-and-claims.md) | building geometry/cost/HP, stability, doors, Tool Cupboard claims | before changing building rules, doors, or the claim system |
| [pvp-combat.md](pvp-combat.md) | combat validation, weapon feel, knockback, death/respawn, loot bags | before touching combat, knockback, or the death/respawn flow |
| [worlds-and-saves.md](worlds-and-saves.md) | world generation, biome classification, and save persistence | before changing generation, a persisted struct, or save format |
| [meteor-shower.md](meteor-shower.md) | the meteor shower event end to end: announce wire contract, seed-pure trajectory math, scheduler and site selection, impact resolution, crater/shards/VFX/HUD | before touching anything meteor-related |
| [cinematic.md](cinematic.md) | the cinematic stage map type, the `/cinematic` shot sequence (server orchestrator, dummy actors, camera paths, slate UI), and the recording workflow | before touching anything cinematic-related or recording the trailer |
| [chunks-and-aoi.md](chunks-and-aoi.md) | runtime chunk grid and Lightyear room-based AoI | before touching chunk membership, AoI math, regrow, or room subscription |
| [art-direction.md](art-direction.md) | overall look-and-feel, palette, lighting mood | before making a prop cel-shaded or planning a wider art shift |
| [toon-shading.md](toon-shading.md) | cel shader mechanics and the `ToonMaterial` family | before editing the cel shader or adding a prop to the cel family |
| [rendering-materials.md](rendering-materials.md) | PBR material conventions and atmosphere/IBL lighting | before adding a `StandardMaterial` or tuning PBR/lighting |
| [voice.md](voice.md) | voice chat: capture, codec, channel, spatial mixing, UI | before touching mic capture, Opus, the voice channel, or the voice UI |
| [ui-and-client.md](ui-and-client.md) | client UI architecture, screens, modals, flow | before adding a screen/overlay/modal or to find where a surface lives |
| [profiling.md](profiling.md) | profiling workflow and the per-tick cost discipline | before optimizing, chasing a spike, or adding O(live-entities) work |
| [resource-node-instancing.md](resource-node-instancing.md) | unimplemented proposal: cutting the resource-node render floor | only when asked to reduce the ~1800-visible-entity per-frame render cost |
| [updates-and-distribution.md](updates-and-distribution.md) | self-update flow, changelog modals, packaging/signing | before changing release-asset names, the update flow, or installers |
| [steam.md](steam.md) | Steamworks package/store IDs and Steam auth notes | before changing auth or touching Steamworks packages |
| [headless-agent-testing.md](headless-agent-testing.md) | driving the game headless to verify changes | when you need to launch/drive/screenshot/assert on the running game |
| [multiplayer-testing.md](multiplayer-testing.md) | the two-client multiplayer-test helper | before running or modifying that helper or capturing peer-to-peer visuals |
| [build-and-dev.md](build-and-dev.md) | the `./cli` surface (build/run/test/profile/ship) | before running, building, testing, or releasing |
| [code-style.md](code-style.md) | code style and structural conventions | before committing, adding a lint/dependency, or on a naming question |
| [game-design.md](game-design.md) | game direction, core loop, intent/scope | before designing a gameplay feature or to understand what is deliberately absent |
| [playbooks/add-content.md](playbooks/add-content.md) | repeatable recipe: add a tool/ore/node/recipe/smeltable/deployable | when the task is literally "add a new X" of that family |
| [playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md) | repeatable recipe: add a networked entity | when introducing any new per-entity authoritative state the client renders |
| [playbooks/art-pipeline.md](playbooks/art-pipeline.md) | repeatable recipe: author a model or icon | when modelling a held item/prop, deriving a tool, or generating an icon/texture |

Proposal docs (front-matter `status: proposal`) describe designs that are NOT shipped. The only one today is [resource-node-instancing.md](resource-node-instancing.md) (render-floor reduction, NOT implemented).

## Directory-vs-file cheat sheet

Several subsystems CLAUDE.md still names as single `.rs` files have since split into directories (and sometimes keep a same-named file beside the directory). When a citation points at a stale path, resolve it here against the live tree under `src/`. Verified against the working tree.

| concern | live layout | notes |
| --- | --- | --- |
| protocol | root `src/protocol.rs` + `src/protocol/` directory: `messages.rs`, `commands.rs`, `items.rs`, `math.rs`, `world.rs`, `world_map.rs` | the root file holds shared consts/ids; wire variants live in `messages.rs` |
| host adapter | `src/net/host.rs` file plus `src/net/host/` directory: `mirror.rs`, `rooms.rs`, `routing.rs`, `handle.rs`, `admin.rs` | file and directory coexist; mirror-sync is `host/mirror.rs`, room/AoI subscription is `host/rooms.rs`, admin socket is `host/admin.rs` |
| server authority | `src/server/` directory, one file per concern: `connection.rs`, `inventory.rs`, `movement.rs`, `crafting.rs`, `combat.rs`, `building.rs`, `door.rs`, `claim.rs`, `stability.rs`, `dropped_items.rs`, `resource_nodes.rs`, `sleeping_bag.rs`, `storage_box.rs`, `torch.rs`, `world_time.rs`, `world_map.rs`, `workbench.rs`, `fuse.rs`, `explosion.rs`, `defuse.rs`, `projectiles.rs`, `projectile_ecs.rs`, `meteor_shower.rs`, `ruin_cache.rs`, plus `commands/`, `furnace/`, `chunk_manager/`, `workbench/`, `tests/` | each `*_ecs.rs` (e.g. `resource_node_ecs.rs`, `dropped_item_ecs.rs`, `player_ecs.rs`, `deployable_ecs.rs`, `loot_bag_ecs.rs`) holds the replicated mirror components for that concern |
| resource nodes | server: `src/server/resource_nodes.rs` + `src/server/resource_node_ecs.rs` + `src/server/tests/resource_nodes.rs`; client: root `src/app/systems/items/resource_nodes.rs` + its directory (`spawn.rs`, `stages.rs`, `pop_in.rs`, `hay_sway.rs`, `tests.rs`) | the client reconciliation pattern CLAUDE.md cites lives in `app/systems/items/resource_nodes.rs` |
| deployables | `src/server/deployables.rs` file; server tests in `src/server/tests/deployables.rs`; replicated mirror in `src/server/deployable_ecs.rs`; client placement in `src/app/systems/deployables/placement.rs` plus `placement/` (`snapping.rs` snap/occupancy geometry, `claim_ring.rs` claim-boundary ring VFX) | the deployables tests live under `src/server/tests/`, not inline |
| loot bags | `src/server/loot_bag.rs` + `src/server/loot_bag_ecs.rs` + `src/server/loot_bag/` (`slots.rs`, `tests.rs`) + `src/server/tests/loot_bag.rs`; client `src/app/ui/loot_bag.rs` and `src/app/systems/items/loot_bag.rs` | bag slot logic is `loot_bag/slots.rs`; two test files (`loot_bag/tests.rs` and `tests/loot_bag.rs`) |
| client audio | root `src/app/audio.rs` + `src/app/audio/` directory: `ambient.rs`, `music.rs`, `footsteps.rs`, `impact.rs`, `surface.rs`, `fader.rs`, `transitions.rs`, `scheduled.rs`, `library.rs`, `manifest.rs`, `category.rs` | there is no `src/app/systems/audio.rs`; main-menu music is `audio/music.rs`, UI one-shots are queued elsewhere via request resources |
| client scene/materials | `src/app/scene.rs` file plus `src/app/scene/` directory: `assets.rs`, `deployable_assets.rs`, `materials.rs`, `world.rs`, `toon.rs`, `terrain.rs`, `sky.rs`, `meteor_sky.rs`, `meteor_shower.rs`, `mesh.rs` (+ `mesh/`), `grass/`, `components.rs` | shared `StandardMaterial` setup is `scene/assets.rs`; cel material is `scene/toon.rs`; terrain material is `scene/terrain.rs` |

Crate module roots are the top of `src/lib.rs`: `analytics, app, auth, building, cli, combat, console, control_socket (dev/unix only), controller, crafting, game_balance, inventory, items, local_crypto, logging, net, protocol, resource_nodes, save, server, update, util, world, world_time`.

The binary is `ashwend`. `src/cli.rs` dispatches four subcommands: `client` (the default when none is given), `server`, `admin`, and `multiplayer-test`.

## Convention legend

Every agent doc (everything in `docs/` except this section's exceptions) follows the same shape. If a doc is missing these, treat it as not yet recalibrated.

- Front-matter: a YAML block between `---` fences at the very top with `title`, `owns` (the one concern the doc is source of truth for), `when_to_read` (the trigger that sends an agent here), `sources` (a list of `src/path.rs - symbol/role` the doc is pinned to), and `related` (sibling docs with a why).
- When-to-read header: an H1 followed by a one-line blockquote `> When to read this: <trigger>. Source of truth: <key files>. Canonical invariants live in CLAUDE.md.`
- Citation style: code is cited as `` `src/path.rs:LINE - symbol` `` or `` `src/path.rs - symbol` ``, repo-root-relative. Lines drift, so symbols are preferred over bare line numbers. Verify a citation against live code before trusting it; if it 404s, re-resolve via the cheat sheet above.
- Paths are absolute from the repo root (`src/...`, `docs/...`), never relative to the reader's cwd.
- No em dashes anywhere (UI copy, docs, comments, commit messages). Use a comma, period, semicolon, colon, parentheses, or reword. The hyphen-minus in a citation like `src/path.rs - symbol` is fine; the long aside dash is banned project-wide.
- No relative dates in docs; state things timelessly or use an absolute date.

## What lives where

- `docs/` describes shipped reality. If a doc claims a behavior, it is in the code; aspirational content is marked explicitly.
- `docs/playbooks/` holds repeatable procedures (the "how to add an X" recipes). A playbook is a checklist, not a reference; the owning reference doc explains the why.
- Proposal docs live in `docs/` alongside shipped-behavior docs, marked `status: proposal` in front-matter. Each carries a STATUS banner stating implementation state; do not treat one as ground truth for current behavior.
- `README.md` (repo root) is human-facing and excluded from the agent doc graph. Agents read it only for canonical design-pillar wording and install prerequisites, both of which are mirrored into agent docs ([game-design.md](game-design.md), [build-and-dev.md](build-and-dev.md)). `docs/README.md` (this file) is the agent index.

## Related docs

- [../CLAUDE.md](../CLAUDE.md) - authoritative entry point and invariant store; this index expands its link list and never overrides it
- [architecture.md](architecture.md) - the usual next hop: app wiring and how a subsystem connects to the rest
- [code-style.md](code-style.md) - the conventions the legend above summarizes, in full
- [playbooks/add-replicated-entity.md](playbooks/add-replicated-entity.md), [playbooks/add-content.md](playbooks/add-content.md), [playbooks/art-pipeline.md](playbooks/art-pipeline.md) - the repeatable procedures
