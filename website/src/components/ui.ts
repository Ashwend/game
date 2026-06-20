// Shared button styling so anchors (`<a>`) and real buttons (`<button onClick>`)
// look identical. Tailwind classes only, no runtime cost.

export type ButtonVariant = 'primary' | 'discord' | 'ghost' | 'subtle'
export type ButtonSize = 'sm' | 'md' | 'lg'

const BASE =
  'inline-flex items-center justify-center gap-2 rounded-full font-medium tracking-tight ' +
  'transition-[background-color,border-color,color,box-shadow,transform] duration-200 ' +
  'select-none whitespace-nowrap disabled:opacity-60 disabled:cursor-not-allowed active:translate-y-px'

const VARIANTS: Record<ButtonVariant, string> = {
  primary:
    'bg-ember-500 text-ink-950 font-semibold shadow-lg shadow-ember-500/20 ' +
    'hover:bg-ember-400 hover:shadow-ember-400/30',
  discord:
    'bg-discord text-white hover:bg-[#6b74f6] shadow-lg shadow-discord/20',
  ghost:
    'border border-white/15 text-fg hover:bg-white/[0.06] hover:border-white/25',
  subtle: 'text-muted hover:text-fg',
}

const SIZES: Record<ButtonSize, string> = {
  sm: 'text-sm px-4 py-2',
  md: 'text-sm px-5 py-2.5',
  lg: 'text-base px-7 py-3.5',
}

export function buttonClasses(
  variant: ButtonVariant = 'primary',
  size: ButtonSize = 'md',
): string {
  return `${BASE} ${VARIANTS[variant]} ${SIZES[size]}`
}

// Shared "eyebrow" kicker: the small uppercase ember label above each section
// heading. Kept in one place so the sections (and the playtest pill, which adds
// its own pill chrome) use one identical letter-spacing and ember shade.
export const eyebrow =
  'text-xs font-medium uppercase tracking-[0.22em] text-ember-300'
