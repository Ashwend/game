// All marketing copy + asset references in one typed place, so the section
// components stay layout-only.

export interface Shot {
  readonly src: string
  readonly alt: string
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

// Ordered so the first row mixes light levels: the dark night shots read as
// near-black rectangles at thumbnail size, so they don't get to lead. The
// brighter daylight and open-plains shots take the top row; the night cabin
// and the dimmer UI capture sit in the second.
export const GALLERY: ReadonlyArray<Shot> = [
  {
    src: '/img/gathering.jpg',
    alt: 'Mining stone with a pickaxe, a timber base on the horizon',
  },
  {
    src: '/img/dawn-plains.jpg',
    alt: 'Dawn mist burning off the open plains',
  },
  {
    src: '/img/pine-mist.jpg',
    alt: 'Lone pines standing in the morning fog',
  },
  {
    src: '/img/furnace-lit.jpg',
    alt: 'A lit furnace smelting ore beside a workbench',
  },
  {
    src: '/img/cabin-night.jpg',
    alt: 'A torch-lit timber cabin holding back the night',
  },
  {
    src: '/img/inventory.jpg',
    alt: 'The inventory and crafting menu, mid-session',
  },
]
