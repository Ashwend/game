import { ArrowRight } from 'lucide-react'
import { ROADMAP } from '#/data/content'

export function Status() {
  return (
    <section className="scroll-mt-16 border-t border-white/5 bg-ink-950/40">
      <div className="mx-auto grid max-w-6xl gap-12 px-5 py-24 sm:px-8 sm:py-28 lg:grid-cols-2 lg:gap-16">
        <div>
          <p className="text-xs font-medium uppercase tracking-[0.22em] text-ember-300">
            The honest version
          </p>
          <h2 className="mt-3 text-3xl font-semibold tracking-tight text-fg sm:text-4xl">
            Early, and we&rsquo;re not hiding it
          </h2>
          <p className="mt-5 text-lg leading-relaxed text-muted">
            The core loop isn&rsquo;t finished. What&rsquo;s there is genuinely
            playable, but you&rsquo;ll hit the content ceiling fast. Make your
            starting tools and a furnace and you&rsquo;ve seen most of it.
          </p>
          <p className="mt-4 text-lg leading-relaxed text-muted">
            We patch roughly weekly. The playtest is how that gets better.
          </p>
        </div>

        <div className="rounded-2xl border border-white/8 bg-ink-850/60 p-7 sm:p-8">
          <p className="text-xs font-medium uppercase tracking-[0.22em] text-ember-300">
            Headed next
          </p>
          <ul className="mt-5 space-y-3.5">
            {ROADMAP.map((item) => (
              <li key={item} className="flex items-start gap-3 text-fg/90">
                <ArrowRight
                  className="mt-1 size-4 shrink-0 text-ember-400"
                  aria-hidden="true"
                />
                <span className="text-[15px]">{item}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </section>
  )
}
