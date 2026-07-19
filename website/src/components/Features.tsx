import { Bomb, Flame, Hammer, Mic, Pickaxe, Swords } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { FEATURES } from '#/data/content'
import type { Feature } from '#/data/content'
import { eyebrow } from './ui'

// Icon lookup lives here so content.ts stays free of component imports; the
// `icon` union in content.ts is the contract between the two.
const ICONS: Record<Feature['icon'], LucideIcon> = {
  gather: Pickaxe,
  build: Hammer,
  raid: Bomb,
  pvp: Swords,
  meteor: Flame,
  voice: Mic,
}

export function Features() {
  return (
    <section className="border-t border-white/5 bg-ink-950/40">
      <div className="mx-auto max-w-6xl px-5 py-24 sm:px-8 sm:py-28">
        <div className="max-w-2xl">
          <p className={eyebrow}>In the playtest right now</p>
          <h2 className="mt-3 text-3xl font-semibold tracking-tight text-fg sm:text-4xl">
            What&rsquo;s in the game
          </h2>
          <p className="mt-4 text-lg text-muted">
            Every world runs on an authoritative dedicated server through a full
            day/night cycle. It&rsquo;s still early, and a determined evening or
            two will reach the content ceiling, but all of this is in and
            playable today.
          </p>
        </div>

        <ul className="mt-12 grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {FEATURES.map((feature) => {
            const Icon = ICONS[feature.icon]
            return (
              <li
                key={feature.title}
                className="rounded-2xl border border-white/10 bg-ink-850/60 p-6"
              >
                <span className="inline-flex size-10 items-center justify-center rounded-xl border border-ember-500/25 bg-ember-500/10">
                  <Icon className="size-5 text-ember-300" aria-hidden="true" />
                </span>
                <h3 className="mt-4 font-semibold text-fg">{feature.title}</h3>
                <p className="mt-2 text-sm leading-relaxed text-muted">
                  {feature.body}
                </p>
              </li>
            )
          })}
        </ul>
      </div>
    </section>
  )
}
