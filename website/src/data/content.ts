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

// Built straight from the release matrix in `.github/workflows/release.yml`. The
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
    asset: 'ashwend-x86_64-pc-windows-msvc.zip',
  },
  {
    platform: 'macos',
    os: 'macOS',
    arch: 'Apple Silicon',
    short: 'macOS',
    asset: 'ashwend-aarch64-apple-darwin.zip',
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
  'Procedurally generated worlds',
  'Open-world',
  'PvP',
  'First-person',
]

export const GALLERY: ReadonlyArray<Shot> = [
  {
    src: '/img/furnace-lit.jpg',
    alt: 'A lit furnace burning beside a workbench, stone hatchet in hand',
  },
  {
    src: '/img/furnace-ui.jpg',
    alt: 'The furnace interface, smelting ore into ingots',
  },
  {
    src: '/img/deploy.jpg',
    alt: 'Placing a deployable in the world, shown as a green outline',
  },
  {
    src: '/img/workbench.jpg',
    alt: 'Crafting at a workbench with a stone hatchet',
  },
  {
    src: '/img/dusk.jpg',
    alt: 'Dusk over the plains, a pine in silhouette',
  },
  {
    src: '/img/sun-vista.jpg',
    alt: 'Sun low over a hazy green plain',
  },
]

export const ROADMAP: ReadonlyArray<string> = [
  'Real terrain: elevation, no more flat world',
  'Water and shorelines shaping an island map',
  'Higher-tier tools and weapons',
  'Proper base building and raiding',
  'A tutorial for new players',
  'A Steam release',
]
