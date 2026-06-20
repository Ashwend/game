import { siteConfig } from '#/lib/config'
import { buttonClasses } from './ui'
import { DiscordIcon } from './icons'

export function Header() {
  // Fixed, not sticky: sticky would occupy 64px of flow above the 100svh
  // hero and push its bottom edge (and the scroll cue) below the fold.
  return (
    <header className="fixed inset-x-0 top-0 z-50 border-b border-white/5 bg-ink-900/70 backdrop-blur-md">
      <div className="mx-auto flex h-16 max-w-6xl items-center justify-between px-5 sm:px-8">
        <a
          href="#top"
          className="wordmark text-lg text-fg"
          aria-label="Ashwend home"
        >
          Ashwend
        </a>

        <nav className="flex items-center gap-2 sm:gap-6" aria-label="Primary">
          <a
            href="#gallery"
            className="hidden text-sm text-muted transition-colors hover:text-fg sm:block"
          >
            Screens
          </a>
          <a
            href={siteConfig.discordInviteUrl}
            target="_blank"
            rel="noreferrer"
            className="hidden items-center gap-1.5 text-sm text-muted transition-colors hover:text-fg sm:inline-flex"
          >
            <DiscordIcon className="size-4" />
            Discord
          </a>
          <a href="#playtest" className={buttonClasses('primary', 'sm')}>
            Join the playtest
          </a>
        </nav>
      </div>
    </header>
  )
}
