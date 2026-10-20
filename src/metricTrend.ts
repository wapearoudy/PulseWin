import type { UsageMetric, UsageMetricPoint } from './types'

/**
 * A user extension's figures, and the series behind them.
 *
 * Port of the original's `Usage/UsageMetric.swift` and the geometry in
 * `Panel/UsageDetailCard.swift`'s `MetricTrendLine`.
 *
 * Nothing here is translated, converted, averaged or judged. A metric has no
 * ceiling, so it cannot drive a ring; and a lower figure is better for a first
 * token's time and worse for an output rate, and only the program knows which
 * this is — which is why a line has one colour whichever way it moves.
 */

/** The original's `UsageMetric.number`: two decimals, or two significant digits below 0.01. */
export function metricNumber(figure: number): string {
  if (!Number.isFinite(figure)) return '—'
  // A figure that is not zero but would print as "0" says something the program
  // never said, so it keeps two significant digits instead.
  if (figure !== 0 && Math.abs(figure) < 0.01) {
    return new Intl.NumberFormat('zh-CN', { maximumSignificantDigits: 2 }).format(figure)
  }
  return new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 2 }).format(figure)
}

/**
 * The original's `UsageMetric.text(for:)`: a percent sign sits against its
 * number, any other unit is a word and gets a space, and a unitless figure is a
 * plain count.
 */
export function metricText(figure: number, unit?: string | null): string {
  const number = metricNumber(figure)
  if (!unit) return number
  return unit === '%' ? number + unit : `${number} ${unit}`
}

/**
 * The point on the series a reader is nearest to, for a hover readout.
 *
 * The original asks `ChartHoverOverlay` the same question: the nearest sample
 * in x, not a value interpolated between two samples the program never stated.
 */
export function nearestPoint(points: TrendPoint[], x: number): number | null {
  if (points.length === 0) return null
  let nearest = 0
  for (let index = 1; index < points.length; index++) {
    if (Math.abs(points[index].x - x) < Math.abs(points[nearest].x - x)) nearest = index
  }
  return nearest
}

/** The original's `UsageMetric.hasTrend`: two points and time actually moving. */
export function hasTrend(metric: UsageMetric): boolean {
  const history = metric.history ?? []
  if (history.length < 2) return false
  const first = Date.parse(history[0].at)
  const last = Date.parse(history[history.length - 1].at)
  return Number.isFinite(first) && Number.isFinite(last) && last > first
}

export interface TrendPoint {
  x: number
  y: number
}

/**
 * The original's `MetricTrendLine.location`: across by time between the first
 * and the last point, up by value between the series' own lowest and highest.
 *
 * Placed by time rather than by index, so a gap in the program's points reads
 * as a gap instead of being closed up into an even line it never reported. A
 * series that never moves is drawn flat through the middle. `inset` is the room
 * the line's ends and the newest point's mark need inside the frame.
 */
export function trendPoints(
  history: UsageMetricPoint[],
  width: number,
  height: number,
  lineWidth = 1.5,
): TrendPoint[] {
  if (history.length === 0) return []
  const start = Date.parse(history[0].at)
  const span = Date.parse(history[history.length - 1].at) - start
  if (!Number.isFinite(start) || !Number.isFinite(span) || span <= 0) return []
  const values = history.map((point) => point.value).filter((value) => Number.isFinite(value))
  if (values.length === 0) return []
  const low = Math.min(...values)
  const high = Math.max(...values)
  const inset = lineWidth * 2
  const plotWidth = Math.max(width - inset * 2, 1)
  const plotHeight = Math.max(height - inset * 2, 1)
  return history.map((point) => {
    const x = inset + (plotWidth * (Date.parse(point.at) - start)) / span
    const y = high > low
      ? inset + plotHeight * (1 - (point.value - low) / (high - low))
      : height / 2
    return { x, y }
  })
}

/**
 * What a screen reader says for a line: the figure, then its lowest and highest
 * point — the original's `MetricTrendLine.summary`.
 */
export function metricSummary(metric: UsageMetric): string {
  const values = (metric.history ?? []).map((point) => point.value).filter((value) => Number.isFinite(value))
  if (values.length === 0) return metricText(metric.value, metric.unit)
  return `${metricText(metric.value, metric.unit)}。最低 ${metricText(Math.min(...values), metric.unit)}，最高 ${metricText(Math.max(...values), metric.unit)}。`
}

/**
 * The original's `Self.time`: minute precision, and the date only when the point
 * is not from today.
 */
export function metricTime(at: string, now = new Date()): string {
  const date = new Date(at)
  if (!Number.isFinite(date.getTime())) return '—'
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const sameDay =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate()
  return sameDay ? `${hours}:${minutes}` : `${date.getMonth() + 1}月${date.getDate()}日 ${hours}:${minutes}`
}

/**
 * How many of an extension's series the panel card may draw.
 *
 * The original's `DetailCardLayout.metricTrendLimit`: a trend under a metric is
 * a limit row's height, so four of them beside a few limits would run past the
 * height the window was sized for. The rest show their figure alone; Settings
 * draws every series.
 */
export const METRIC_TREND_LIMIT = 2

/**
 * Which of a reading's metrics draw their series in the card: the first few that
 * carry one, in the order the extension stated them — the original's
 * `trendedMetrics`.
 */
export function cardMetricTrends(metrics: readonly UsageMetric[] | null | undefined): boolean[] {
  const list = metrics ?? []
  let drawn = 0
  return list.map((metric) => {
    if (drawn >= METRIC_TREND_LIMIT || !hasTrend(metric)) return false
    drawn += 1
    return true
  })
}
