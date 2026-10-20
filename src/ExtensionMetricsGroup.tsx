import { useEffect, useRef, useState } from 'react'
import type { UsageMetric } from './types'
import { hasTrend, metricSummary, metricText, metricTime, nearestPoint, trendPoints } from './metricTrend'
import './extension-metrics.css'

/**
 * Port of the original's `Settings/ExtensionMetricsGroup.swift`.
 *
 * An extension states a figure and, optionally, the series behind it. Nothing
 * here is translated, converted, averaged or judged: a metric has no ceiling,
 * so it cannot drive a ring, and a lower figure is better for a first token's
 * time and worse for an output rate — only the program knows which this is.
 * That is why the line has one colour whichever way it moves.
 */

const CHART_HEIGHT = 64
const LINE_WIDTH = 1.5

/** One metric's series: the line, the newest point's mark, and a hover readout. */
function MetricSeriesChart({ metric }: { metric: UsageMetric }) {
  const box = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(0)
  const [hover, setHover] = useState<number | null>(null)
  const history = metric.history ?? []

  useEffect(() => {
    const node = box.current
    if (!node) return
    const measure = () => setWidth(node.getBoundingClientRect().width)
    const observer = new ResizeObserver(measure)
    observer.observe(node)
    measure()
    return () => observer.disconnect()
  }, [])

  const points = width > 0 ? trendPoints(history, width, CHART_HEIGHT, LINE_WIDTH) : []
  const label = metric.label
  const readout = hover === null ? null : history[hover]
  const hoverX = hover === null ? 0 : points[hover]?.x ?? 0
  const dot = LINE_WIDTH * 2.4

  return (
    <div className="metric-chart">
      <div
        className="metric-plot"
        ref={box}
        style={{ height: CHART_HEIGHT }}
        onMouseMove={(event) => {
          const rect = event.currentTarget.getBoundingClientRect()
          if (points.length === 0) return
          const found = nearestPoint(points, event.clientX - rect.left)
          if (found !== null) setHover(found)
        }}
        onMouseLeave={() => setHover(null)}
      >
        {points.length >= 2 && (
          <svg
            width={width}
            height={CHART_HEIGHT}
            role="img"
            aria-label={`${label}：${metricSummary(metric)}`}
          >
            <polyline
              className="metric-line"
              points={points.map((point) => `${point.x},${point.y}`).join(' ')}
              strokeWidth={LINE_WIDTH}
              strokeLinecap="round"
              strokeLinejoin="round"
              fill="none"
            />
            <circle cx={points[points.length - 1].x} cy={points[points.length - 1].y} r={dot / 2} className="metric-dot" />
            {hover !== null && points[hover] && (
              <>
                <line className="metric-guide" x1={hoverX} y1={0} x2={hoverX} y2={CHART_HEIGHT} strokeWidth={1} />
                <circle cx={points[hover].x} cy={points[hover].y} r={dot / 2} className="metric-dot" />
              </>
            )}
          </svg>
        )}
        {readout && (
          <span
            className="metric-tooltip"
            style={hoverX > width * 0.65 ? { right: width - hoverX + 8, left: 'auto' } : { left: hoverX + 8 }}
            role="status"
          >
            {metricTime(readout.at)} · {metricText(readout.value, metric.unit)}
          </span>
        )}
      </div>
      <div className="metric-axis">
        <span>{metricTime(history[0].at)}</span>
        <span>{metricTime(history[history.length - 1].at)}</span>
      </div>
    </div>
  )
}

/**
 * The 「指标」 group in an extension account's pane.
 *
 * Drawn only for an account that is switched on and actually reported
 * something, as the original's group is: a heading over an empty box would
 * claim a reading that does not exist.
 */
export function ExtensionMetricsGroup({ metrics, enabled }: { metrics?: UsageMetric[] | null; enabled: boolean }) {
  const shown = metrics ?? []
  if (!enabled || shown.length === 0) return null
  return (
    <section className="group-wrap" aria-label="指标">
      <h2>指标</h2>
      <div className="settings-group">
        {shown.map((metric, index) => (
          <MetricRow key={metric.id} metric={metric} first={index === 0} />
        ))}
      </div>
      <p className="metric-note">
        数值由扩展自己给出，不做换算也不判断好坏。扩展没有上限，所以指标不会驱动圆环。
      </p>
    </section>
  )
}

function MetricRow({ metric, first }: { metric: UsageMetric; first: boolean }) {
  return (
    <>
      {!first && <div className="metric-divider" />}
      <div className="setting-row">
        <div>
          <strong>{metric.label}</strong>
        </div>
        <div className="setting-control">
          <span className="metric-value">{metricText(metric.value, metric.unit)}</span>
        </div>
      </div>
      {hasTrend(metric) && <MetricSeriesChart metric={metric} />}
    </>
  )
}
