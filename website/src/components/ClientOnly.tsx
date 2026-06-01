import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'

interface ClientOnlyProps {
  /** Rendered during prerender + first client paint, then replaced on mount. */
  readonly fallback: ReactNode
  readonly children: ReactNode
}

/**
 * Renders `children` only in the browser, after hydration. The WorkOS SDK
 * touches `window`/`localStorage`, which don't exist during static prerender,
 * so anything that depends on it must live behind this boundary. Server output
 * and the first client render both show `fallback`, so there's no hydration
 * mismatch — the swap happens in an effect.
 */
export function ClientOnly({ fallback, children }: ClientOnlyProps): ReactNode {
  const [mounted, setMounted] = useState(false)
  useEffect(() => {
    setMounted(true)
  }, [])
  return mounted ? children : fallback
}
