import { describe, expect, it } from 'vitest'
import { bubbleCanvas, bubblePath, bubbleTransform, squircleRect } from './bubbleShape'

const base = {
  width: 300,
  height: 200,
  edge: 'right' as const,
  pointerCenter: 100,
  cornerRadius: 20,
  pointerWidth: 12,
  pointerHeight: 24,
}

/** Counts subpaths: one `M` starts one. */
const subpaths = (d: string) => (d.match(/M /g) ?? []).length

/**
 * The path's anchor points — the LAST coordinate pair of each command, which is
 * where the pen actually lands. Control points are not anchors and must not be
 * measured as if they were.
 */
function anchors(d: string): Array<{ x: number; y: number }> {
  const out: Array<{ x: number; y: number }> = []
  for (const m of d.matchAll(/([MLC])([^MLCZ]*)/g)) {
    const nums = m[2]
      .trim()
      .split(/[\s,]+/)
      .map(Number)
      .filter((v) => !Number.isNaN(v))
    if (nums.length >= 2) {
      out.push({ x: nums[nums.length - 2], y: nums[nums.length - 1] })
    }
  }
  return out
}

describe('squircleRect', () => {
  it('samples every one of the four corners', () => {
    const d = squircleRect(0, 0, 100, 60, 10)
    // Four straight edges plus four corners x 48 samples.
    expect((d.match(/L /g) ?? []).length).toBe(4 + 4 * 48)
  })

  it('degenerates to a plain rectangle at zero radius', () => {
    const d = squircleRect(0, 0, 100, 60, 0)
    expect(d).toBe('M 0 0 L 100 0 L 100 60 L 0 60 Z')
  })

  it('clamps a radius larger than half the shorter side', () => {
    // No self-intersection: the path stays inside its box.
    const d = squircleRect(0, 0, 100, 60, 500)
    for (const m of d.matchAll(/(?:M|L) ([-\d.]+) ([-\d.]+)/g)) {
      expect(Number(m[1])).toBeGreaterThanOrEqual(-0.001)
      expect(Number(m[1])).toBeLessThanOrEqual(100.001)
      expect(Number(m[2])).toBeGreaterThanOrEqual(-0.001)
      expect(Number(m[2])).toBeLessThanOrEqual(60.001)
    }
  })
})

describe('bubblePath', () => {
  it('draws the body and the tail as two subpaths, not two shapes', () => {
    // "Drawing both as one shape makes that impossible, since there is nothing
    // left to fall out of sync."
    expect(subpaths(bubblePath({ ...base }))).toBe(2)
  })

  it('can emit either half on its own for the glass surface', () => {
    expect(subpaths(bubblePath({ ...base, part: 'body' }))).toBe(1)
    expect(subpaths(bubblePath({ ...base, part: 'tail' }))).toBe(1)
    expect(bubblePath({ ...base, part: 'body' })).not.toContain('C ')
    expect(bubblePath({ ...base, part: 'tail' })).toContain('C ')
  })

  it('keeps the body clear of the pointer strip', () => {
    // The pointer lives in a strip along the rail-facing side; the body fills
    // what's left.
    const d = bubblePath({ ...base, part: 'body' })
    for (const m of d.matchAll(/(?:M|L) ([-\d.]+)/g)) {
      expect(Number(m[1])).toBeLessThanOrEqual(base.width - base.pointerWidth + 0.001)
    }
  })

  it('puts the pointer strip on the left when the rail is on the left', () => {
    const left = bubblePath({ ...base, edge: 'left', part: 'body' })
    for (const m of left.matchAll(/(?:M|L) ([-\d.]+)/g)) {
      expect(Number(m[1])).toBeGreaterThanOrEqual(base.pointerWidth - 0.001)
    }
  })

  it('bites the tail back into the body so the join is filled', () => {
    // "Bite back into the body so the join is covered by the fill rather than
    // leaving a seam along the edge."
    const d = bubblePath({ ...base, part: 'tail' })
    const bodyEdge = base.width - base.pointerWidth
    const xs = anchors(d).map((p) => p.x)
    // reach is pointerWidth; the tail reaches 8% of it back past the body edge.
    expect(Math.min(...xs)).toBeCloseTo(bodyEdge - base.pointerWidth * 0.08, 2)
  })

  it('reaches the tip exactly', () => {
    const d = bubblePath({ ...base, part: 'tail' })
    expect(Math.max(...anchors(d).map((p) => p.x))).toBeCloseTo(base.width, 6)
  })

  it('clamps the pointer clear of the rounded corners', () => {
    // "Keep the tail clear of the rounded corners, and inside the card even
    // when the caller asks for something out of range."
    //
    // The clamp applies to the pointer's **centre**, so the tail always spans
    // `centre ± half` — it slides, it does not stretch. The invariant is
    // therefore that its extent stays inside `[cornerRadius, height −
    // cornerRadius]`, and that the two extremes land exactly on those bounds.
    const half = base.pointerHeight / 2
    const extent = (pointerCenter: number) => {
      const ys = anchors(bubblePath({ ...base, part: 'tail', pointerCenter })).map((p) => p.y)
      return { low: Math.min(...ys), high: Math.max(...ys) }
    }

    for (const pointerCenter of [-999, 0, 100, base.height / 2, 999]) {
      const { low, high } = extent(pointerCenter)
      expect(low, `pointerCenter ${pointerCenter}`).toBeGreaterThanOrEqual(
        base.cornerRadius - 1e-6,
      )
      expect(high, `pointerCenter ${pointerCenter}`).toBeLessThanOrEqual(
        base.height - base.cornerRadius + 1e-6,
      )
    }

    expect(extent(-999).low).toBeCloseTo(base.cornerRadius, 6)
    expect(extent(999).high).toBeCloseTo(base.height - base.cornerRadius, 6)
    // Unclamped in the middle: the tail is exactly pointerHeight tall.
    const middle = extent(100)
    expect(middle.low).toBeCloseTo(100 - half, 6)
    expect(middle.high).toBeCloseTo(100 + half, 6)
  })

  it('gives the tail the same winding in both edge cases', () => {
    // The two subpaths are filled as one under the non-zero rule; opposite
    // windings cancel where they overlap and punch a gap between them, "which
    // is exactly what the left edge did once".
    const right = bubblePath({ ...base, edge: 'right', part: 'tail' })
    const left = bubblePath({ ...base, edge: 'left', part: 'tail' })

    const signedArea = (d: string) => {
      const pts = [...d.matchAll(/(?:M|L|C) ([-\d.]+) ([-\d.]+)(?:, [-\d.]+ [-\d.]+, [-\d.]+ [-\d.]+)?/g)].map(
        (m) => ({ x: Number(m[1]), y: Number(m[2]) }),
      )
      let sum = 0
      for (let i = 0; i < pts.length; i++) {
        const a = pts[i]
        const b = pts[(i + 1) % pts.length]
        sum += a.x * b.y - b.x * a.y
      }
      return Math.sign(sum)
    }

    expect(signedArea(right)).toBe(signedArea(left))
  })

  it('picks a different tail profile for round ends', () => {
    const soft = bubblePath({ ...base, part: 'tail', usesRoundEnds: false })
    const round = bubblePath({ ...base, part: 'tail', usesRoundEnds: true })
    expect(round).not.toBe(soft)
    // With round ends the near control point sits ON the card's edge.
    const reach = base.pointerWidth
    const firstControlX = base.width - base.pointerWidth + reach * 0
    expect(round).toContain(`${firstControlX} `)
  })
})

describe('placement', () => {
  it('leaves a side-docked card alone', () => {
    expect(bubbleTransform('right', 200)).toBeUndefined()
    expect(bubbleTransform('left', 200)).toBeUndefined()
  })

  it('rotates rather than reflects for a card under a top rail', () => {
    // A reflection would reverse the winding and the body and tail would cancel
    // where they overlap.
    const t = bubbleTransform('top', 200)
    expect(t).toBe('matrix(0,-1,1,0,0,200)')
    expect(t).not.toContain('scale(-1')
  })

  it('lays the canonical rect on its side for a top rail', () => {
    expect(bubbleCanvas('right', 300, 200)).toEqual({ width: 300, height: 200 })
    expect(bubbleCanvas('top', 300, 200)).toEqual({ width: 200, height: 300 })
  })

  it('produces a valid path for a top-docked card', () => {
    const d = bubblePath({ ...base, edge: 'top' })
    expect(d.startsWith('M')).toBe(true)
    expect(subpaths(d)).toBe(2)
  })
})
