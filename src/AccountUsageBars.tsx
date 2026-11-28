import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { UsageBar } from './panel/UsageBar'
import { isSpent, tintColour, usageTint } from './panel/tint'
import type { Preferences } from './preferences'
import { quotaTimeFraction } from './quotaPacing'
import { formatReset, type ProviderUsage } from './types'
import { percentFigure } from './usagePresentation'
import './account-usage-bars.css'

export type UsageBarPreferences = Pick<Preferences, 'showsRemaining' | 'usageBarStyle' | 'warningThreshold'>

export function AccountUsageBars({ usage, settings }: { usage: ProviderUsage; settings: UsageBarPreferences }) {
  const list = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)
  const [now, setNow] = useState(Date.now)
  const { showsRemaining, usageBarStyle, warningThreshold } = settings

  useLayoutEffect(() => {
    const element = list.current
    if (!element || usageBarStyle !== 'segmented') return
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width))
    observer.observe(element)
    return () => observer.disconnect()
  }, [usageBarStyle])

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  // The Settings row is wider than the hover card, so it uses the same bar
  // at the scale of the Settings controls.
  const height = (usageBarStyle === 'segmented' ? 12 : 6) * 1.3
  return <div className="account-usage-bars" ref={list}>
    {usage.windows.map((window, index) => {
      const label = window.scope ? `${window.label} · ${window.scope}` : window.label
      const used = window.percentUsed != null && Number.isFinite(window.percentUsed) ? window.percentUsed / 100 : null
      const spent = isSpent(used, window.isExhausted)
      const figure = used === null ? null : percentFigure(used, showsRemaining)
      const caption = showsRemaining ? '剩余' : '已用'
      const tint = usageTint({
        usedFraction: used,
        isExhausted: spent,
        warningAt: warningThreshold,
        elapsedFraction: usage.stale || usage.error ? null : quotaTimeFraction(window, now),
      })
      const reset = formatReset(window.resetsAt, new Date(now))
      const fill = <UsageBar fraction={used === null ? 0 : showsRemaining ? 1 - used : used}
        tint="var(--account-usage-tint)" style={usageBarStyle} height={height} width={width} />
      return <div key={window.id ?? `${label}:${index}`} className="account-usage-row"
        style={{ '--account-usage-tint-light': tintColour(tint, true), '--account-usage-tint-dark': tintColour(tint) } as CSSProperties}>
        <strong>{label}</strong>
        {window.detail && <small>{window.detail}</small>}
        {figure === null ? fill : <div role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100}
          aria-valuenow={figure} aria-valuetext={`${figure}% ${caption}${spent ? ' · 已限额' : ''}`}>{fill}</div>}
        <div className="account-usage-labels">
          <span className={spent ? 'account-usage-exhausted' : undefined}>
            <span title={figure === null ? '服务商未报告额度' : undefined}>{figure === null ? '—' : `${figure}%`}</span>{figure === null ? '' : ` ${caption}`}{spent ? ' · 已限额' : ''}
          </span>
          {reset && <small>{reset}</small>}
        </div>
      </div>
    })}
  </div>
}
