// Central, typed view of the site's public configuration. The site is fully
// static (no backend, no secrets, no environment variables), so these are
// plain constants. Identity is handled entirely inside the desktop game
// (players create their account on first launch), so the website only needs to
// link out to downloads, the source repo, and Discord.

export interface SiteConfig {
  /** Absolute origin, used to build canonical + Open Graph URLs. */
  readonly siteUrl: string
  readonly discordInviteUrl: string
  readonly githubRepoUrl: string
}

export const siteConfig: SiteConfig = {
  siteUrl: 'https://ashwend.game',
  discordInviteUrl: 'https://discord.gg/gVqTumNb8b',
  githubRepoUrl: 'https://github.com/Ashwend/game',
}

/** Absolute URL helper for canonical/OG tags. `path` should start with `/`. */
export function absoluteUrl(path: string): string {
  return `${siteConfig.siteUrl}${path}`
}

/** GitHub releases listing: every build and its patch notes. */
export function releasesUrl(): string {
  return `${siteConfig.githubRepoUrl}/releases`
}

/**
 * Direct download for a release asset from whichever release is newest. GitHub
 * 302-redirects `releases/latest/download/<asset>` to the latest non-prerelease
 * release's matching asset, so these links never need bumping per release.
 */
export function latestDownloadUrl(asset: string): string {
  return `${siteConfig.githubRepoUrl}/releases/latest/download/${asset}`
}
