// Central, typed view of the build-time configuration. Everything is read from
// `import.meta.env` (Vite inlines `VITE_*` at build), with sensible fallbacks so
// the site still renders and links work even before WorkOS is wired up.

export interface WorkosConfig {
  readonly clientId: string
  /** Optional; when unset the SDK falls back to the dashboard-configured URI. */
  readonly redirectUri: string | undefined
  /** Optional custom auth domain for production (CNAME'd to WorkOS). */
  readonly apiHostname: string | undefined
}

export interface SiteConfig {
  /** Absolute origin, used to build canonical + Open Graph URLs. */
  readonly siteUrl: string
  readonly discordInviteUrl: string
  readonly githubRepoUrl: string
  /** `null` until a WorkOS client id is provided — drives the "not wired up" UI. */
  readonly workos: WorkosConfig | null
}

const DEFAULT_SITE_URL = 'https://ashwend.game'
const DEFAULT_DISCORD = 'https://discord.gg/gVqTumNb8b'
const DEFAULT_GITHUB = 'https://github.com/Ashwend/game'

function trimTrailingSlash(value: string): string {
  return value.endsWith('/') ? value.slice(0, -1) : value
}

function buildWorkos(): WorkosConfig | null {
  const clientId = import.meta.env.VITE_WORKOS_CLIENT_ID
  if (clientId === undefined || clientId.length === 0) return null
  return {
    clientId,
    redirectUri: import.meta.env.VITE_WORKOS_REDIRECT_URI,
    apiHostname: import.meta.env.VITE_WORKOS_API_HOSTNAME,
  }
}

export const siteConfig: SiteConfig = {
  siteUrl: trimTrailingSlash(import.meta.env.VITE_SITE_URL ?? DEFAULT_SITE_URL),
  discordInviteUrl: import.meta.env.VITE_DISCORD_INVITE_URL ?? DEFAULT_DISCORD,
  githubRepoUrl: import.meta.env.VITE_GITHUB_REPO_URL ?? DEFAULT_GITHUB,
  workos: buildWorkos(),
}

/** Absolute URL helper for canonical/OG tags. `path` should start with `/`. */
export function absoluteUrl(path: string): string {
  return `${siteConfig.siteUrl}${path}`
}
