// Best-effort OS + form-factor guesses from the browser's user-agent hints.
// Used only to pick a sensible default download and to hint phone visitors
// that Ashwend is a desktop game; every platform stays reachable from the
// "other platforms" row regardless of what these return.

import type { Platform } from '#/data/content'

/** Pure classifier over a lowercased UA/platform hint string. Falls back to
 *  Windows when unsure. */
export function platformFromHint(hint: string): Platform {
  // 'windows', not a bare 'win': 'darwin' contains 'win' and would
  // misclassify any hint that carries the kernel name.
  if (hint.includes('windows')) return 'windows'
  if (
    hint.includes('mac') ||
    hint.includes('darwin') ||
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

/** Phone/tablet visitors can't run the desktop game; used to show a hint, not
 *  to hide anything. (iPads requesting the desktop site present as Macs and
 *  slip through; that's fine for a hint.) */
export function mobileFromHint(hint: string): boolean {
  return (
    hint.includes('iphone') ||
    hint.includes('ipad') ||
    hint.includes('android') ||
    hint.includes('mobile')
  )
}

function browserHint(): string {
  const nav = navigator as Navigator & {
    userAgentData?: { platform?: string }
  }
  return `${nav.userAgentData?.platform ?? ''} ${nav.userAgent}`.toLowerCase()
}

export function detectPlatform(): Platform {
  return platformFromHint(browserHint())
}

export function detectMobile(): boolean {
  return mobileFromHint(browserHint())
}
