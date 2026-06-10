// Responsive <picture> helper. Given a fallback JPEG at `src` and the set of
// pre-generated variant widths, it emits AVIF + WebP <source> srcsets (built
// from the `<base>-<width>.<ext>` naming the build script produces) and falls
// back to the original JPEG for the handful of browsers without AVIF/WebP.
//
// The <picture> itself uses `display: contents` so the rendered <img> is laid
// out by the parent exactly as a bare <img> would be; callers style the image
// through `className` and keep their existing layout untouched.

interface PictureProps {
  /** Fallback JPEG, e.g. "/img/hero.jpg". Variant base is this minus ".jpg". */
  readonly src: string
  readonly alt: string
  /** Pre-generated variant widths, e.g. [768, 1280, 1920]. */
  readonly widths: ReadonlyArray<number>
  /** `sizes` for the responsive srcset, e.g. "100vw". */
  readonly sizes: string
  /** Classes for the rendered <img>. */
  readonly className?: string
  /** Intrinsic aspect-ratio hints; prevents layout shift. */
  readonly width: number
  readonly height: number
  /** Above-the-fold LCP image: load eagerly at high priority. */
  readonly eager?: boolean
}

export function Picture({
  src,
  alt,
  widths,
  sizes,
  className,
  width,
  height,
  eager = false,
}: PictureProps) {
  const base = src.replace(/\.jpg$/, '')
  const srcSet = (ext: string) =>
    widths.map((w) => `${base}-${w}.${ext} ${w}w`).join(', ')

  return (
    <picture className="contents">
      <source type="image/avif" srcSet={srcSet('avif')} sizes={sizes} />
      <source type="image/webp" srcSet={srcSet('webp')} sizes={sizes} />
      <img
        src={src}
        alt={alt}
        width={width}
        height={height}
        className={className}
        decoding="async"
        loading={eager ? 'eager' : 'lazy'}
        fetchPriority={eager ? 'high' : 'auto'}
      />
    </picture>
  )
}
