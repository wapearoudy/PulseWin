import { describe, expect, it } from 'vitest'
import { CLOCK_GAP, CLOCK_LINE_WIDTH, dashArray, ringGeometry } from './ringGeometry'

/** One ring at the shipped standard size: `DockLayout.ringDiameter` 36, line width 4. */
const standard = {
  diameter: 36,
  lineWidth: 4,
  scale: 1,
  hasSecondRing: false,
}

describe('ringGeometry', () => {
  it('puts a 20pt disc inside a standard ring', () => {
    expect(ringGeometry(standard).centreDiameter).toBe(20)
  })

  it('leaves the documented 6pt band between disc and ring', () => {
    // "At standard scale the band between the icon's disc and the ring's inner
    // edge is six points wide."
    const g = ringGeometry(standard)
    const discRadius = g.centreDiameter / 2
    // Strokes are centred on the path, so the ring's inner edge is radius - lineWidth/2.
    const ringInnerRadius = g.ringRadius - standard.lineWidth / 2
    expect(ringInnerRadius - discRadius).toBeCloseTo(6, 10)
  })

  it('rides the activity mark halfway between disc and ring', () => {
    // "halfway between the two, so it touches neither"
    const g = ringGeometry(standard)
    const discRadius = g.centreDiameter / 2
    const ringInnerRadius = g.ringRadius - standard.lineWidth / 2
    expect(g.busyDiameter / 2).toBeCloseTo((discRadius + ringInnerRadius) / 2, 10)
  })

  it('takes that band for the second ring instead', () => {
    // "The second ring takes this band. The mark and the ring would be stroked
    // at the same radius otherwise."
    const g = ringGeometry({ ...standard, hasSecondRing: true })
    // The disc gives up two points a side.
    expect(g.centreDiameter).toBe(16)
    expect(g.busyDiameter).toBe(18)
    expect(g.busyDiameter / 2).toBeGreaterThan(g.centreDiameter / 2)
    expect(g.busyDiameter / 2).toBeLessThan(g.secondRingRadius)
  })

  it('keeps the ring and rail geometry unchanged when the second ring appears', () => {
    // "the ring itself, the rail's width and the ring centres are all
    // unchanged, so nothing outside this circle notices."
    const without = ringGeometry(standard)
    const with2 = ringGeometry({ ...standard, hasSecondRing: true })
    expect(with2.ringRadius).toBe(without.ringRadius)
    expect(with2.secondRingRadius).toBe(13)
  })

  it.each([0.82, 1, 1.22])('keeps the activity and clock ink clear of the usage strokes at %s', scale => {
    const lineWidth = 4 * scale
    const g = ringGeometry({ ...standard, diameter: 36 * scale, lineWidth, scale, hasSecondRing: true })
    const activityHalfStroke = Math.max(lineWidth / 2, 1.5) / 2
    const clockHalfStroke = CLOCK_LINE_WIDTH * scale / 2
    expect(g.busyDiameter / 2 - activityHalfStroke).toBeCloseTo(g.centreDiameter / 2, 10)
    expect(g.busyDiameter / 2 + activityHalfStroke).toBeLessThan(g.secondRingRadius - lineWidth / 2)
    expect(g.clockDiameter / 2 - clockHalfStroke - (g.ringRadius + lineWidth / 2)).toBeCloseTo(CLOCK_GAP * scale, 10)
    expect(g.inkHalf).toBeCloseTo(g.clockDiameter / 2 + clockHalfStroke, 10)
  })

  it('fits the avatar box and its clip within the adjacent stroke', () => {
    const single = ringGeometry(standard)
    const double = ringGeometry({ ...standard, hasSecondRing: true })
    expect(single.robotDiameter).toBeCloseTo(27, 10)
    expect(single.robotRoom).toBe(28)
    expect(double.robotDiameter).toBeCloseTo(17.6, 10)
    expect(double.robotRoom).toBe(18)
  })

  it('clears the usage ring by exactly clockGap', () => {
    // clockDiameter is measured out from the ring's outer edge, and the clock
    // arc is stroked centred, so its inner edge is radius - clockLineWidth/2.
    const g = ringGeometry(standard)
    const ringOuterRadius = g.ringRadius + standard.lineWidth / 2
    const clockInnerRadius = g.clockDiameter / 2 - CLOCK_LINE_WIDTH / 2
    expect(clockInnerRadius - ringOuterRadius).toBeCloseTo(CLOCK_GAP, 10)
    expect(g.clockDiameter).toBe(48)
  })

  it('reports the widest ink so the viewBox can contain it', () => {
    // The clock arc is drawn as an overlay outside the ring's frame; sizing the
    // viewBox from `diameter` would clip it.
    const g = ringGeometry(standard)
    expect(g.inkHalf).toBeCloseTo(g.clockDiameter / 2 + CLOCK_LINE_WIDTH / 2, 10)
    expect(g.inkHalf).toBe(25)
    // The usage ring's own ink still fits inside that.
    expect(g.ringRadius + standard.lineWidth / 2).toBeLessThan(g.inkHalf)
  })

  it('does not scale the centre gap, because the original does not', () => {
    // `UsageRingView.centreGap` is a bare `4` with no `PanelMetrics.scale`,
    // unlike `lineWidth` and `secondRingSqueeze` immediately beside it. So the
    // disc shrinks *faster* than the ring does — 20pt at standard, 14.96pt at
    // small rather than the 16.4pt a proportional reading would predict. Kept
    // faithful rather than tidied: every other ring measurement is taken off
    // this disc, so "fixing" it would move the icon and the activity band on
    // two of the three panel sizes.
    expect(ringGeometry(standard).centreDiameter).toBe(20)

    const small = ringGeometry({
      diameter: 36 * 0.82,
      lineWidth: 4 * 0.82,
      scale: 0.82,
      hasSecondRing: false,
    })
    expect(small.centreDiameter).toBeCloseTo(36 * 0.82 - (4 * 0.82 + 4) * 2, 10)
    expect(small.centreDiameter).toBeCloseTo(14.96, 10)
  })

  it('scales the second-ring squeeze, which the original does scale', () => {
    for (const scale of [0.82, 1, 1.22]) {
      const g = ringGeometry({
        diameter: 36 * scale,
        lineWidth: 4 * scale,
        scale,
        hasSecondRing: true,
      })
      const withoutSecond = ringGeometry({
        diameter: 36 * scale,
        lineWidth: 4 * scale,
        scale,
        hasSecondRing: false,
      })
      // Exactly two scaled points a side.
      expect(withoutSecond.centreDiameter - g.centreDiameter, `scale ${scale}`).toBeCloseTo(
        4 * scale,
        10,
      )
    }
  })

  it('never returns a negative disc', () => {
    // A ring narrower than its own padding must clamp rather than invert.
    const g = ringGeometry({ ...standard, diameter: 4, lineWidth: 4 })
    expect(g.centreDiameter).toBe(0)
  })
})

describe('dashArray', () => {
  it('draws the fraction and leaves the rest as gap', () => {
    const [drawn, gap] = dashArray(0.25, 10).split(' ').map(Number)
    const circumference = 2 * Math.PI * 10
    expect(drawn).toBeCloseTo(circumference * 0.25, 10)
    expect(gap).toBeCloseTo(circumference, 10)
  })

  it('clamps out-of-range fractions rather than wrapping', () => {
    const full = 2 * Math.PI * 10
    expect(dashArray(1.5, 10).split(' ')[0]).toBe(`${full}`)
    expect(dashArray(-1, 10).split(' ')[0]).toBe('0')
  })
})
