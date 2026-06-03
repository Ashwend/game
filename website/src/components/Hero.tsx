import { ChevronDown } from 'lucide-react'
import { siteConfig } from '#/lib/config'
import { HERO_META } from '#/data/content'
import { buttonClasses } from './ui'
import { DiscordIcon } from './icons'

export function Hero() {
  return (
    <section className="relative flex min-h-[100svh] items-center overflow-hidden">
      {/* Lone-pine hero, captured in-engine. Decorative, described by the page copy. */}
      <div className="absolute inset-0" aria-hidden="true">
        <img
          src="/img/hero.jpg"
          alt=""
          fetchPriority="high"
          className="size-full object-cover object-center"
        />
        <div className="absolute inset-0 bg-gradient-to-t from-ink-900 via-ink-900/45 to-ink-900/20" />
        <div className="absolute inset-0 bg-gradient-to-r from-ink-900/85 via-ink-900/30 to-transparent" />
        <div className="absolute inset-0 bg-[radial-gradient(120%_80%_at_50%_120%,rgba(224,132,51,0.16),transparent_60%)]" />
      </div>

      <div className="relative mx-auto w-full max-w-6xl px-5 pt-24 pb-28 sm:px-8">
        <p className="mb-5 text-xs font-medium uppercase tracking-[0.22em] text-ember-300">
          Survival sandbox · in active development
        </p>

        <h1 className="wordmark bg-gradient-to-b from-[#f6eedd] to-ember-300 bg-clip-text text-[3.25rem] leading-none text-transparent drop-shadow-sm sm:text-7xl lg:text-[8.5rem]">
          Ashwend
        </h1>

        <p className="mt-7 max-w-2xl text-lg text-fg/90 sm:text-xl">
          Drop into a procedurally generated world.
        </p>
        <p className="mt-3 max-w-xl text-base text-muted">
          Gather, craft, build, and raid your way through an open-world survival
          sandbox shaped by the players in it.
        </p>

        <div className="mt-9 flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-center">
          <a
            href="#playtest"
            className={`w-full sm:w-auto ${buttonClasses('primary', 'lg')}`}
          >
            Join the playtest
          </a>
          <a
            href={siteConfig.discordInviteUrl}
            target="_blank"
            rel="noreferrer"
            className={`w-full sm:w-auto ${buttonClasses('discord', 'lg')}`}
          >
            <DiscordIcon className="size-5" />
            Join the Discord
          </a>
        </div>

        <ul className="mt-10 flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-muted">
          {HERO_META.map((item, i) => (
            <li key={item} className="flex items-center gap-3">
              {i > 0 && (
                <span
                  className="size-1 rounded-full bg-ember-500/60"
                  aria-hidden="true"
                />
              )}
              {item}
            </li>
          ))}
        </ul>
      </div>

      <a
        href="#playtest"
        aria-label="Scroll to the playtest sign-up"
        className="absolute inset-x-0 bottom-6 mx-auto flex w-fit animate-bounce text-muted/70 transition-colors hover:text-fg"
      >
        <ChevronDown className="size-6" />
      </a>
    </section>
  )
}
