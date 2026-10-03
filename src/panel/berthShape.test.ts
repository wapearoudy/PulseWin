import { describe, expect, it } from 'vitest'
import {
  berthCanvas,
  berthPath,
  berthTransform,
  cornerRadiusAt,
  dockLayout,
  DEFAULT_RAIL_CAPACITY,
  PANEL_SCALE,
  RAIL_SPACING,
} from './berthShape'

/**
 * These pin the invariants the original's own docs state as hard rules. If one
 * of them breaks, the rail folds in on itself or the window frame is measured
 * against a rail that is no longer that long.
 */
describe('dockLayout', () => {
  it('matches the documented standard-size budgets', () => {
    const l = dockLayout()
    expect(l.width).toBe(64)
    expect(l.cornerRadius).toBe(26)
    expect(l.flareHeight).toBe(24)
    expect(l.flareWidth).toBe(38)
    expect(l.verticalPadding).toBe(46)
    expect(l.ringDiameter).toBe(36)
    expect(l.ringLineWidth).toBe(4)
    expect(l.ringToTextSpacing).toBe(6)
    expect(l.percentTextHeight).toBe(16)
    expect(l.percentTextWidth).toBe(38)
    expect(l.itemSpacing).toBe(30)
    expect(l.secondRingDiameter).toBe(26)
    expect(l.secondRingLineWidth).toBe(2.5)
    expect(l.collapsedWidth).toBe(6)
    expect(l.collapsedHeight).toBe(96)
    expect(l.collapsedHitWidth).toBe(20)
    // itemHeight = ringDiameter + ringToTextSpacing + percentTextHeight
    expect(l.itemHeight).toBe(58)
  })

  it('keeps cornerRadius + flareWidth === width in both end styles', () => {
    // "Either way, constrained by cornerRadius + flareWidth <= width, since the
    // corner and the flare share the rail's top and bottom edges. Both styles
    // land on exactly width."
    for (const usesRoundEnds of [false, true]) {
      const l = dockLayout({ usesRoundEnds })
      expect(l.cornerRadius + l.flareWidth).toBeCloseTo(l.width, 10)
    }
  })

  it('clears the flare by 22pt in both end styles', () => {
    // Content laid out above the body's flat top would fall outside the shape
    // and be clipped.
    for (const usesRoundEnds of [false, true]) {
      const l = dockLayout({ usesRoundEnds })
      expect(l.verticalPadding - l.flareHeight).toBeCloseTo(22, 10)
    }
  })

  it('derives 54pt of padding with round ends', () => {
    const l = dockLayout({ usesRoundEnds: true })
    expect(l.cornerRadius).toBe(32)
    expect(l.flareHeight).toBe(32)
    expect(l.flareWidth).toBe(32)
    expect(l.verticalPadding).toBeCloseTo(54, 10)
  })

  it('scales every length with PanelSize', () => {
    for (const [name, scale] of Object.entries(PANEL_SCALE)) {
      const l = dockLayout({ scale })
      expect(l.width, name).toBeCloseTo(64 * scale, 10)
      expect(l.ringDiameter, name).toBeCloseTo(36 * scale, 10)
      expect(l.itemHeight, name).toBeCloseTo(58 * scale, 10)
    }
  })

  it('moves only the gap with RailSpacing', () => {
    const standard = dockLayout()
    for (const [name, multiplier] of Object.entries(RAIL_SPACING)) {
      const l = dockLayout({ spacing: multiplier })
      expect(l.itemSpacing, name).toBeCloseTo(30 * multiplier, 10)
      // Everything else is unchanged: the rings keep their size and the rail
      // grows or shrinks around them.
      expect(l.ringDiameter, name).toBe(standard.ringDiameter)
      expect(l.width, name).toBe(standard.width)
      expect(l.itemHeight, name).toBe(standard.itemHeight)
    }
  })

  it('exposes the rail capacity the window frame is budgeted from', () => {
    expect(DEFAULT_RAIL_CAPACITY).toBe(25)
  })
})

describe('berthPath', () => {
  const layout = dockLayout()

  it('starts on the body top edge and closes', () => {
    const d = berthPath({ width: 64, height: 400, layout })
    expect(d.startsWith(`M ${layout.cornerRadius} ${layout.flareHeight}`)).toBe(true)
    expect(d.trimEnd().endsWith('Z')).toBe(true)
  })

  it('is flush against the screen edge for the full height', () => {
    const d = berthPath({ width: 64, height: 400, layout })
    // The fillet meets the right edge at y=0, runs the full height, then leaves
    // it again. Nothing is rounded on that side.
    expect(d).toContain('64 0')
    expect(d).toContain('L 64 400')
  })

  it('collapses to a sliver of exactly collapsedWidth', () => {
    // At openness 0 the flare vanishes and the corner radius equals the whole
    // width, leaving the rounded-on-one-side sliver — the same shape wound
    // down, not a second one. The original still emits the fillet's `addCurve`
    // call, degenerate, so the invariant to check is the extent, not the
    // absence of a command.
    const d = berthPath({ width: layout.collapsedWidth, height: 200, openness: 0, layout })
    expect(d.startsWith('M 6 0')).toBe(true)

    const xs = [...d.matchAll(/(?:M|L|C|Q) ([-\d.]+)/g)].map((m) => Number(m[1]))
    expect(xs.length).toBeGreaterThan(0)
    expect(Math.min(...xs)).toBe(0)
    expect(Math.max(...xs)).toBeCloseTo(layout.collapsedWidth, 6)
  })

  it('samples a superellipse corner rather than a circular arc', () => {
    const d = berthPath({ width: 64, height: 400, layout })
    const quads = d.match(/Q /g) ?? []
    // Two corners × 48 samples.
    expect(quads.length).toBe(96)
    // A circular arc would have produced 'A' commands on the body outline.
    expect(d).not.toMatch(/\bA /)
  })

  it('draws a capsule rather than a berth when floating', () => {
    const d = berthPath({ width: 64, height: 400, isDocked: false, layout })
    expect(d).toMatch(/\bA /)
    expect(d).not.toContain('Q ')
  })

  it('never folds the flare back past the corner', () => {
    // fw = min(flareWidth, width - r); if flareWidth + r exceeded the width the
    // body's flat top edge would run backwards.
    for (const openness of [0, 0.25, 0.5, 0.75, 1]) {
      const d = berthPath({ width: 64, height: 300, openness, layout })
      for (const m of d.matchAll(/(?:M|L) ([-\d.]+) ([-\d.]+)/g)) {
        const x = Number(m[1])
        expect(x, `openness ${openness}`).toBeGreaterThanOrEqual(0)
        expect(x, `openness ${openness}`).toBeLessThanOrEqual(64)
      }
    }
  })

  it('clamps the flare on a rail shorter than two flares', () => {
    // f = min(flareHeight, h/2), so a very short rail still yields a sane path.
    const d = berthPath({ width: 64, height: 20, layout })
    expect(d.startsWith('M')).toBe(true)
    expect(d.trimEnd().endsWith('Z')).toBe(true)
  })
})

describe('cornerRadiusAt', () => {
  const layout = dockLayout()

  it('runs from the sliver width to the full rail corner', () => {
    expect(cornerRadiusAt(layout, 0)).toBe(layout.collapsedWidth)
    expect(cornerRadiusAt(layout, 1)).toBe(layout.cornerRadius)
    expect(cornerRadiusAt(layout, 0.5)).toBeCloseTo((6 + 26) / 2, 10)
  })
})

describe('edge transforms', () => {
  it('draws the canonical path facing right', () => {
    expect(berthTransform('right', 64, 400)).toBeUndefined()
  })

  it('mirrors for the left edge', () => {
    expect(berthTransform('left', 64, 400)).toBe('translate(64 0) scale(-1 1)')
  })

  it('rotates rather than reflects for the top edge, preserving winding', () => {
    // A reflection would reverse the winding; the original is explicit that top
    // is a quarter turn anticlockwise.
    const t = berthTransform('top', 64, 400)
    expect(t).toContain('rotate(-90)')
    expect(t).not.toContain('scale(-1')
  })

  it('lays the canonical rect on its side for the top edge', () => {
    expect(berthCanvas('right', 64, 400)).toEqual({ width: 64, height: 400 })
    expect(berthCanvas('top', 64, 400)).toEqual({ width: 400, height: 64 })
  })
})
