import { AuthKitProvider, useAuth } from '@workos-inc/authkit-react'
import type { User } from '@workos-inc/authkit-react'
import { CheckCircle2, LogOut, ShieldCheck } from 'lucide-react'
import { siteConfig } from '#/lib/config'
import { displayName, initials } from '#/lib/user'
import { ClientOnly } from './ClientOnly'
import { buttonClasses } from './ui'
import { DiscordIcon } from './icons'

/**
 * Playtest sign-up — the page's primary call to action. Identity is fully
 * delegated to WorkOS AuthKit via the browser SDK (PKCE, no backend), so this
 * widget must only run client-side; the marketing copy around it is static and
 * prerendered for SEO. See [[workos-playtest-auth]] for the wider plan.
 */
export function Playtest() {
  const { workos, discordInviteUrl } = siteConfig

  return (
    <section
      id="playtest"
      className="relative scroll-mt-16 overflow-hidden bg-ink-900"
    >
      <div className="absolute inset-0 bg-[radial-gradient(70%_60%_at_50%_0%,rgba(224,132,51,0.12),transparent_65%)]" />
      <div className="relative mx-auto max-w-3xl px-5 py-24 sm:px-8 sm:py-28">
        <div className="relative overflow-hidden rounded-3xl border border-ember-500/25 bg-ink-850/80 p-8 shadow-2xl shadow-black/40 backdrop-blur-sm sm:p-12">
          <div className="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-ember-400/50 to-transparent" />

          <span className="inline-flex items-center gap-2 rounded-full border border-ember-500/30 bg-ember-500/10 px-3 py-1 text-xs font-medium uppercase tracking-wider text-ember-200">
            <span className="relative flex size-2">
              <span className="absolute inline-flex size-full animate-ping rounded-full bg-ember-400/70" />
              <span className="relative inline-flex size-2 rounded-full bg-ember-400" />
            </span>
            Early playtest &middot; open now
          </span>

          <h2 className="mt-5 text-3xl font-semibold tracking-tight text-fg sm:text-4xl">
            Get your hands on it
          </h2>
          <p className="mt-4 text-lg leading-relaxed text-muted">
            Ashwend is early and crude, and we want players in it. Create a
            playtest account to get access as builds roll out, then bring the
            same login in-game.
          </p>

          <div className="mt-8">
            <ClientOnly fallback={<SignedOutView />}>
              {workos !== null ? (
                <AuthKitProvider
                  clientId={workos.clientId}
                  onRedirectCallback={stripAuthQuery}
                  {...(workos.redirectUri !== undefined
                    ? { redirectUri: workos.redirectUri }
                    : {})}
                  {...(workos.apiHostname !== undefined
                    ? { apiHostname: workos.apiHostname }
                    : {})}
                >
                  <PlaytestAuth discordUrl={discordInviteUrl} />
                </AuthKitProvider>
              ) : (
                <UnconfiguredView discordUrl={discordInviteUrl} />
              )}
            </ClientOnly>
          </div>
        </div>
      </div>
    </section>
  )
}

/** Remove `?code=…&state=…` from the URL after AuthKit completes the redirect. */
function stripAuthQuery(): void {
  window.history.replaceState(null, '', `${window.location.pathname}#playtest`)
}

function PlaytestAuth({ discordUrl }: { readonly discordUrl: string }) {
  const { isLoading, user, signIn, signUp, signOut } = useAuth()

  if (isLoading) return <SignedOutView busy />
  if (user !== null) {
    return (
      <SignedInView
        user={user}
        discordUrl={discordUrl}
        onSignOut={() => signOut()}
      />
    )
  }
  return (
    <SignedOutView
      onSignUp={() => void signUp()}
      onSignIn={() => void signIn()}
    />
  )
}

interface SignedOutViewProps {
  readonly onSignUp?: () => void
  readonly onSignIn?: () => void
  readonly busy?: boolean
}

function SignedOutView({
  onSignUp,
  onSignIn,
  busy = false,
}: SignedOutViewProps) {
  return (
    <div aria-busy={busy}>
      <div className="flex flex-col gap-3 sm:flex-row">
        <button
          type="button"
          onClick={onSignUp}
          disabled={busy}
          className={buttonClasses('primary', 'lg')}
        >
          Create playtest account
        </button>
        <button
          type="button"
          onClick={onSignIn}
          disabled={busy}
          className={buttonClasses('ghost', 'lg')}
        >
          I already have one
        </button>
      </div>
      <p className="mt-5 flex items-start gap-2 text-sm text-muted">
        <ShieldCheck
          className="mt-0.5 size-4 shrink-0 text-ember-400"
          aria-hidden="true"
        />
        <span>
          Sign-in is handled end-to-end by{' '}
          <strong className="text-fg/90">WorkOS</strong>. We never see or store
          your password, and it&rsquo;s the same login you&rsquo;ll use in-game.
        </span>
      </p>
    </div>
  )
}

interface SignedInViewProps {
  readonly user: User
  readonly discordUrl: string
  readonly onSignOut: () => void
}

function SignedInView({ user, discordUrl, onSignOut }: SignedInViewProps) {
  return (
    <div>
      <div className="flex items-center gap-4 rounded-2xl border border-white/8 bg-ink-800/60 p-4">
        <Avatar user={user} />
        <div className="min-w-0">
          <p className="flex items-center gap-2 font-medium text-fg">
            <CheckCircle2
              className="size-4 text-ember-400"
              aria-hidden="true"
            />
            You&rsquo;re on the list, {displayName(user)}.
          </p>
          <p className="truncate text-sm text-muted">{user.email}</p>
        </div>
      </div>

      <p className="mt-6 text-[15px] leading-relaxed text-muted">
        You&rsquo;ll get the playtest build through Discord as it goes out. Two
        things worth doing now:
      </p>

      <div className="mt-5 flex flex-col gap-3 sm:flex-row">
        <a
          href={discordUrl}
          target="_blank"
          rel="noreferrer"
          className={buttonClasses('discord', 'lg')}
        >
          <DiscordIcon className="size-5" />
          Join the Discord
        </a>
        <button
          type="button"
          onClick={onSignOut}
          className={buttonClasses('ghost', 'lg')}
        >
          <LogOut className="size-4" />
          Sign out
        </button>
      </div>

      <p className="mt-5 text-sm text-muted/80">
        Sign in with this same account from Ashwend&rsquo;s main menu once
        desktop builds go out.
      </p>
    </div>
  )
}

function Avatar({ user }: { readonly user: User }) {
  if (user.profilePictureUrl !== null) {
    return (
      <img
        src={user.profilePictureUrl}
        alt=""
        className="size-12 shrink-0 rounded-full object-cover ring-1 ring-white/10"
      />
    )
  }
  return (
    <span className="flex size-12 shrink-0 items-center justify-center rounded-full bg-ember-500/15 text-sm font-semibold text-ember-200 ring-1 ring-ember-500/25">
      {initials(user)}
    </span>
  )
}

function UnconfiguredView({ discordUrl }: { readonly discordUrl: string }) {
  return (
    <div>
      <p className="rounded-xl border border-white/10 bg-ink-800/60 p-4 text-sm leading-relaxed text-muted">
        Playtest sign-in isn&rsquo;t wired up in this environment yet. Set
        <code className="mx-1 rounded bg-ink-700 px-1.5 py-0.5 text-ember-200">
          VITE_WORKOS_CLIENT_ID
        </code>
        to enable WorkOS accounts.
      </p>
      <a
        href={discordUrl}
        target="_blank"
        rel="noreferrer"
        className={`mt-5 ${buttonClasses('discord', 'lg')}`}
      >
        <DiscordIcon className="size-5" />
        Join the Discord meanwhile
      </a>
    </div>
  )
}
