import { describe, expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { UsageBar } from './UsageBar'

const markup = (fraction: number, style: 'continuous' | 'segmented', width?: number) =>
  renderToStaticMarkup(<UsageBar fraction={fraction} tint="#007aff" style={style} height={style === 'segmented' ? 12 : 6} width={width} />)

describe('UsageBar', () => {
  it('draws a continuous fill at the used fraction', () => {
    const html = markup(0.5, 'continuous')
    expect((html.match(/<span/g) ?? []).length).toBe(1)
    expect(html).toContain('width:50%')
    expect(html).toContain('border-radius:3px')
  })

  it('rounds segmented ticks to whole capsules without overstating progress', () => {
    const html = markup(0.5, 'segmented', 214)
    const spans = (html.match(/<span/g) ?? []).length
    expect(spans).toBe(48)
    expect((html.match(/background:#007aff/g) ?? []).length).toBe(24)
    expect((html.match(/background:var\(--panel-track\)/g) ?? []).length).toBe(24)
  })

  it('keeps the first tick lit but never fills the last tick early', () => {
    const tiny = markup(0.01, 'segmented', 214)
    expect((tiny.match(/background:#007aff/g) ?? []).length).toBe(1)
    const almost = markup(0.999, 'segmented', 214)
    expect((almost.match(/background:#007aff/g) ?? []).length).toBe(47)
  })

  it('falls back to a gradient tick pattern when the width is unknown', () => {
    const html = markup(0.5, 'segmented')
    expect(html).toContain('usage-bar--segmented')
    expect(html).toContain('width:50%')
  })
})
