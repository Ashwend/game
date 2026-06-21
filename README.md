# Ashwend

[![Quality Gate](https://github.com/ashwend/game/actions/workflows/quality-gate.yml/badge.svg)](https://github.com/ashwend/game/actions/workflows/quality-gate.yml)
[![Coverage Gate](https://github.com/ashwend/game/actions/workflows/coverage.yml/badge.svg)](https://github.com/ashwend/game/actions/workflows/coverage.yml)
[![Dependency Audit](https://github.com/ashwend/game/actions/workflows/audit.yml/badge.svg)](https://github.com/ashwend/game/actions/workflows/audit.yml)

Ashwend is a multiplayer, first-person open-world survival game. If you've
played one before, you already know the shape of it: spawn into a
procedurally generated world, gather wood and stone, knap your first crude
tools, mine and smelt ore, raise a base, and fight other players for whatever
they're carrying. Play it solo or together. Both run on the same
dedicated-server core, natively.[^stack]

Ashwend isn't trying to reinvent the genre. It shares its core mechanics
with the survival games already out there, and there's no secret ingredient
in the brewing meant to set it apart. The goal is to do the familiar loop,
and do it well.

## Current state

The honest version: it's early and crude. The core game loop isn't finished
yet. What's there is genuinely playable: gather resources, craft a set of
tools, smelt ore in a furnace, raise a base, and lock it down with a tool
cupboard. But you'll hit the content ceiling *fast*; a determined evening or
two reaches the end of what's there today.

Ashwend is in **active development**, with no release date set yet. Expect
patches to land roughly **weekly**, with new content and bug fixes.

## Installing

Grab a build from [the website](https://www.ashwend.com) or the
[GitHub releases page](https://github.com/Ashwend/game/releases). There's no
sign-up: you create your account in the game itself the first time you launch.

The builds aren't code-signed with proper certificates yet, so your OS will
flag Ashwend as coming from an unidentified developer the first time you open
it. It's safe to run; you just have to allow it once.

- **macOS**: open the app and dismiss the first warning, then open System
  Settings, go to Privacy & Security, scroll to the bottom, and click "Open
  Anyway". Confirm once and it launches normally from then on.
- **Windows**: if "Windows protected your PC" appears, click "More info", then
  "Run anyway".

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
