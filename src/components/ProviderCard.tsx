import { memo, useEffect, useState } from 'react'
import { credentialHint } from '../api'
import {
  formatReset,
  peakUsed,
  percentLeft,
  severityOf,
  type ProviderUsage,
} from '../types'
import { UsageRing } from './UsageRing'

interface Props {
  provider: ProviderUsage
  /** Ticking clock so "resets in 2h" stays honest without a refetch. */
  now: Date
  expanded: boolean
  onToggle: () => void
}

function ProviderCardImpl({ provider, now, expanded, onToggle }: Props) {
  const peak = peakUsed(provider)
  const severity = severityOf(peak)
  const left = percentLeft(peak)
  const [hintPaths, setHintPaths] = useState<string[] | null>(null)

  // Only ask the backend for search paths when the provider-card is actually broken.
  useEffect(() => {
    if (!provider.error || hintPaths) return
    let cancelled = false
    credentialHint(provider.id)
      .then((paths) => {
        if (!cancelled) setHintPaths(paths)
      })
      .catch(() => {
        if (!cancelled) setHintPaths([])
      })
    return () => {
      cancelled = true
    }
  }, [provider.error, provider.id, hintPaths])

  const headline =
    provider.error !== null
      ? 'Not detected'
      : left === null
        ? 'No usage reported'
        : `${Math.round(left)}% left`

  return (
    <section
      className={`provider-card provider-card--${severity} ${expanded ? 'provider-card--expanded' : ''}`}
      onClick={onToggle}
      role="button"
      tabIndex={0}
      aria-expanded={expanded}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onToggle()
        }
      }}
    >
      <div className="provider-card__head">
        <UsageRing
          percentUsed={peak}
          severity={severity}
          label={provider.windows[0]?.label ?? '—'}
        />

        <div className="provider-card__meta">
          <h3 className="provider-card__name">{provider.name}</h3>
          <p className="provider-card__headline">{headline}</p>
          {provider.plan && <span className="provider-card__plan">{provider.plan}</span>}
          {provider.account && <span className="provider-card__account">{provider.account}</span>}
        </div>
      </div>

      {expanded && (
        <div className="provider-card__body">
          {provider.error ? (
            <>
              <p className="provider-card__error">{provider.error}</p>
              {hintPaths && hintPaths.length > 0 && (
                <>
                  <p className="provider-card__hint-label">PulseWin looked for:</p>
                  <ul className="provider-card__paths">
                    {hintPaths.map((p) => (
                      <li key={p} title={p}>
                        {p}
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </>
          ) : (
            <ul className="windows">
              {provider.windows.map((w) => {
                const sev = severityOf(w.percentUsed)
                const reset = formatReset(w.resetsAt, now)
                return (
                  <li key={w.label} className="window">
                    <div className="window__top">
                      <span className="window__label">{w.label}</span>
                      <span className={`window__pct window__pct--${sev}`}>
                        {w.percentUsed === null ? '—' : `${Math.round(w.percentUsed)}%`}
                      </span>
                    </div>
                    <div className="window__track">
                      <div
                        className={`window__fill window__fill--${sev}`}
                        style={{ width: `${w.percentUsed ?? 0}%` }}
                      />
                    </div>
                    {(reset || w.detail) && (
                      <span className="window__sub">
                        {[reset, w.detail].filter(Boolean).join(' · ')}
                      </span>
                    )}
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      )}
    </section>
  )
}

export const ProviderCard = memo(ProviderCardImpl)
