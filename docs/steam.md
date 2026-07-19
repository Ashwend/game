---
title: Steamworks identifiers and auth notes
owns: The Steamworks package/store identifiers, and the reference notes needed when changing or extending player auth to Steam.
when_to_read: Before changing auth (adding Steam sign-in or ownership checks), or when touching Steamworks packages, the store item, or entitlement grants.
sources:
  - "src/auth/ - AuthMode::Workos (WorkOS JWT verified offline against JWKS) vs AuthMode::NoAuth"
related:
  - "docs/updates-and-distribution.md - release packaging, installers, self-update"
---

# Steamworks identifiers and auth notes

> Recorded 2026-07-18 from the Steamworks package setup output. This doc exists so the identifiers are not lost; the Steam auth path itself is NOT implemented yet.

## App

**App ID `4985910`.** The primary identifier: it is what a server validates a Steam session ticket against, and what every ownership check hangs off. Package entitlements (below) are read relative to this app.

## Packages

| Package | ID | What it is for |
| --- | --- | --- |
| Ashwend Developer Comp | `1729809` | Complimentary grants for the developer and anyone comped. |
| Ashwend for Beta Testing | `1729810` | Beta tester access, granted outside a purchase. |
| Ashwend | `1729811` | The retail package customers buy on the store. |

Other setup from the same run:

- **Publisher auto-grant** added to **Dannie Hansen Consulting ApS**, so publisher accounts receive the app automatically.
- **Store item `0`** created, plus a store package for that store item.

## Not captured yet (fill these in before auth work)

- **Depot IDs.** Needed for build uploads, not for auth.
- **Steamworks publisher Web API key.** Required server side to validate session tickets. Treat as a secret: store it in the environment, never in this repo.

## Why these IDs matter when auth changes

Auth today is WorkOS only. `src/auth/` exposes `AuthMode::Workos` (verifies a WorkOS JWT offline against JWKS) and `AuthMode::NoAuth` (loopback and localhost only, used by singleplayer and `multiplayer-test`). Nothing Steam related exists in the codebase yet.

If Steam sign in is added later, these identifiers are the ones that get used:

- **App ID** identifies the game when the server validates a client's Steam session ticket (the standard flow is the client requesting a ticket, then the server calling the Steamworks Web API `ISteamUser/AuthenticateUserTicket` for that App ID).
- **Package IDs** are how you tell *kinds* of access apart once a user is authenticated: a comp grant (`1729809`), a beta tester (`1729810`), and a retail buyer (`1729811`) are three different entitlements on the same app. Any "is this player allowed in / which build can they run" rule keys off these rather than off the App ID alone.

## Decisions to make before implementing

These are open, and worth settling before touching `src/auth/`:

1. **Identity model.** Does a Steam identity replace the WorkOS identity, link to it, or run as a parallel `AuthMode`? The server currently assumes one identity provider.
2. **Where ownership is checked.** Server side on connect (authoritative, matches the rest of the architecture) versus client side (trivially spoofed, so not suitable on its own).
3. **Whether beta access gates anything in game**, or only controls who can download the build on Steam's side.
