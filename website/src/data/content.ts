// All marketing copy + asset references in one typed place, so the section
// components stay layout-only.

/** One tile in the "in the playtest right now" section. `icon` keys into the
 *  icon map in Features.tsx so this file stays free of component imports. */
export interface Feature {
  readonly icon: 'gather' | 'build' | 'raid' | 'pvp' | 'meteor' | 'voice'
  readonly title: string
  readonly body: string
}

/** The OS families we ship desktop builds for. Used to pick a sensible default
 *  download from the visitor's browser. */
export type Platform = 'windows' | 'macos' | 'linux'

/** A downloadable build. `asset` is the release artifact filename uploaded by
 *  the release workflow; the rest are the human labels shown in the UI. */
export interface Download {
  readonly platform: Platform
  /** Primary label, e.g. "Windows", "macOS", "Linux". */
  readonly os: string
  /** Secondary label, e.g. "Apple Silicon", "Intel / AMD · 64-bit". */
  readonly arch: string
  /** Compact label for the "other platforms" row, e.g. "Linux (ARM)". */
  readonly short: string
  readonly asset: string
}

// Built straight from the release matrix in `.github/workflows/release.yml`,
// whose source of truth is `.github/scripts/release_assets.py`; keep the
// `asset` values below in sync with it when an asset is renamed. The
// asset names carry Rust target triples (e.g. `x86_64-unknown-linux-gnu`); the
// "unknown" there is the triple's vendor field, not a mistake; we just show
// friendly OS/arch labels here instead of the raw filename. For a detected
// platform with more than one build (Linux), the first match wins as the
// default and the rest stay reachable in the "other platforms" row.
export const DOWNLOADS: ReadonlyArray<Download> = [
  {
    platform: 'windows',
    os: 'Windows',
    arch: 'Intel / AMD · 64-bit',
    short: 'Windows',
    // Per-user Inno Setup installer (Start Menu + optional desktop shortcut +
    // uninstaller). The bare `...-msvc.zip` is still published as the
    // self-update transport, but the website hands first-time users the
    // installer.
    asset: 'ashwend-x86_64-pc-windows-msvc-setup.exe',
  },
  {
    platform: 'macos',
    os: 'macOS',
    arch: 'Apple Silicon',
    short: 'macOS',
    // Drag-to-Applications .dmg. The `...darwin.zip` is still published for
    // self-update and for the no-prompt `curl | sh install.sh` path.
    asset: 'ashwend-aarch64-apple-darwin.dmg',
  },
  {
    platform: 'linux',
    os: 'Linux',
    arch: 'Intel / AMD · 64-bit',
    short: 'Linux (x86-64)',
    asset: 'ashwend-x86_64-unknown-linux-gnu.tar.gz',
  },
  {
    platform: 'linux',
    os: 'Linux',
    arch: 'ARM · 64-bit',
    short: 'Linux (ARM)',
    asset: 'ashwend-aarch64-unknown-linux-gnu.tar.gz',
  },
]

export const META = {
  title: 'Ashwend, multiplayer open-world survival',
  description:
    'Ashwend is a multiplayer open-world survival game. Drop into a procedurally generated world, gather, craft, build, and raid. Join the early playtest.',
  ogImage: '/img/og.jpg',
} as const

export const HERO_META: ReadonlyArray<string> = [
  'Multiplayer',
  'Open-world',
  'PvP',
  'First-person',
]

// What is actually in the playtest today. Keep this honest and current: every
// line here should describe a shipped mechanic, not a roadmap item.
export const FEATURES: ReadonlyArray<Feature> = [
  {
    icon: 'gather',
    title: 'Gather and craft',
    body: 'Fell pines, mine stone and ore veins, smelt ingots in a furnace, and work up from stone tools to iron.',
  },
  {
    icon: 'build',
    title: 'Build and claim',
    body: 'Raise a timber base, hang code-locked doors, and stake your ground with a tool cupboard claim.',
  },
  {
    icon: 'raid',
    title: 'Raid and defend',
    body: 'Powder bombs, satchel charges, and kegs crack open walls; upkeep and decay keep the map from fossilizing.',
  },
  {
    icon: 'pvp',
    title: 'Fight in first person',
    body: 'Committed melee swings, bows and crossbows, and three armor tiers that change how a fight goes.',
  },
  {
    icon: 'meteor',
    title: 'Meteor showers',
    body: 'Showers streak in over the plains and leave craters of meteorite alloy worth fighting over.',
  },
  {
    icon: 'voice',
    title: 'Proximity voice',
    body: 'Spatial voice chat: talk to whoever is close enough to hear you, and no one else.',
  },
]
