// All marketing copy + asset references in one typed place, so the section
// components stay layout-only.

export interface Shot {
  readonly src: string
  readonly alt: string
}

export const META = {
  title: 'Ashwend, multiplayer open-world survival',
  description:
    'Ashwend is a multiplayer open-world survival game. Drop into a procedurally generated world, gather, craft, build, and raid. Join the early playtest.',
  ogImage: '/img/og.jpg',
} as const

export const HERO_META: ReadonlyArray<string> = [
  'Procedurally generated worlds',
  'Open-world PvP',
  'Patched weekly',
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
