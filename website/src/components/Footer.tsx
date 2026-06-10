import { siteConfig } from '#/lib/config'
import { DiscordIcon, GitHubIcon } from './icons'

export function Footer() {
  return (
    <footer className="border-t border-white/5 bg-ink-950">
      <div className="mx-auto max-w-6xl px-5 py-12 sm:px-8">
        <div className="flex flex-col gap-8 sm:flex-row sm:items-start sm:justify-between">
          <div className="max-w-sm">
            <span className="wordmark text-base text-fg">Ashwend</span>
            <p className="mt-3 text-sm leading-relaxed text-muted">
              A multiplayer open-world survival game. Gather, craft, build, and
              raid.
            </p>
          </div>

          <nav className="flex flex-col gap-3 text-sm" aria-label="Footer">
            <a
              href="#playtest"
              className="text-muted transition-colors hover:text-fg"
            >
              Join the playtest
            </a>
            <a
              href={siteConfig.discordInviteUrl}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-2 text-muted transition-colors hover:text-fg"
            >
              <DiscordIcon className="size-4" />
              Discord
            </a>
            <a
              href={siteConfig.githubRepoUrl}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-2 text-muted/80 transition-colors hover:text-fg"
            >
              <GitHubIcon className="size-4" />
              Source on GitHub
            </a>
          </nav>
        </div>

        <div className="mt-10 flex flex-col gap-1 border-t border-white/5 pt-6 text-xs text-muted/85">
          <span>
            Ashwend&trade;. Source-available under PolyForm Strict 1.0.0.
          </span>
          <span>
            The name and logo are trademarks, not covered by the code license.
          </span>
        </div>
      </div>
    </footer>
  )
}
