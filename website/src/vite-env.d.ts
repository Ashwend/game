/// <reference types="vite/client" />

// All of these are PUBLIC, build-time-inlined values — the site is static and
// has no backend, so there are no secrets here. The WorkOS client id is meant
// to be public; the API key/secret never touches this project.
interface ImportMetaEnv {
  readonly VITE_WORKOS_CLIENT_ID?: string
  readonly VITE_WORKOS_REDIRECT_URI?: string
  readonly VITE_WORKOS_API_HOSTNAME?: string
  readonly VITE_DISCORD_INVITE_URL?: string
  readonly VITE_GITHUB_REPO_URL?: string
  readonly VITE_SITE_URL?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
