# Ashwend

[![Quality Gate](https://github.com/ashwend/game/actions/workflows/quality-gate.yml/badge.svg)](https://github.com/ashwend/game/actions/workflows/quality-gate.yml)
[![Coverage Gate](https://github.com/ashwend/game/actions/workflows/coverage.yml/badge.svg)](https://github.com/ashwend/game/actions/workflows/coverage.yml)
[![Dependency Audit](https://github.com/ashwend/game/actions/workflows/audit.yml/badge.svg)](https://github.com/ashwend/game/actions/workflows/audit.yml)

Ashwend is an open-world survival game. If you've played one before, you
already know the shape of it: spawn into a procedurally generated world,
gather wood and stone, knap your first crude tools, mine and smelt ore,
build, and fight other players for whatever they're carrying. Play it solo
or together. Both run on the same dedicated-server core, natively.[^stack]

Ashwend isn't trying to reinvent the genre. It shares its core mechanics
with the survival games already out there, and there's no secret ingredient
in the brewing meant to set it apart. The goal is to do the familiar loop,
and do it well.

## Current state

The honest version: it's early and crude. The core game loop isn't finished
yet. What's there is genuinely playable, but you'll hit the content ceiling
*fast*. Once you've made the starting tools and a furnace, there isn't much
left to chase.

Ashwend is in **active development with no planned release date**. Expect
patches to land roughly **weekly**, with new content and bug fixes.

## Planned

Roughly where things are headed next:

- **Terrain heightmaps**: real elevation in the procedural generator. No more
  flat world.
- **Water**: ideally shaping the map into an island, so natural shorelines can
  replace artificial world borders.
- **Higher-tier tools.**
- **Weapons**: both primitive and modern.
- **Player building**: proper base building on top of the existing deployable
  system.
- **Base raiding**: explosives.
- **A tutorial**: just enough to show new players the ropes.
- **Steam integration**: a real implementation / alpha.

## License

Ashwend is source-available, not open source. The code is published so people
can read it, learn from it, and play with it, and so anyone who wants to get
involved can contribute back.

It is licensed under the [PolyForm Strict License 1.0.0](LICENSE): you may use
and study the source for noncommercial purposes, but you may not redistribute
it, and you may not distribute changes or new works based on it. All rights are
reserved to the licensor.

The name "Ashwend", together with any logos, is a trademark and is not covered
by the code license.

Want to get involved? See [CONTRIBUTING.md](CONTRIBUTING.md).

[^stack]: Built with [Rust](https://www.rust-lang.org/) and the
    [Bevy](https://bevyengine.org/) engine, with
    [Lightyear](https://github.com/cBournhonesque/lightyear) for netcode over
    UDP, [Rapier3D](https://rapier.rs/) for physics, [egui](https://www.egui.rs/)
    for the UI, [Opus](https://opus-codec.org/) for voice chat, and
    postcard + zstd for compact, versioned saves.
