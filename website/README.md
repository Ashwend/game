# Ashwend website

Static splash + playtest sign-up for Ashwend. React + TanStack Start, Tailwind,
strict TypeScript. Fully prerendered to static HTML — **no backend** — and
deployed to Cloudflare Pages. Identity is delegated entirely to
[WorkOS AuthKit](https://workos.com) via the browser SDK (`@workos-inc/authkit-react`,
PKCE), so no playtester accounts live on our infrastructure.

## Develop

```bash
npm install
cp .env.example .env   # fill in at least VITE_WORKOS_CLIENT_ID
npm run dev            # http://localhost:3000
```

Without `VITE_WORKOS_CLIENT_ID` the playtest box renders a "not wired up" state;
everything else still works.

## Scripts

| Script              | What it does                              |
| ------------------- | ----------------------------------------- |
| `npm run dev`       | Dev server on :3000                       |
| `npm run build`     | Prerender to static HTML → `dist/client`  |
| `npm run preview`   | Serve the built output                    |
| `npm run test`      | Vitest                                    |
| `npm run typecheck` | `tsc --noEmit` (very strict)              |
| `npm run lint`      | ESLint                                    |
| `npm run check`     | Prettier + typecheck + lint (the CI gate) |

## Environment variables

All are **public** (build-time inlined, no secrets). See `.env.example`.

| Var                        | Purpose                               |
| -------------------------- | ------------------------------------- |
| `VITE_WORKOS_CLIENT_ID`    | WorkOS AuthKit client id (`client_…`) |
| `VITE_WORKOS_REDIRECT_URI` | Login return URL; the site origin     |
| `VITE_WORKOS_API_HOSTNAME` | Optional custom auth domain (prod)    |
| `VITE_DISCORD_INVITE_URL`  | Discord invite                        |
| `VITE_GITHUB_REPO_URL`     | Repo link (footer)                    |
| `VITE_SITE_URL`            | Canonical origin for SEO / Open Graph |

## WorkOS dashboard setup

1. Create an **AuthKit** project; copy the **client id** into `VITE_WORKOS_CLIENT_ID`.
2. **Redirects** → add the site origin(s): `http://localhost:3000` (dev) and the
   production origin (e.g. `https://ashwend.game`). This is where AuthKit returns
   with `?code` — the SDK completes the exchange client-side.
3. Enable the auth methods you want (email + password, Google, Discord OAuth, …).
4. Allow the site origin for the SPA (CORS) so the browser SDK can call WorkOS.

## Deploy — Cloudflare Pages

- **Build command:** `npm run build`
- **Build output directory:** `dist/client` (root directory `website` if the
  Pages project points at the repo root)
- Set the `VITE_*` vars as Pages build environment variables.
- `_redirects` and `_headers` in `public/` ship with the build (SPA fallback +
  caching/security headers).

Or from the CLI: `npx wrangler pages deploy dist/client`.

## How this connects to the game

The same WorkOS user that signs up here logs into the desktop game and is what
dedicated servers authorise. The game uses native OAuth (PKCE + loopback
redirect) to obtain an access-token JWT, and servers verify it offline against
WorkOS' public JWKS endpoint — no secrets on the servers. The game-side work is
tracked separately and is not part of this package.
