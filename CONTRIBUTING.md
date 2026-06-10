# Contributing to Ashwend

Thanks for your interest in Ashwend. The source is public so people can read it,
learn from it, and get involved. Contributions are welcome, whether that is a bug
report, a fix, or a new feature.

Please read the short sections below before opening a pull request. The
"Contribution terms" and "Sign your commits" sections are required: they are what
let the project accept your work while keeping all rights reserved to Ashwend.

## Before you start

Ashwend is **source-available, not open source**. The code is licensed under the
[PolyForm Strict License 1.0.0](LICENSE), which lets people use and study it for
noncommercial purposes but does not grant redistribution or the right to ship
derivative builds. Contributing does not change that: your work becomes part of
Ashwend and is distributed under the same license, on the terms below.

If you are planning a large change, please open an issue first so we can agree on
the approach before you invest the time. Small, focused pull requests are much
easier to review and land than sweeping ones.

## Build dependencies

A fresh machine needs a Rust toolchain installed via [rustup](https://rustup.rs)
(the pinned version in `rust-toolchain.toml` is picked up automatically) plus the
system packages below before `./cli dev` builds. The only third-party C library
is libopus, used by voice chat; the rest are the usual Bevy desktop deps.

- **Linux** (Debian/Ubuntu):
  `sudo apt-get install -y g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev libxkbcommon-dev libopus-dev`
- **macOS**: `brew install opus`. If the build falls back to compiling a bundled
  Opus and fails under CMake 4.x, point pkg-config at the brew install:
  `export PKG_CONFIG_PATH="$(brew --prefix opus)/lib/pkgconfig:${PKG_CONFIG_PATH:-}"`
- **Windows** (MSVC): install Opus through vcpkg and point the build at it, the
  Opus bindings do not use pkg-config on Windows and their bundled CMake build
  fails on CMake 4.x:
  `vcpkg install opus:x64-windows-static-md`, then set
  `OPUS_LIB_DIR=<vcpkg root>\installed\x64-windows-static-md` and `OPUS_STATIC=1`.

## How to contribute

1. **Open an issue** for bugs or proposals, or comment on an existing one.
2. **Fork the repository** and create a branch for your change. Forking a public
   repository to submit a pull request is allowed by GitHub's Terms of Service
   regardless of the license.
3. **Make your change.** Keep modules split by concern and follow the existing
   structure (see [`CLAUDE.md`](CLAUDE.md) and [`docs/`](docs/)). Add tests next to
   the code they cover, especially for protocol, server authority, persistence,
   and layout or state helpers.
   - Run the client with `./cli dev`. For faster rebuilds during local
     iteration use `./cli dev-fast`, which enables `bevy/dynamic_linking`. It is
     a dev-only shortcut: never publish a `dev-fast` build. `./cli profile`
     captures a Chrome trace for performance work.
4. **Run the checks** before you push:
   - `./cli check` (compiles all targets)
   - `./cli test` (unit and integration tests)
   - `./cli lint` (rustfmt check and clippy with warnings denied)
   - `./cli ci` runs all of the above the way CI does (including the
     `--all-features` leg), so you can reproduce a Quality Gate failure locally
     in one command. Optionally install the git hook with `./cli setup-hooks`
     to run the lint step automatically on `git push`.
5. **Use Conventional Commits** for your commit subjects. The release pipeline
   parses these to build the changelog. Examples:
   - `feat: add water rendering to coastal chunks`
   - `fix: stop trees clipping the camera on spawn`
   - `docs: clarify the chunk AoI ring in networking.md`
   Common types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`,
   `ci`, `chore`, `revert`. Append `!` for a breaking change.
6. **Open a pull request** against `main` and describe what changed and why.

## Contribution terms

By submitting a contribution (any code, documentation, or other material you
propose for inclusion, for example through a pull request), you agree that:

1. You grant Dannie Hansen, operating as Ashwend (the "maintainer"), a perpetual,
   worldwide, non-exclusive, royalty-free, irrevocable license, with the right to
   sublicense, to use, reproduce, modify, adapt, prepare derivative works of,
   publicly display and perform, distribute, relicense, and otherwise exploit your
   contribution and any derivative works, for any purpose, including commercial
   purposes, and under any license the maintainer chooses (including the current
   [PolyForm Strict License 1.0.0](LICENSE) and any future license).
2. You retain copyright in your contribution. This grant is a license, not an
   assignment, so you keep ownership while the maintainer receives the rights
   above.
3. You have the legal right to grant this license, your contribution is your
   original work or you otherwise have the right to submit it, and to the best of
   your knowledge it does not infringe anyone else's rights.
4. Your contribution is provided without any warranty.

This keeps all rights over the combined work reserved to Ashwend, so the project
can ship, relicense, and commercialize freely while you remain the author of your
own work.

## Sign your commits

Every commit must carry a `Signed-off-by` line that certifies the Developer
Certificate of Origin (reproduced below) and indicates your agreement to the
Contribution terms above. Add it automatically with:

```
git commit -s -m "feat: your change"
```

This appends a line in the form:

```
Signed-off-by: Your Name <you@example.com>
```

Use your real name and an email you can be reached at. Pull requests whose commits
are not signed off cannot be merged.

For this project, read "the open source license indicated in the file" in the
Developer Certificate of Origin as the license under which Ashwend is made
available, namely the PolyForm Strict License 1.0.0 together with the Contribution
terms in this document.

## Developer Certificate of Origin

```
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

## Questions

If anything here is unclear, open an issue and ask before you start. Thanks for
helping build Ashwend.
