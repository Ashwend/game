# Ashwend website

Static splash + playtest landing page for Ashwend. React + TanStack Start,
Tailwind, strict TypeScript. Fully prerendered to static HTML (**no backend,
no environment variables, no secrets**) and deployed to Cloudflare Pages.

There's no account flow on the site: players download a build and create their
account inside the desktop game on first launch. The site just links out to the
downloads (GitHub Releases), the source repo, and Discord.

## Develop

```bash
npm install
npm run dev   # http://localhost:3000
```

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

## Configuration

The handful of public links (site origin, Discord invite, GitHub repo) are plain
constants in [`src/lib/config.ts`](src/lib/config.ts); edit them there. Download
buttons point at `…/releases/latest/download/<asset>`, which GitHub redirects to
the newest release's asset, so they never need bumping per release. The build
list lives in [`src/data/content.ts`](src/data/content.ts) (`DOWNLOADS`) and
mirrors the release matrix in `.github/workflows/release.yml`.

## Deploy: Cloudflare Pages

- **Build command:** `npm run build`
- **Build output directory:** `dist/client` (root directory `website` if the
  Pages project points at the repo root)
- `_redirects` and `_headers` in `public/` ship with the build (SPA fallback +
  caching/security headers).

Or from the CLI: `npx wrangler pages deploy dist/client`.

## How this connects to the game

Players sign up inside the desktop game, not here. The game uses native OAuth
(PKCE + loopback redirect) to obtain an access-token JWT, and dedicated servers
verify it offline against the identity provider's public JWKS endpoint, with no
secrets on the servers, and nothing for this website to wire up. That work is
tracked in the main game crate, not this package.
