import type { CSSProperties } from 'react'

export type UsageBarStyle = 'continuous' | 'segmented'

/**
 * A limit bar, in the original's two styles.
 *
 * `continuous` is the plain six-point fill. `segmented` is twelve points tall
 * and made of equal capsules with a gap between them — the original rounds the
 * lit tick count so a bar never claims more progress than it has, and never
 * loses the first tick at any non-zero fraction. `width` is the bar's inside
 * width in CSS pixels; without it (the tray menu, whose width the browser
 * owns) the ticks are drawn as a repeating gradient instead.
 */
export function UsageBar({
  fraction,
  tint,
  style = 'continuous',
  height,
  width,
  className = '',
}: {
  fraction: number
  tint: string
  style?: UsageBarStyle
  height: number
  width?: number
  className?: string
}) {
  const clamped = Math.max(0, Math.min(1, Number.isFinite(fraction) ? fraction : 0))
  if (style === 'segmented') {
    if (width == null) {
      return (
        <div
          className={`usage-bar usage-bar--segmented ${className}`}
          style={{ height }}
          aria-hidden="true"
        >
          <span style={{ width: `${clamped * 100}%` }} />
        </div>
      )
    }
    const scale = height / 12
    const tickWidth = 2.5 * scale
    const tickGap = 2 * scale
    const count = Math.max(Math.floor((width + tickGap) / (tickWidth + tickGap)), 1)
    const lit = Math.min(
      count,
      Math.max(clamped > 0 ? 1 : 0, Math.round(clamped * count)),
      clamped < 1 ? count - 1 : count,
    )
    return (
      <div
        className={`usage-bar usage-bar--segmented ${className}`}
        style={{ height, display: 'flex', gap: tickGap }}
        aria-hidden="true"
      >
        {Array.from({ length: count }, (_, index) => (
          <span
            key={index}
            style={{
              width: tickWidth,
              height: '100%',
              borderRadius: tickWidth / 2,
              background: index < lit ? tint : 'var(--panel-track)',
              flex: '0 0 auto',
            }}
          />
        ))}
      </div>
    )
  }
  const rounded: CSSProperties = { height, borderRadius: height / 2 }
  return (
    <div
      className={`usage-bar ${className}`}
      style={{ ...rounded, overflow: 'hidden', background: 'var(--panel-track)' }}
      aria-hidden="true"
    >
      <span
        className="card__bar-fill"
        style={{
          display: 'block',
          height: '100%',
          width: `${clamped * 100}%`,
          background: tint,
          borderRadius: height / 2,
        }}
      />
    </div>
  )
}
