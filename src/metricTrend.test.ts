import { describe, expect, it } from 'vitest'
import type { UsageMetric } from './types'
import {
  METRIC_TREND_LIMIT,
  cardMetricTrends,
  hasTrend,
  metricNumber,
  metricSummary,
  metricText,
  metricTime,
  nearestPoint,
  trendPoints,
} from './metricTrend'

const metric = (patch: Partial<UsageMetric> = {}): UsageMetric => ({
  id: 'm1',
  label: 'First token',
  value: 320,
  unit: 'ms',
  history: [],
  ...patch,
})

const series = (...values: number[]): UsageMetric['history'] =>
  values.map((value, index) => ({
    at: new Date(2026, 9, 3, 9 + index).toISOString(),
    value,
  }))

describe('metric figures', () => {
  // The original's `UsageMetric.number`: two decimals, and a figure below 0.01
  // that is not zero keeps two significant digits so it does not read as zero.
  it('keeps two decimals and rescues a figure that would round to zero', () => {
    expect(metricNumber(99.234)).toBe('99.23')
    expect(metricNumber(0)).toBe('0')
    expect(metricNumber(0.0042)).toBe('0.0042')
    expect(metricNumber(Number.NaN)).toBe('—')
  })

  it('puts a percent sign against the number and a space before a word', () => {
    expect(metricText(99.246, '%')).toBe('99.25%')
    expect(metricText(320, 'ms')).toBe('320 ms')
    expect(metricText(7, null)).toBe('7')
  })
})

describe('a series is drawn only when it says something', () => {
  // `hasTrend` needs two points and time actually moving: two points at the
  // same timestamp would be a line of zero width, which says nothing.
  it('needs two points and a moving clock', () => {
    expect(hasTrend(metric({ history: series(1) }))).toBe(false)
    expect(hasTrend(metric({ history: [{ at: '2026-10-03T09:00:00Z', value: 1 }, { at: '2026-10-03T09:00:00Z', value: 2 }] }))).toBe(false)
    expect(hasTrend(metric({ history: series(1, 2) }))).toBe(true)
  })

  it('places points by time, not by index, so a gap stays a gap', () => {
    // Three points where the third is an hour after the second but only a
    // minute after the first pair: an index-based line would space them evenly.
    const points = trendPoints(
      [
        { at: '2026-10-03T09:00:00Z', value: 0 },
        { at: '2026-10-03T09:10:00Z', value: 10 },
        { at: '2026-10-03T10:00:00Z', value: 20 },
      ],
      104,
      64,
    )
    expect(points).toHaveLength(3)
    // Inset is 2*lineWidth on each side, so the plot spans 3..101.
    expect(points[0].x).toBeCloseTo(3, 6)
    expect(points[2].x).toBeCloseTo(101, 6)
    // Ten minutes of an hour is a sixth of the way across.
    expect(points[1].x).toBeCloseTo(3 + (98 * 10) / 60, 6)
    // Values 0..20 scale between the bottom of the plot and its top: the lowest
    // point sits on the bottom edge, the highest on the top one.
    expect(points[0].y).toBeCloseTo(61, 6)
    expect(points[2].y).toBeCloseTo(3, 6)
  })

  it('draws a flat series through the middle instead of on an edge', () => {
    const points = trendPoints(series(5, 5, 5), 104, 64)
    expect(points.every((point) => point.y === 32)).toBe(true)
  })

  it('draws nothing when time does not move', () => {
    expect(trendPoints([{ at: '2026-10-03T09:00:00Z', value: 1 }], 100, 60)).toEqual([])
    expect(trendPoints([], 100, 60)).toEqual([])
  })

  it('picks the nearest sample to the pointer rather than interpolating', () => {
    const points = trendPoints(series(1, 2, 3), 104, 64)
    expect(nearestPoint(points, 0)).toBe(0)
    expect(nearestPoint(points, 102)).toBe(2)
    expect(nearestPoint([], 10)).toBeNull()
  })
})

describe('what a reader is told', () => {
  it('reads the figure and then its range', () => {
    expect(metricSummary(metric({ value: 320, history: series(280, 410) }))).toBe(
      '320 ms。最低 280 ms，最高 410 ms。',
    )
  })

  it('falls back to the figure alone when there is no series', () => {
    expect(metricSummary(metric({ value: 99.2, unit: '%' }))).toBe('99.2%')
  })

  it('omits the date for a point from today', () => {
    const now = new Date(2026, 9, 3, 12)
    expect(metricTime(new Date(2026, 9, 3, 9, 5).toISOString(), now)).toBe('09:05')
    expect(metricTime(new Date(2026, 9, 2, 9, 5).toISOString(), now)).toBe('10月2日 09:05')
  })
})

describe('the card draws a bounded number of series', () => {
  // The original's `DetailCardLayout.metricTrendLimit`: a trend under a metric
  // is a limit row's height, so only the first few carry one in the card and
  // Settings draws every series.
  it('keeps the first two that have a series, in the order stated', () => {
    expect(METRIC_TREND_LIMIT).toBe(2)
    const metrics = [
      metric({ id: 'a', history: series(1, 2) }),
      metric({ id: 'b' }),
      metric({ id: 'c', history: series(1, 2) }),
      metric({ id: 'd', history: series(1, 2) }),
    ]
    expect(cardMetricTrends(metrics)).toEqual([true, false, true, false])
    expect(cardMetricTrends(null)).toEqual([])
    expect(cardMetricTrends([metric({ id: 'e' })])).toEqual([false])
  })
})
