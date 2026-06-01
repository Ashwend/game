import type { User } from '@workos-inc/authkit-react'

export type NamedUser = Pick<User, 'firstName' | 'lastName' | 'email'>

/** Best human-readable name for greetings: first name, else the email. */
export function displayName(user: NamedUser): string {
  const first = user.firstName?.trim()
  return first !== undefined && first.length > 0 ? first : user.email
}

/** Up to two uppercase initials for the avatar fallback. */
export function initials(user: NamedUser): string {
  const first = user.firstName?.trim() ?? ''
  const last = user.lastName?.trim() ?? ''
  if (first.length > 0 && last.length > 0) {
    return `${first[0] ?? ''}${last[0] ?? ''}`.toUpperCase()
  }
  if (first.length > 0) return first.slice(0, 2).toUpperCase()
  const handle = user.email.split('@')[0] ?? user.email
  return handle.slice(0, 2).toUpperCase()
}
