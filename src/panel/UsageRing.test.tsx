import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { UsageRing } from './UsageRing'

describe('usage rings', () => {
  it.each([0.82, 1, 1.22])('draws both strokes at the same weight with a one-point scaled gap at %s', scale => {
    const markup = renderToStaticMarkup(<UsageRing diameter={36 * scale} lineWidth={4 * scale} scale={scale}
      usedFraction={0.82} secondFraction={0.37} warningAt={0.75} />)
    const [outer, inner] = [...markup.matchAll(/<circle\b[^>]+stroke-dasharray="[^"]+"[^>]*>/g)]
      .map(([tag]) => ({
        radius: Number(tag.match(/\br="([^"]+)"/)?.[1]),
        width: Number(tag.match(/\bstroke-width="([^"]+)"/)?.[1]),
      }))
    expect(outer.width).toBeCloseTo(4 * scale, 10)
    expect(inner.width).toBeCloseTo(outer.width, 10)
    expect((outer.radius - outer.width / 2) - (inner.radius + inner.width / 2)).toBeCloseTo(scale, 10)
  })
})
