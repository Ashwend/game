import { useEffect, useState } from 'react'
import { ArrowUpRight, Download as DownloadIcon } from 'lucide-react'
import { DOWNLOADS } from '#/data/content'
import type { Platform } from '#/data/content'
import { latestDownloadUrl, releasesUrl } from '#/lib/config'
import { buttonClasses } from './ui'

/**
 * The page's primary call to action. There's no account flow here (players
 * sign up inside the game on first launch), so it's a single, focused download
 * moment: lead with one build for the visitor's detected OS, keep the rest a
 * click away. Fully prerendered (defaults to Windows); the OS guess is a
 * progressive enhancement applied on mount, so no-JS visitors still get every
 * platform.
 */
export function Playtest() {
  return (
    <section
      id="playtest"
      className="relative scroll-mt-16 overflow-hidden bg-ink-900"
    >
      <div className="absolute inset-0 bg-[radial-gradient(60%_50%_at_50%_0%,rgba(224,132,51,0.14),transparent_70%)]" />
      <div className="relative mx-auto max-w-2xl px-5 py-24 text-center sm:px-8 sm:py-32">
        <span className="inline-flex items-center gap-2 rounded-full border border-ember-500/30 bg-ember-500/10 px-3 py-1 text-xs font-medium uppercase tracking-wider text-ember-200">
          <span className="relative flex size-2">
            <span className="absolute inline-flex size-full animate-ping rounded-full bg-ember-400/70" />
            <span className="relative inline-flex size-2 rounded-full bg-ember-400" />
          </span>
          Early playtest &middot; open now
        </span>

        <h2 className="mt-6 text-4xl font-semibold tracking-tight text-fg sm:text-5xl">
          Jump into the playtest
        </h2>
        <p className="mx-auto mt-5 max-w-xl text-lg leading-relaxed text-muted">
          Ashwend is early and crude, and free to play while it&rsquo;s in
          playtest. There&rsquo;s nothing to sign up for here: you&rsquo;ll
          create your account in the game itself the first time you launch. Just
          grab a build and dive in.
        </p>

        <DownloadCallout />
      </div>
    </section>
  )
}

function DownloadCallout() {
  // Server + first paint render this default so hydration matches; the effect
  // then narrows it to the visitor's actual OS.
  const [platform, setPlatform] = useState<Platform>('windows')
  useEffect(() => setPlatform(detectPlatform()), [])

  const primary = DOWNLOADS.find((build) => build.platform === platform)
  if (primary === undefined) return null
  const others = DOWNLOADS.filter((build) => build !== primary)

  return (
    <div className="mt-10 flex flex-col items-center">
      <a
        href={latestDownloadUrl(primary.asset)}
        target="_blank"
        rel="noreferrer"
        className={buttonClasses('primary', 'lg')}
      >
        <DownloadIcon className="size-5" aria-hidden="true" />
        Download for {primary.os}
      </a>
      <p className="mt-3 text-sm text-muted">
        {primary.arch} &middot; always the latest release &middot; free
      </p>

      <div className="mt-8 flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-sm text-muted">
        <span className="text-muted/85">Also on</span>
        {others.map((build, i) => (
          <span key={build.asset} className="flex items-center gap-2">
            {i > 0 && (
              <span aria-hidden="true" className="text-muted/40">
                &middot;
              </span>
            )}
            <a
              href={latestDownloadUrl(build.asset)}
              target="_blank"
              rel="noreferrer"
              className="text-fg/85 underline-offset-4 transition-colors hover:text-fg hover:underline"
            >
              {build.short}
            </a>
          </span>
        ))}
      </div>

      <a
        href={releasesUrl()}
        target="_blank"
        rel="noreferrer"
        className="mt-6 inline-flex items-center gap-0.5 text-sm font-medium text-ember-300 underline-offset-4 transition-colors hover:text-ember-200 hover:underline"
      >
        Browse all releases
        <ArrowUpRight className="size-3.5" aria-hidden="true" />
      </a>
    </div>
  )
}

/** Best-effort OS guess from the browser. Falls back to Windows when unsure;
 *  every platform is reachable from the "other platforms" row regardless. */
function detectPlatform(): Platform {
  const nav = navigator as Navigator & {
    userAgentData?: { platform?: string }
  }
  const hint =
    `${nav.userAgentData?.platform ?? ''} ${nav.userAgent}`.toLowerCase()
  if (hint.includes('win')) return 'windows'
  if (
    hint.includes('mac') ||
    hint.includes('iphone') ||
    hint.includes('ipad')
  ) {
    return 'macos'
  }
  if (
    hint.includes('android') ||
    hint.includes('linux') ||
    hint.includes('x11')
  ) {
    return 'linux'
  }
  return 'windows'
}
