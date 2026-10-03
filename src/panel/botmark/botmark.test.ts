import { describe, expect, it } from 'vitest'
import { EYE_HALF, EXPRESSIONS, SHAPE_ORDER, STATES, shape, shapeChoices, state } from './data'
import { centroid, HEAD_CENTRE, lerpRing, pathFromSegments, ringOutline, ringPath, spanAt } from './geometry'
import {
  resolveMood,
  rotationEmphasis,
  squashEmphasis,
  tempoEmphasis,
  UPSTREAM_STATE,
} from './mood'
import { brandColour, deal, eyesOn, hueOf, INK, lifted, luminance, MONOCHROME, PALETTE_SIZE, PAPER, separation, wheelColour } from './tint'

describe('bot-data port', () => {
  it('carries all eighteen bodies, in upstream order', () => {
    expect(SHAPE_ORDER).toHaveLength(18)
    expect(SHAPE_ORDER[0]).toBe('blob')
    expect(shapeChoices()).toHaveLength(18)
    for (const id of SHAPE_ORDER) expect(shape(id).ring.length).toBeGreaterThan(0)
  })

  it('samples every body to the same 96-point ring', () => {
    // "all sampled to the same 96-point ring so any one blends into any other"
    const counts = new Set(SHAPE_ORDER.map((id) => shape(id).ring.length))
    expect([...counts]).toEqual([96])
  })

  it('keeps the canvas constants the rings are expressed in', () => {
    expect(HEAD_CENTRE).toBeCloseTo(114.2705, 4)
    expect(EYE_HALF).toBe(21)
  })

  it('holds twenty-five expressions of two eye rings', () => {
    expect(EXPRESSIONS).toHaveLength(25)
    for (const expression of EXPRESSIONS) {
      expect(expression).toHaveLength(2)
      expect(expression[0]).toHaveLength(48)
      expect(expression[1]).toHaveLength(48)
    }
  })

  it('holds all thirty-nine upstream states, and every mood resolves to one', () => {
    expect(Object.keys(STATES)).toHaveLength(39)
    for (const upstream of Object.values(UPSTREAM_STATE)) {
      expect(STATES[upstream], `state ${upstream} must exist`).toBeDefined()
    }
  })

  it('falls back rather than drawing nothing', () => {
    // A stored choice that no longer exists must still draw a character.
    expect(shape('no-such-body').id).toBe('blob')
    expect(state('no-such-state').id).toBe('idle')
  })

  it('gives every expression a live pool entry', () => {
    for (const entry of Object.values(STATES)) {
      expect(entry.expressionPool.length).toBeGreaterThan(0)
      for (const index of entry.expressionPool) {
        expect(EXPRESSIONS[index], `expression ${index} for ${entry.id}`).toBeDefined()
      }
    }
  })
})

describe('geometry', () => {
  const square = [
    { x: 0, y: 0 },
    { x: 10, y: 0 },
    { x: 10, y: 10 },
    { x: 0, y: 10 },
  ]

  it('averages the ring for the centroid', () => {
    expect(centroid(square)).toEqual({ x: 5, y: 5 })
  })

  it('interpolates point by point', () => {
    const moved = lerpRing(square, square.map((p) => ({ x: p.x + 4, y: p.y })), 0.5)
    expect(moved[0]).toEqual({ x: 2, y: 0 })
  })

  it('refuses to interpolate rings of different lengths', () => {
    expect(lerpRing(square, square.slice(0, 2), 0.5)).toHaveLength(2)
  })

  it('closes the straight-sided path', () => {
    expect(ringPath(square).endsWith('Z')).toBe(true)
    expect(ringPath(square).startsWith('M 0 0')).toBe(true)
  })

  it('emits one cubic per point for the smoothed outline', () => {
    const d = ringOutline(square)
    expect((d.match(/C /g) ?? []).length).toBe(4)
  })

  it('falls back to the straight path when there is nothing to smooth', () => {
    expect(ringOutline(square.slice(0, 2))).toBe(ringPath(square.slice(0, 2)))
  })

  it('measures the silhouette at a height', () => {
    const [left, right] = spanAt(square, 5, 5)
    expect(left).toBe(0)
    expect(right).toBe(10)
  })

  it('returns the head centre when the scan finds no crossing', () => {
    expect(spanAt(square, 99, 5)).toEqual([5, 5])
  })

  it('replays upstream path segments', () => {
    const d = pathFromSegments([
      { op: 'M', values: [1, 2] },
      { op: 'C', values: [3, 4, 5, 6, 7, 8] },
      { op: 'Z', values: [] },
    ])
    expect(d).toBe('M 1 2 C 3 4 5 6 7 8 Z')
  })
})

describe('mood', () => {
  it('plays the upstream states the original names', () => {
    expect(UPSTREAM_STATE).toEqual({
      idle: 'idle',
      working: 'working',
      fetching: 'searching',
      spent: 'sad',
      unavailable: 'confused',
    })
  })

  it('lets busy outrank spent, and spent outrank an empty reading', () => {
    expect(resolveMood({ isBusy: true, isRefreshing: true, isSpent: true })).toBe('working')
    expect(resolveMood({ isRefreshing: true, isSpent: true })).toBe('fetching')
    expect(resolveMood({ isSpent: true, hasReading: true })).toBe('spent')
    expect(resolveMood({ hasReading: false })).toBe('unavailable')
    expect(resolveMood({ hasReading: true })).toBe('idle')
  })

  it('exaggerates only the moods that are an event', () => {
    expect(rotationEmphasis('working')).toBe(2.4)
    expect(rotationEmphasis('fetching')).toBe(1.6)
    for (const quiet of ['idle', 'spent', 'unavailable'] as const) {
      expect(rotationEmphasis(quiet)).toBe(1)
      expect(squashEmphasis(quiet)).toBe(1)
      expect(tempoEmphasis(quiet)).toBe(1)
    }
    expect(squashEmphasis('working')).toBe(3)
  })

  it('makes smaller mean busier', () => {
    expect(tempoEmphasis('working')).toBeLessThan(tempoEmphasis('fetching'))
    expect(tempoEmphasis('fetching')).toBeLessThan(tempoEmphasis('idle'))
  })
})

describe('tint', () => {
  it('sizes the wheel prime, so every stride is coprime to it', () => {
    expect(PALETTE_SIZE).toBe(17)
    expect(Number.isInteger(PALETTE_SIZE)).toBe(true)
  })

  it('levels every hue to the same luminance', () => {
    // "At one fixed lightness a yellow comes out glaring and a blue nearly
    // black, so each hue's lightness is solved for the same perceived
    // luminance instead." The wheel's target is 0.62; the bisection lands on it.
    for (let slot = 0; slot < PALETTE_SIZE; slot++) {
      const css = wheelColour(slot)
      const match = /^rgb\((\d+) (\d+) (\d+)\)$/.exec(css)
      expect(match, `slot ${slot} is a css colour`).not.toBeNull()
      const value = luminance({
        r: Number(match![1]) / 255,
        g: Number(match![2]) / 255,
        b: Number(match![3]) / 255,
      })
      expect(Math.abs(value - 0.62)).toBeLessThan(0.02)
    }
  })

  it('never repeats a hue on the wheel', () => {
    const hues = new Set(Array.from({ length: PALETTE_SIZE }, (_, slot) => wheelColour(slot)))
    expect(hues.size).toBe(PALETTE_SIZE)
    expect(wheelColour(0)).toBe(wheelColour(PALETTE_SIZE))
  })

  it('lifts a colour that would vanish against the disc, and no further', () => {
    // The floor is relative luminance 0.42.
    const dark = { r: 0.05, g: 0.05, b: 0.05 }
    expect(luminance(lifted(dark))).toBeGreaterThanOrEqual(0.41)
    const already = { r: 0.9, g: 0.9, b: 0.9 }
    expect(lifted(already)).toEqual(already)
  })

  it('puts dark eyes on a light body and light eyes on a dark one', () => {
    expect(eyesOn({ r: 0.9, g: 0.9, b: 0.9 })).toBe(INK)
    expect(eyesOn({ r: 0.1, g: 0.1, b: 0.1 })).toBe(PAPER)
  })

  it('keeps neighbouring dealt rings apart, and refuses to score two brands', () => {
    // Every ring colourless: the deal must spread them.
    const dealt = deal([null, null, null, null, null, null])
    expect(new Set(dealt).size).toBe(6)

    // A brand keeps its own colour; only the colourless ones are dealt.
    const mixed = deal(['rgb(217 119 87)', null, null])
    expect(mixed[0]).toBe('rgb(217 119 87)')

    // Two brand colours side by side are what those two brands are, so they do
    // not drag the score down — and each keeps its own colour, lifted only if
    // it would otherwise vanish against the disc.
    const allBrand = deal(['rgb(217 119 87)', 'rgb(232 72 63)'])
    expect(allBrand[0]).toBe('rgb(217 119 87)')
    // MiniMax's red sits just under the floor, so it comes back lifted.
    const before = luminance({ r: 232 / 255, g: 72 / 255, b: 63 / 255 })
    expect(before).toBeLessThan(0.42)
    const channels = /^rgb\((\d+) (\d+) (\d+)\)$/.exec(allBrand[1])!
    const after = luminance({
      r: Number(channels[1]) / 255,
      g: Number(channels[2]) / 255,
      b: Number(channels[3]) / 255,
    })
    expect(after).toBeGreaterThan(before)
    expect(after).toBeGreaterThanOrEqual(0.41)
  })

  it('names the monochrome providers, rather than letting them fall through', () => {
    // "has no colour" and "has a colour we have not written down" are different
    // claims, and only the first is safe to deal from.
    for (const id of MONOCHROME) expect(brandColour(id)).toBeNull()
    expect(brandColour('claude-code')).toBe('rgb(217 119 87)')
    expect(brandColour('hugging-face')).toBe('rgb(255 210 30)')
    // An unported provider is not claimed to be monochrome.
    expect(MONOCHROME.has('mistral')).toBe(false)
  })

  it('measures hue separation the short way round', () => {
    expect(separation(10, 350)).toBe(20)
    expect(separation(0, 180)).toBe(180)
    expect(hueOf({ r: 1, g: 0, b: 0 })).toBe(0)
    expect(hueOf({ r: 0, g: 1, b: 0 })).toBeCloseTo(120, 6)
  })
})
