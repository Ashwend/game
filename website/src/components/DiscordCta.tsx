import { siteConfig } from '#/lib/config'
import { buttonClasses } from './ui'
import { DiscordIcon } from './icons'

export function DiscordCta() {
  return (
    <section className="border-t border-white/5">
      <div className="mx-auto max-w-6xl px-5 py-24 sm:px-8 sm:py-28">
        <div className="relative overflow-hidden rounded-3xl border border-discord/25 bg-gradient-to-br from-discord/20 via-ink-850 to-ink-850 p-10 text-center sm:p-14">
          <div className="absolute inset-0 bg-[radial-gradient(90%_120%_at_50%_-10%,rgba(88,101,242,0.25),transparent_60%)]" />
          <div className="relative mx-auto max-w-2xl">
            <h2 className="text-3xl font-semibold tracking-tight text-fg sm:text-4xl">
              Come hang out while it&rsquo;s being built
            </h2>
            <p className="mt-4 text-lg text-muted">
              The Discord is where playtest builds land, where bugs get
              reported, and where the next patch gets argued about. Pull up a
              chair.
            </p>
            <a
              href={siteConfig.discordInviteUrl}
              target="_blank"
              rel="noreferrer"
              className={`mt-8 ${buttonClasses('discord', 'lg')}`}
            >
              <DiscordIcon className="size-5" />
              Join the Discord
            </a>
          </div>
        </div>
      </div>
    </section>
  )
}
