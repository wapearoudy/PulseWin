import { memo } from 'react'
import type { Severity } from '../types'

interface Props {
  /** 0..100. `null` renders an empty, muted track. */
  percentUsed: number | null
  severity: Severity
  /** Short text inside the ring, e.g. "5h". */
  label: string
  /** Big number inside the ring; defaults to the rounded percent. */
  value?: string
  size?: number
  strokeWidth?: number
}

const COLORS: Record<Severity, { from: string; to: string }> = {
  ok: { from: '#34d399', to: '#22d3ee' },
  warning: { from: '#fbbf24', to: '#fb923c' },
  critical: { from: '#f87171', to: '#ef4444' },
  unknown: { from: '#64748b', to: '#475569' },
}

/**
 * The ring gauge from Pulse's panel: a rounded progress arc that sweeps
 * clockwise from 12 o'clock, with the "used" percentage in the middle.
 */
function UsageRingImpl({
  percentUsed,
  severity,
  label,
  value,
  size = 76,
  strokeWidth = 7,
}: Props) {
  const radius = (size - strokeWidth) / 2
  const circumference = 2 * Math.PI * radius
  const clamped = percentUsed === null ? 0 : Math.max(0, Math.min(100, percentUsed))
  const dash = (clamped / 100) * circumference
  const colors = COLORS[severity]
  const gradientId = `ring-${severity}-${size}`

  return (
    <div className="provider-ring" style={{ width: size, height: size }}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} aria-hidden="true">
        <defs>
          <linearGradient id={gradientId} x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor={colors.from} />
            <stop offset="100%" stopColor={colors.to} />
          </linearGradient>
        </defs>

        <circle
          className="provider-ring__track"
          cx={size / 2}
          cy={size / 2}
          r={radius}
          strokeWidth={strokeWidth}
          fill="none"
        />

        {clamped > 0 && (
          <circle
            className="provider-ring__value"
            cx={size / 2}
            cy={size / 2}
            r={radius}
            strokeWidth={strokeWidth}
            stroke={`url(#${gradientId})`}
            fill="none"
            strokeLinecap="round"
            strokeDasharray={`${dash} ${circumference - dash}`}
            transform={`rotate(-90 ${size / 2} ${size / 2})`}
          />
        )}
      </svg>

      <div className="provider-ring__center">
        <span className="provider-ring__number">
          {value ?? (percentUsed === null ? '–' : `${Math.round(clamped)}%`)}
        </span>
        <span className="provider-ring__label">{label}</span>
      </div>
    </div>
  )
}

export const UsageRing = memo(UsageRingImpl)
