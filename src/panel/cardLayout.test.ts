import { describe, expect, it } from 'vitest'
import { PANEL_SCALE } from './berthShape'
import { cardBox, detailCardHeight, detailCardLayout, RING_OUTER_RADIUS } from './cardLayout'

describe('detailCardLayout', () => {
  it('takes its width and its rhythm from the original at standard size', () => {
    const l = detailCardLayout('standard')
    expect(l.width).toBe(250)
    expect(l.padding).toBe(18)
    expect(l.pointerWidth).toBe(20)
    expect(l.pointerHeight).toBe(40)
    expect(l.horizontalGap).toBe(8)
    expect(l.contentSpacing).toBe(14)
    expect(l.rowInternalSpacing).toBe(7)
    expect(l.progressBarHeight).toBe(6)
    expect(l.headerHeight).toBe(19)
    expect(l.rowTextLineHeight).toBe(14)
  })

  it('gives the card the same curve as the rings', () => {
    // (DockLayout.ringDiameter + DockLayout.ringLineWidth) / 2 = 20 at standard.
    expect(RING_OUTER_RADIUS).toBe(20)
    expect(detailCardLayout('standard').cornerRadius).toBe(20)
    expect(detailCardLayout('large').cornerRadius).toBeCloseTo(20 * PANEL_SCALE.large, 6)
  })

  it('scales every font with the card, not just the frame', () => {
    // The original shipped a version where the width followed the scale and the
    // type did not; at Small a 205pt card truncated its own title.
    for (const name of ['small', 'standard', 'large'] as const) {
      const l = detailCardLayout(name)
      const s = PANEL_SCALE[name]
      expect(l.titleFontSize).toBeCloseTo(14 * s, 6)
      expect(l.rowFontSize).toBeCloseTo(11.5 * s, 6)
      expect(l.footnoteFontSize).toBeCloseTo(11 * s, 6)
    }
  })
})

describe('detailCardHeight', () => {
  const l = detailCardLayout('standard')

  it('is padding and a header when there are no limits to show', () => {
    expect(detailCardHeight(l, 0)).toBe(l.padding * 2 + l.headerHeight)
  })
  it('reserves the original detailed header, value rows and history before hover',()=>{
    const basic=detailCardLayout('standard'), detailed=detailCardLayout('standard',false,true)
    expect(detailed.activityHeight).toBe(231)
    expect(detailCardHeight(detailed,3)-detailCardHeight(basic,3)).toBe(17+3*21+14+231)
    expect(cardBox('top',detailed,3,false,true).height).toBe(detailCardHeight(detailed,3)+20+27)
  })

  it('adds a row per limit plus the gaps between them', () => {
    const row = l.rowTextLineHeight * 2 + l.rowInternalSpacing * 2 + l.progressBarHeight
    expect(detailCardHeight(l, 1)).toBe(detailCardHeight(l, 0) + l.contentSpacing + row)
    expect(detailCardHeight(l, 2)).toBe(detailCardHeight(l, 0) + l.contentSpacing + row * 2 + l.contentSpacing)
  })

  // An extension's figures are drawn in the same card but are not limits: each
  // is one line of text with no bar, and only the ones carrying a series add a
  // chart. The frame is worked out before a card opens, so this arithmetic is
  // what keeps a series from falling outside the outline.
  it('budgets an extension metric as a line, and its series as a chart', () => {
    expect(l.metricChartHeight).toBe(30)
    const one = detailCardHeight(l, 1, 1)
    expect(one).toBe(detailCardHeight(l, 1) + l.contentSpacing + l.rowTextLineHeight)
    const trended = detailCardHeight(l, 1, 1, 1)
    expect(trended).toBe(one + l.metricChartHeight + l.rowInternalSpacing)
    // Two metrics, no limits: both lines and one gap between them.
    expect(detailCardHeight(l, 0, 2)).toBe(
      l.padding * 2 + l.headerHeight + l.contentSpacing + l.rowTextLineHeight * 2 + l.contentSpacing,
    )
  })

  it('counts a metric as content, so an extension with only figures is not drawn as a refusal', () => {
    // A card with no limits reserves room for the message saying so; a card
    // whose body is an extension's figures is content, not that message.
    expect(cardBox('right', l, 0, false, false, false, 1).height).toBe(detailCardHeight(l, 0, 1))
    expect(cardBox('right', l, 0, false, false, false, 1).height).not.toBe(cardBox('right', l, 0).height)
  })
})

describe('cardBox', () => {
  const l = detailCardLayout('standard')
  const body = detailCardHeight(l, 2)

  it('puts the tail beside the body for a rail on the left or right', () => {
    // The outline is authored facing right, and a side card is not turned, so
    // the tail costs width and no height.
    for (const edge of ['left', 'right'] as const) {
      expect(cardBox(edge, l, 2)).toEqual({ width: l.width + l.pointerWidth, height: body })
    }
  })

  it('turns the same outline a quarter turn for a top rail', () => {
    // On its back the tail leaves upwards and the card grows away from the top
    // edge instead of away from a side, so the two sides swap which one the
    // tail is charged to.
    expect(cardBox('top', l, 2)).toEqual({ width: l.width, height: body + l.pointerWidth })
  })

  it('is the only place the tail is charged, so frame and card cannot drift', () => {
    // A card with no limits still has a header; its box is never zero.
    const empty = cardBox('right', l, 0)
    expect(empty.height).toBeGreaterThan(l.padding * 2 + l.headerHeight + l.rowTextLineHeight * 3)
    expect(empty.width).toBe(l.width + l.pointerWidth)
  })

  it('budgets a real balance-only reading as one value row, not a connection error', () => {
    const balance = cardBox('right',l,0,true)
    expect(balance.height).toBe(l.padding*2+l.headerHeight+l.contentSpacing+l.rowTextLineHeight)
    expect(balance.height).toBeLessThan(cardBox('right',l,0).height)
    expect(cardBox('top',l,0,true).height).toBe(balance.height+l.pointerWidth)
  })
})
