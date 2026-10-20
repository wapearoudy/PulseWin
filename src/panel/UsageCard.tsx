import { memo, useEffect, useId, useMemo, useState } from 'react'
import { bubblePath, bubbleTransform } from './bubbleShape'
import type { PanelEdge } from './berthShape'
import { cardBox, type DetailCardLayout } from './cardLayout'
import { tintColour, usageTint } from './tint'
import { openSettings } from '../api'
import type { ProviderUsage, UsageMetric } from '../types'
import { percentFigure } from '../usagePresentation'
import { quotaTimeFraction } from '../quotaPacing'
import { forecastText } from '../burnRate'
import { ProviderIcon } from './ProviderIcon'
import { formatReset } from '../types'
import { balanceOnly, creditText } from '../creditAmount'
import { budgetEstimate, useCardHistory } from './cardHistory'
import { CardActivity } from './CardActivity'
import { PanelOutline } from './PanelOutline'
import { formatCost } from '../token-spend/summary'
import { cardMetricTrends, metricSummary, metricText, trendPoints } from '../metricTrend'
import './panel.css'

/**
 * The hover bubble: one provider's limits, opened beside its ring.
 *
 * The card is an **overlay**, never a resize. The original is explicit about
 * this — the panel's frame does not change while a card opens, and card and
 * rail are never stacked in the same layout — which is also why every length
 * here comes from `DetailCardLayout` rather than from anything measured.
 */

export interface UsageCardProps {
  light?: boolean
  provider: ProviderUsage
  layout: DetailCardLayout
  /** The edge the rail is docked to; the tail leaves towards it. */
  edge: PanelEdge
  /** Where the pointer's tip sits along the card's rail-facing side. */
  pointerCentre: number
  /** Top of the card within the window, in CSS pixels. */
  top: number
  warningAt: number
  usesRoundEnds?: boolean
  showsRemaining?: boolean
  tokenSpendEnabled?: boolean
  onRetry: (provider: string) => Promise<void>
  refreshing?: boolean
}

function UsageCardImpl({
  provider,
  light=false,
  layout,
  edge,
  pointerCentre,
  top,
  warningAt,
  showsRemaining = false,
  usesRoundEnds = false,
  tokenSpendEnabled = false,
  onRetry,
  refreshing = false,
}: UsageCardProps) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => { const timer = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(timer) }, [])
  const rows = provider.windows
  const clipId=`card-clip-${useId().replace(/:/g,'')}`
  const history=useCardHistory(provider.id,layout.detailed,tokenSpendEnabled)

  // The box the card lands in, tail included, and the space the outline is
  // drawn in. For a top rail the outline is the same one turned a quarter turn,
  // so its viewBox is expressed in screen coordinates with the tail's band
  // sitting *above* the body rather than beside it.
  // An extension states figures that are not limits: one line each, and the series
  // behind the first few that carry one. The card's frame has to budget for them
  // before anything is drawn, or the last series would fall outside the outline.
  const metrics = provider.metrics ?? []
  const metricTrends = cardMetricTrends(metrics)
  const trendCount = metricTrends.filter(Boolean).length
  const box = cardBox(edge, layout, rows.length, balanceOnly(provider),!!provider.stale,!!provider.error, metrics.length, trendCount)
  const tail = layout.pointerWidth
  const viewBox =
    `0 0 ${box.width} ${box.height}`

  const transform = bubbleTransform(edge, box.height, box.width)

  const d = useMemo(
    () =>
      bubblePath({
        width: box.width,
        height: box.height,
        usesRoundEnds,
        edge,
        pointerCenter: pointerCentre,
        cornerRadius: layout.cornerRadius,
        pointerWidth: layout.pointerWidth,
        pointerHeight: layout.pointerHeight,
      }),
    [layout, box.width, box.height, usesRoundEnds, edge, pointerCentre],
  )

  // The body sits inside the tail: padding, then the pointer's band, on
  // whichever side the rail is.
  const contentInset =
    edge === 'left'
      ? { marginLeft: tail }
      : edge === 'right'
        ? { marginRight: tail }
        : edge === 'bottom' ? { marginBottom: tail } : { marginTop: tail }

  return (
    <div
      className="card"
      style={{
        width: box.width,
        height: box.height,
        top,
      }}
    >
      <svg
        className="card__bubble"
        width={box.width}
        height={box.height}
        viewBox={viewBox}
        aria-hidden="true"
      >
        <defs><clipPath id={clipId}><path d={d} transform={transform}/></clipPath>{light&&<PanelOutline id={`${clipId}-outline`}/>}</defs>
        <path d={d} transform={transform} filter={light?`url(#${clipId}-outline)`:undefined} className="card__surface" />
      </svg>

      <div style={{position:'absolute',inset:0,clipPath:`url(#${clipId})`}}><div
        className="card__content"
        style={{
          padding: layout.padding,
          gap: layout.contentSpacing,
          ...contentInset,
        }}
      >
        <div><header
          className="card__header"
          style={{ height: layout.headerHeight, gap: 8, fontSize: layout.titleFontSize }}
        >
          <span
            className="card__icon"
            style={{ width: layout.headerIconSize, height: layout.headerIconSize }}
            aria-hidden="true"
          >
            <ProviderIcon provider={provider.id}/>
          </span>
          <span className="card__title" title={provider.stale ? `旧读数 · ${new Date(provider.fetchedAt).toLocaleString()}${provider.error ? ' · '+provider.error : ''}` : undefined}>{provider.name}{provider.stale ? ' · 旧读数' : ''}</span>
          {layout.detailed && provider.plan ? (
            <span className="card__plan" style={{ fontSize: layout.footnoteFontSize }}>
              {provider.plan}
            </span>
          ) : null}
        </header>
        {layout.detailed&&<div className="card__muted" style={{height:layout.footnoteHeight,marginTop:layout.headerLineSpacing,marginLeft:layout.headerIconSize+8,fontSize:layout.footnoteFontSize}}>{provider.stale?'':Number.isFinite(Date.parse(provider.fetchedAt))?(now-Date.parse(provider.fetchedAt)<60000?'刚刚更新':`${Math.max(1,Math.floor((now-Date.parse(provider.fetchedAt))/60000))}分钟前更新`):'更新时间未知'}</div>}
        </div>

        {balanceOnly(provider) ? (
          <div className="card__row-line card__balance" style={{fontSize:layout.rowFontSize,lineHeight:`${layout.rowTextLineHeight}px`}}><span>余额</span><span>{creditText(provider.creditRemaining)}</span></div>
        ) : provider.error && rows.length === 0 ? (
          <WhyNoReading
            message={provider.error}
            provider={provider.id}
            layout={layout}
          />
        ) : rows.length === 0 ? (
          <p className="card__message" style={{ fontSize: layout.messageFontSize }}>
            暂无用量读数。
          </p>
        ) : (
          rows.map((w,index) => {
            const fraction =
              w.percentUsed === null || w.percentUsed === undefined ? null : w.percentUsed / 100
            const spent = !!w.isExhausted || (w.percentUsed !== null && w.percentUsed !== undefined && w.percentUsed >= 100)
            const tint = tintColour(usageTint({ usedFraction: fraction, isExhausted: spent, warningAt,elapsedFraction:provider.stale || provider.error?null:quotaTimeFraction(w,Date.now()) }),light)
            const reset = formatReset(w.resetsAt, new Date(now))
            const displayed = fraction === null ? null : showsRemaining ? Math.max(0, 1 - fraction) : fraction

            return (
              <section
                key={w.id ?? `${w.label}:${index}`}
                className="card__row"
                style={{ gap: layout.rowInternalSpacing }}
              >
                <div
                  className="card__row-line"
                  style={{ fontSize: layout.rowFontSize, lineHeight: `${layout.rowTextLineHeight}px` }}
                >
                  <span className="card__row-label" title={w.detail??undefined}>{w.label}{w.scope?` · ${w.scope}`:''}</span>
                </div>

                <div
                  className="card__bar"
                  style={{ height: layout.progressBarHeight, borderRadius: layout.progressBarHeight / 2 }}
                >
                  <div
                    className="card__bar-fill"
                    style={{
                      width: `${Math.max(0, Math.min(1, displayed ?? 0)) * 100}%`,
                      background: tint,
                      borderRadius: layout.progressBarHeight / 2,
                    }}
                  />
                </div>

                <div
                  className="card__row-line"
                  style={{ fontSize: layout.rowFontSize, lineHeight: `${layout.rowTextLineHeight}px` }}
                >
                  <span className="card__percent" style={{ color: spent?tint:light?'var(--panel-text)':'rgba(255,255,255,.9)' }}>
                    {displayed === null ? '—' : `${percentFigure(displayed)}% ${showsRemaining ? '剩余' : '已用'}`}
                  </span>
                  {reset ? (
                    <span className="card__reset" style={{ fontSize: layout.footnoteFontSize }}>
                      {reset}
                    </span>
                  ) : null}
                </div>
                {layout.detailed&&<div className="card__estimate card__muted" style={{height:layout.rowTextLineHeight,fontSize:layout.footnoteFontSize,lineHeight:`${layout.rowTextLineHeight}px`}}>{(()=>{const estimate=history.snapshot?budgetEstimate(w,history.snapshot,now):null;return estimate?`估算额度 ≈${formatCost(estimate.full)} · 已用 ≈${formatCost(estimate.spent)}`:''})()}</div>}
                {layout.showForecast && <div className="card__forecast" style={{height:layout.rowTextLineHeight,fontSize:layout.footnoteFontSize,lineHeight:`${layout.rowTextLineHeight}px`,color:light?'var(--panel-muted)':'rgba(255,255,255,.55)'}}>{spent?'':forecastText(w,now)}</div>}
              </section>
            )
          })
        )}
        {metrics.map((metric, index) => (
          <MetricRow key={metric.id} metric={metric} showsTrend={metricTrends[index]} layout={layout}/>
        ))}
        {provider.stale&&<div className="card__muted card__footnote" style={{height:layout.footnoteHeight,fontSize:layout.footnoteFontSize}}>旧读数 · 截至 {new Date(provider.fetchedAt).toLocaleString()}</div>}
        {provider.error && (rows.length>0 || balanceOnly(provider)) && <WhyNoReading message={provider.error} provider={provider.id} layout={layout} onRetry={()=>onRetry(provider.id)} refreshing={refreshing}/>}
        {layout.detailed&&<CardActivity history={history} layout={layout} now={now}/>}
      </div></div>
    </div>
  )
}


/**
 * One of an extension's figures: the number in the program's own words, and
 * under it the series it came from.
 *
 * **Static, like the days chart above it** — the panel never becomes key, so a
 * hover readout would not fire here; Settings draws the same series with one.
 * Placed by time rather than by index, so a gap in the points stays a gap, and
 * scaled between the series' own lowest and highest because a metric has no
 * ceiling to read against.
 */
function MetricRow({ metric, showsTrend, layout }: { metric: UsageMetric; showsTrend: boolean; layout: DetailCardLayout }) {
  const width = layout.width - layout.padding * 2
  const height = layout.metricChartHeight
  const points = showsTrend ? trendPoints(metric.history ?? [], width, height, 1.5) : []
  return (
    <section className="card__row" style={{ gap: layout.rowInternalSpacing }}>
      <div className="card__row-line" style={{ fontSize: layout.rowFontSize, lineHeight: `${layout.rowTextLineHeight}px` }}>
        <span className="card__row-label">{metric.label}</span>
        <span className="card__metric" style={{ marginLeft: 'auto' }}>{metricText(metric.value, metric.unit)}</span>
      </div>
      {points.length >= 2 && (
        <svg
          width={width}
          height={height}
          role="img"
          aria-label={metric.label}
          aria-description={metricSummary(metric)}
          style={{ display: 'block', overflow: 'visible' }}
        >
          <polyline
            className="card__metric-line"
            points={points.map((point) => `${point.x},${point.y}`).join(' ')}
            strokeWidth={1.5}
            strokeLinecap="round"
            strokeLinejoin="round"
            fill="none"
          />
          <circle
            className="card__metric-dot"
            cx={points[points.length - 1].x}
            cy={points[points.length - 1].y}
            r={(1.5 * 2.4) / 2}
          />
        </svg>
      )}
    </section>
  )
}

export const UsageCard = memo(UsageCardImpl)

/**
 * Why this ring has no reading, and where to go about it.
 *
 * A card that only says "no key" leaves the reader to find the tool's token
 * themselves. The first place PulseWin looked is the useful half of that
 * sentence, and the settings window is the other half — a pasted credential is
 * the route for every tool whose own store this port cannot read.
 */
function WhyNoReading({
  message,
  provider,
  layout,
  onRetry,
  refreshing,
}: {
  message: string
  provider: string
  layout: DetailCardLayout
  onRetry?: () => Promise<void>
  refreshing?: boolean
}) {
  return (
    <div className="card__why" style={{gap:layout.rowInternalSpacing}}>
      <p className={`card__message${onRetry?' card__refresh-error':''}`} title={message} style={{ fontSize: layout.messageFontSize, lineHeight:`${layout.rowTextLineHeight}px`,height:layout.rowTextLineHeight*3,display:'-webkit-box',WebkitLineClamp:3,WebkitBoxOrient:'vertical',overflow:'hidden' }}>
        {message}
      </p>
      <div style={{display:'flex',gap:layout.rowInternalSpacing}}>
      {onRetry && <button type="button" className="card__action" disabled={refreshing} style={{fontSize:layout.footnoteFontSize}} onClick={()=>onRetry().catch(()=>undefined)}>{refreshing?'正在检查…':'重新检查'}</button>}
      <button
        type="button"
        className="card__action"
        style={{ fontSize: layout.footnoteFontSize }}
        onClick={() => {
          openSettings(provider).catch(() => undefined)
        }}
      >
        连接设置…
      </button>
      </div>
    </div>
  )
}
