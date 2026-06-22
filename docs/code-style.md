---
title: Code style and conventions
owns: The review-and-CI rules of the road, em-dash ban, module-split philosophy, the lint/format/dependency config topology, the game_balance.rs single-source rule, and the commit conventions (Conventional Commits + DCO sign-off).
when_to_read: Before committing code, adding a lint or dependency, editing a tool config, or when unsure of a naming or structure convention.
sources:
  - Cargo.toml - [lints.clippy] / [lints.rust] levels, [features], [profile.*]
  - clippy.toml - clippy tunables (allow-unwrap-in-tests)
  - rustfmt.toml - max_width=100, Unix newlines
  - deny.toml - cargo-deny license/ban/source policy
  - .cargo/audit.toml - cargo-audit ignore list (must stay in sync with deny.toml)
  - src/game_balance.rs - centralized tuning constants + re-export idiom
  - CONTRIBUTING.md - Conventional Commits, DCO sign-off, build prerequisites
  - cli - ./cli lint / ci / setup-hooks gates
related:
  - CLAUDE.md - canonical invariants (SP==MP, gameplay-never-pauses, replicated-state rules, balance-in-game_balance.rs)
  - docs/build-and-dev.md - the ./cli surface (every subcommand, feature flags, build prerequisites)
  - docs/replication.md - the replicated-state pattern this doc points back to
---

# Code style and conventions

> When to read this: before committing code, adding a lint or dependency, or when unsure of a naming/structure convention. Source of truth: `Cargo.toml`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `.cargo/audit.toml`, `src/game_balance.rs`, `CONTRIBUTING.md`. Canonical invariants live in `CLAUDE.md`.

This doc is the bar a change must clear to pass review and CI. The hard gameplay invariants (singleplayer==multiplayer, gameplay-never-pauses, the replicated-state rules) are owned by `CLAUDE.md`; this doc covers the mechanical conventions and links back for the invariants.

## No em dashes, anywhere

Absolute project rule: em dashes (the long dash, U+2014, used as an aside or clause break) are banned in every artifact. UI copy, website text, docs, code comments, commit messages, all of it. The hyphen-minus `-` (as in `src/path.rs - symbol`) is fine; only the long dash is banned.

Rewrite instead of reaching for one:
- A comma for a light pause: "fast, but never light".
- A period or semicolon to split two clauses.
- A colon to introduce a list or explanation.
- Parentheses for a true aside.
- Or just reword the sentence so it does not need the dash.

CI does not currently grep for em dashes, so this is enforced socially and by reviewers. When you touch a file, do not introduce one, and fix any you see. (Historically `docs/toon-shading.md` was the lone violator with 7; the doc rebuild fixes it. If you find an em dash in any committed file, treat it as a bug to fix, not a precedent.)

## Module-split philosophy

`CLAUDE.md` owns the canonical clean-code rules; the short version:

- No monolithic files. If a file starts mixing transport, domain rules, UI layout, persistence, and tests, split it by concern before extending it. Good existing splits: `src/server/`, `src/controller/`, `src/app/systems/`, `src/app/state/`, `src/app/ui/worlds/`, `src/server/furnace/` (state.rs / tick.rs / commands.rs).
- Small modules with clear ownership over broad helper files.
- Keep UI rendering, UI state, session actions, and authoritative game rules in separate modules.
- Networking transport adapters stay thin: translate to shared `ClientMessage`/`ServerMessage` and delegate gameplay to `GameServer`. Do not put gameplay rules in the transport layer.
- Tests live next to the module they cover (see `src/server/tests/`). Add tests for protocol changes, server authority, persistence, and layout/state helpers specifically.

When you add a feature, build it through `ClientMessage`/`ServerMessage` + `GameServer` first, then let both loopback singleplayer and direct multiplayer consume the same path. See `CLAUDE.md` for the full singleplayer==multiplayer invariant and `docs/replication.md` for new per-entity authoritative state (it ships through Lightyear replication, never a new snapshot `ServerMessage`).

## Lint, format, and dependency config topology

Five files, each owning a distinct concern. Editing the wrong one is the common mistake.

| Concern | File | What lives here |
| --- | --- | --- |
| Lint LEVELS | `Cargo.toml` `[lints.clippy]` / `[lints.rust]` | Which lints are `warn`/`deny`. Versioned with the code so cargo, editors, and CI agree. |
| Lint TUNABLES | `clippy.toml` | Clippy knobs, not levels (`allow-unwrap-in-tests`, `allow-expect-in-tests`). |
| FORMAT | `rustfmt.toml` | `max_width = 100`, `newline_style = "Unix"`. That is the whole file. |
| DEPENDENCY policy | `deny.toml` | cargo-deny: license allow-list, duplicate-version bans, source allow-list, advisory ignores. |
| ADVISORY ignores | `.cargo/audit.toml` | cargo-audit ignore list. Must stay in sync with `deny.toml` `[advisories].ignore`. |

### Lint levels (`Cargo.toml`)

`[lints.clippy]` is a curated, high-signal set (not blanket `pedantic`, which is too noisy for Bevy systems with many params). Current `warn` lints:

- `uninlined_format_args`
- `semicolon_if_nothing_returned`
- `manual_let_else`
- `cloned_instead_of_copied`
- `unnested_or_patterns`
- `dbg_macro` (no `dbg!()` in committed code)
- `todo` (no `todo!()` in committed code)

`[lints.rust]` sets `unsafe_op_in_unsafe_fn = "deny"`.

CI runs clippy with `-D warnings`, so every `warn` above is a hard build failure. Do not leave `dbg!()` or `todo!()` in a commit.

`unwrap_used` / `expect_used` are deliberately NOT enabled crate-wide yet (production code has documented-invariant `expect`s that need individual review first). `clippy.toml` already exempts tests so those restriction lints can be turned on for production code later without churning the ~200 deliberate test-only unwraps.

### Format (`rustfmt.toml`)

100-column max width, Unix newlines. Run `cargo fmt --all` (or `./cli lint`, which includes `--check`). The toolchain is pinned in `rust-toolchain.toml` (channel `1.94.0`, with `clippy` + `rustfmt` components) so everyone formats and lints identically.

### Dependency policy (`deny.toml` + `.cargo/audit.toml`)

`deny.toml`:
- License allow-list is a curated permissive-OSS set (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0, Unicode-3.0, and a few more). Anything outside the list needs an explicit, justified entry. Per-crate non-code exceptions exist for bundled fonts (`epaint_default_fonts`) and cert data (`webpki-roots`).
- `multiple-versions = "warn"` (the bevy/lightyear/rapier graphs regularly carry short-lived transitive dupes; CI surfaces them without blocking).
- `wildcards = "deny"`, unknown registries and unknown git sources denied.

`.cargo/audit.toml` holds the cargo-audit ignore list. Every ID is a triaged advisory that cannot be resolved by a version bump (relevant direct deps are already at their latest release). Two of them (`RUSTSEC-2026-0121` steamworks, `RUSTSEC-2025-0134` rustls-pemfile) are lockfile-only: the `steam` and `webtransport` lightyear features are off, so neither crate is compiled.

When you add or change a dependency: the ignore list lives in BOTH `deny.toml` `[advisories].ignore` and `.cargo/audit.toml`, and they must stay in sync. Update both with a one-line triage rationale, or the daily Dependency Audit workflow (and `./cli audit`) breaks. The only third-party C dependency is libopus (voice chat); per-OS install steps and the CMake 4.x workaround (`CMAKE_POLICY_VERSION_MINIMUM` in `.cargo/config.toml`) are in `CONTRIBUTING.md` and `docs/build-and-dev.md`.

### Feature flags

Three feature flags exist in `[features]`, all dev-only, `default = []`:

- `dev-fast` = `bevy/dynamic_linking`. Faster local rebuilds. Never ship or publish a `dev-fast` build.
- `replication-trace`. Logs every server-side mutation and client-side reception of replicated components. Run with `RUST_LOG=replication_trace=info` to verify post-spawn diffs ship (see `docs/replication.md`).
- `profile` = `bevy/trace_chrome`. Emits a Chrome trace plus runtime diagnostics. Has a perf cost and the trace grows fast; use via `./cli profile` only.

`--all-features` enables all three, which is why CI's all-features leg (and `./cli ci`) is what catches breakage in cfg-gated code a plain `./cli check` never compiles.

### The gate

`./cli ci` is the canonical local pre-push gate. It runs fmt check, `clippy --all-features -D warnings`, `check --all-features`, and the test suite. It is a slightly stricter superset of GitHub CI: CI's all-features matrix leg runs clippy + check only and skips tests (`run_tests: false` in `quality-gate.yml`), whereas `./cli ci` also runs the full test suite. The opt-in pre-push hook (`./cli setup-hooks` sets `core.hooksPath=.githooks`) runs only the lighter `./cli lint` leg, not the full `ci`. See `docs/build-and-dev.md` for the complete `./cli` surface.

## game_balance.rs: single source of truth for tuning

Every gameplay tuneable (combat ranges, gather windows, interact distances, smelt timings, knockback shapes, respawn radii, building HP/costs/raid balance) lives in `src/game_balance.rs` and nowhere else. This is canonical in `CLAUDE.md`; do not inline a magic number in a subsystem file, even a "throwaway" one is harder to find later, and an evals/balance-tuning pass expects one file to edit.

The re-export idiom (`src/game_balance.rs` module doc):

1. Declare the constant in `game_balance.rs` with a doc comment explaining what it controls and why this value:
   ```rust
   pub const COMBAT_ATTACK_RANGE_M: f32 = 3.5;
   ```
2. Re-export it from the owning subsystem module:
   ```rust
   pub(crate) use crate::game_balance::COMBAT_ATTACK_RANGE_M;
   ```
3. Reference it via the subsystem path at the call site, so the one-tunable-per-feature-area shape is preserved.

Worked examples of the `pub(crate) use` re-export: `src/combat.rs` (`HATCHET_KNOCKBACK_SPEED`, `PICKAXE_KNOCKBACK_SPEED`), `src/server/loot_bag.rs` (`LOOT_BAG_INTERACT_RANGE_M`), `src/server/storage_box.rs` (`STORAGE_BOX_INTERACT_RANGE_M`), `src/server/furnace/state.rs`.

## Commit conventions

### Conventional Commits

Commit subjects must be Conventional Commits; the release pipeline parses them to build the changelog. Documented common types (`CONTRIBUTING.md`): `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `revert`. Append `!` for a breaking change.

Two notes on what the history actually uses, beyond the documented list:
- `release:` is a real, heavily used type (second most common after `feat:`) for version-bump commits (`release: v0.21.0`). It is not in the `CONTRIBUTING.md` list but is canonical in this repo. Use it for version bumps.
- Avoid vague scope-less subjects like `misc:` (it has appeared, e.g. `refactor: misc`, but a meaningful type/summary is preferred so the changelog is useful).

Examples:
```
feat: add water rendering to coastal chunks
fix: stop trees clipping the camera on spawn
docs: clarify the chunk AoI ring in networking.md
release: v0.21.0
```

### DCO sign-off vs the agent trailer

Two trailers, different purposes; do not confuse them.

- DCO `Signed-off-by` is mandatory. Every commit needs a `Signed-off-by: Name <email>` line certifying the Developer Certificate of Origin (`CONTRIBUTING.md`). Add it with `git commit -s`. Pull requests whose commits are not signed off cannot merge.
- `Co-Authored-By: Claude ...` is the separate agent attribution trailer that this repo's automated harness appends to its own commits (per the harness rules). It is attribution, not the DCO certification, and does not substitute for the `Signed-off-by` line.

A commit authored through the agent harness carries both lines.

## Invariants this doc does not own

These are canonical in `CLAUDE.md`; do not re-derive or contradict them here:

- Singleplayer==multiplayer: gameplay logic stays in shared modules and flows through `ClientMessage`/`ServerMessage` + `GameServer`. No separate SP gameplay implementation.
- Gameplay never pauses: overlays gate local controls via `gameplay_accepts_controls`, never `gameplay_simulation_allowed`. See `docs/gameplay-gating.md`.
- Replicated state: per-entity authoritative state ships through Lightyear per-component replication (a `HashMap` on `GameServer` + an ECS mirror entity), never a `ServerMessage` snapshot; every spawn attaches `ReplicationGroup::new_from_entity()`; reconciliation systems are event-driven (`Added` / `RemovedComponents`), not full-query polling. See `docs/replication.md`.
- Balance constants live in `src/game_balance.rs` (restated above for the re-export idiom).

## Related docs

- `CLAUDE.md` - canonical invariants this doc links back to (SP==MP, gameplay-never-pauses, replicated-state, balance constants, clean-code rules).
- `docs/build-and-dev.md` - the full `./cli` surface, feature flags, build prerequisites (libopus, CMake workaround).
- `docs/replication.md` - the per-component replication pattern and the procedure for adding a new replicated entity/component.
- `docs/gameplay-gating.md` - the control-gating mechanism the never-pauses invariant uses.
- `docs/architecture.md` - the module map and Bevy app wiring the module-split philosophy applies to.
