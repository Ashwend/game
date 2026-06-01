import { useCallback, useEffect, useState } from 'react'
import { ChevronLeft, ChevronRight, X } from 'lucide-react'
import { GALLERY } from '#/data/content'

export function Gallery() {
  const [active, setActive] = useState<number | null>(null)

  const close = useCallback(() => setActive(null), [])
  const step = useCallback(
    (dir: 1 | -1) =>
      setActive((cur) =>
        cur === null ? cur : (cur + dir + GALLERY.length) % GALLERY.length,
      ),
    [],
  )

  return (
    <section id="gallery" className="scroll-mt-16 border-t border-white/5">
      <div className="mx-auto max-w-6xl px-5 py-24 sm:px-8 sm:py-28">
        <div className="max-w-2xl">
          <p className="text-xs font-medium uppercase tracking-[0.22em] text-ember-300">
            Screens
          </p>
          <h2 className="mt-3 text-3xl font-semibold tracking-tight text-fg sm:text-4xl">
            From the build
          </h2>
          <p className="mt-4 text-lg text-muted">
            Captured in-engine. It&rsquo;s a prototype, and it already has a
            mood.
          </p>
        </div>

        <div className="mt-12 grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {GALLERY.map((shot, i) => (
            <button
              key={shot.src}
              type="button"
              onClick={() => setActive(i)}
              aria-label={`Open screenshot: ${shot.alt}`}
              className="group relative block aspect-[16/9] cursor-zoom-in overflow-hidden rounded-2xl border border-white/8 bg-ink-850"
            >
              <img
                src={shot.src}
                alt={shot.alt}
                loading="lazy"
                className="size-full object-cover transition-transform duration-500 ease-out group-hover:scale-[1.05]"
              />
              <div className="pointer-events-none absolute inset-0 bg-gradient-to-t from-ink-950/55 to-transparent opacity-0 transition-opacity duration-300 group-hover:opacity-100" />
            </button>
          ))}
        </div>
      </div>

      {active !== null && (
        <Lightbox
          index={active}
          onClose={close}
          onPrev={() => step(-1)}
          onNext={() => step(1)}
        />
      )}
    </section>
  )
}

const CLOSE_MS = 200

interface LightboxProps {
  readonly index: number
  readonly onClose: () => void
  readonly onPrev: () => void
  readonly onNext: () => void
}

function Lightbox({ index, onClose, onPrev, onNext }: LightboxProps) {
  // `shown` drives the enter/exit transition; closing animates out first.
  const [shown, setShown] = useState(false)

  useEffect(() => {
    const raf = requestAnimationFrame(() => setShown(true))
    return () => cancelAnimationFrame(raf)
  }, [])

  const requestClose = useCallback(() => {
    setShown(false)
    window.setTimeout(onClose, CLOSE_MS)
  }, [onClose])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') requestClose()
      else if (event.key === 'ArrowLeft') onPrev()
      else if (event.key === 'ArrowRight') onNext()
    }
    window.addEventListener('keydown', onKey)
    // Lock scroll, padding the root to replace the removed scrollbar so the
    // page width stays constant (no flicker).
    const root = document.documentElement
    const scrollbarWidth = window.innerWidth - root.clientWidth
    const previousOverflow = root.style.overflow
    const previousPaddingRight = root.style.paddingRight
    root.style.overflow = 'hidden'
    if (scrollbarWidth > 0) root.style.paddingRight = `${scrollbarWidth}px`
    return () => {
      window.removeEventListener('keydown', onKey)
      root.style.overflow = previousOverflow
      root.style.paddingRight = previousPaddingRight
    }
  }, [requestClose, onPrev, onNext])

  const shot = GALLERY[index]
  if (shot === undefined) return null

  const navButton =
    'pointer-events-auto flex size-11 items-center justify-center rounded-full bg-white/5 text-fg/80 ring-1 ring-white/10 backdrop-blur transition hover:bg-white/10 hover:text-fg'

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={shot.alt}
      onClick={requestClose}
      className={`fixed inset-0 z-[100] flex items-center justify-center bg-ink-950/90 p-4 backdrop-blur-sm transition-opacity duration-200 ease-out sm:p-8 ${
        shown ? 'opacity-100' : 'opacity-0'
      }`}
    >
      <button
        type="button"
        onClick={requestClose}
        aria-label="Close"
        className="absolute right-4 top-4 z-10 flex size-11 items-center justify-center rounded-full bg-white/5 text-fg/80 ring-1 ring-white/10 backdrop-blur transition hover:bg-white/10 hover:text-fg"
      >
        <X className="size-5" />
      </button>

      <figure
        onClick={(event) => event.stopPropagation()}
        className={`max-h-full transition-transform duration-200 ease-out ${
          shown ? 'scale-100' : 'scale-95'
        }`}
      >
        <img
          key={shot.src}
          src={shot.src}
          alt={shot.alt}
          className="mx-auto max-h-[72vh] w-auto max-w-full animate-lb-img rounded-lg shadow-2xl shadow-black/60 sm:max-h-[82vh]"
        />
        <figcaption className="mt-3 text-center text-sm text-muted">
          {shot.alt}
          <span className="text-muted/60">
            {' '}
            · {index + 1} / {GALLERY.length}
          </span>
        </figcaption>
      </figure>

      {/* Nav: bottom-centered on every size, clear of the top-right close
          button. The wrapper ignores pointer events so the backdrop still
          closes on tap; only the buttons are interactive. */}
      <div className="pointer-events-none absolute inset-x-0 bottom-6 flex items-center justify-center gap-4">
        <button
          type="button"
          aria-label="Previous screenshot"
          onClick={(event) => {
            event.stopPropagation()
            onPrev()
          }}
          className={navButton}
        >
          <ChevronLeft className="size-6" />
        </button>
        <button
          type="button"
          aria-label="Next screenshot"
          onClick={(event) => {
            event.stopPropagation()
            onNext()
          }}
          className={navButton}
        >
          <ChevronRight className="size-6" />
        </button>
      </div>
    </div>
  )
}
