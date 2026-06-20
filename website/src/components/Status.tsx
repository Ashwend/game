import { eyebrow } from './ui'

export function Status() {
  return (
    <section className="border-t border-white/5 bg-ink-950/40">
      <div className="mx-auto max-w-3xl px-5 py-24 sm:px-8 sm:py-28">
        <p className={eyebrow}>The honest version</p>
        <h2 className="mt-3 text-3xl font-semibold tracking-tight text-fg sm:text-4xl">
          Early, and we&rsquo;re not hiding it
        </h2>
        <p className="mt-5 text-lg leading-relaxed text-muted">
          The core loop is genuinely playable: gather resources, craft a set of
          tools, smelt ore in a furnace, raise a base, and lock it down with a
          tool cupboard. It&rsquo;s still early, though, and a determined
          evening or two will reach the current content ceiling.
        </p>
        <p className="mt-4 text-lg leading-relaxed text-muted">
          We patch roughly weekly. The playtest is how that gets better.
        </p>
      </div>
    </section>
  )
}
