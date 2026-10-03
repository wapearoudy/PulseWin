import { describe, expect, it } from 'vitest'
import {
  CAUTION_THRESHOLD,
  DEFAULT_WARNING_THRESHOLD,
  isSpent,
  usageTint,
  USAGE_TINT,
  WARNING_THRESHOLDS,
} from './tint'

describe('usageTint', () => {
  it('is green below the caution threshold', () => {
    expect(usageTint({ usedFraction: 0 })).toBe('good')
    expect(usageTint({ usedFraction: 0.49 })).toBe('good')
  })

  it('turns amber exactly at the caution threshold', () => {
    // `case ..<cautionThreshold` — so 0.5 itself is already caution.
    expect(usageTint({ usedFraction: CAUTION_THRESHOLD })).toBe('caution')
    expect(usageTint({ usedFraction: 0.74 })).toBe('caution')
  })

  it('turns red exactly at the warning threshold', () => {
    // `default: return .pulseWarning` — 0.75 itself is red, not 0.76.
    expect(usageTint({ usedFraction: DEFAULT_WARNING_THRESHOLD })).toBe('warning')
    expect(usageTint({ usedFraction: 0.99 })).toBe('warning')
  })

  it('gives spent its own colour, not a redder red', () => {
    expect(usageTint({ usedFraction: 1 })).toBe('exhausted')
    expect(usageTint({ usedFraction: 1.5 })).toBe('exhausted')
    expect(USAGE_TINT.exhausted).not.toBe(USAGE_TINT.warning)
  })

  it('trusts the provider flag over the fraction', () => {
    // "a lock can happen well short of 100%"
    expect(usageTint({ usedFraction: 0.2, isExhausted: true })).toBe('exhausted')
  })

  it('never inverts a missing reading into a full green ring', () => {
    // The original: "No reading → empty track either way. Do not invert nil to
    // a full '100% left' circle." The tint is still the comfortable one; the
    // caller draws an empty track rather than a filled arc.
    expect(usageTint({ usedFraction: null })).toBe('good')
    expect(usageTint({ usedFraction: undefined })).toBe('good')
  })

  it('honours every offered threshold, and they all clear caution', () => {
    for (const t of WARNING_THRESHOLDS) {
      expect(t, 'every option sits above caution').toBeGreaterThan(CAUTION_THRESHOLD)
      expect(usageTint({ usedFraction: t, warningAt: t })).toBe('warning')
      expect(usageTint({ usedFraction: t - 0.01, warningAt: t })).toBe('caution')
    }
  })

  it('ships 75% as the default', () => {
    expect(DEFAULT_WARNING_THRESHOLD).toBe(0.75)
    expect(WARNING_THRESHOLDS).toContain(0.75)
  })
})

describe('isSpent', () => {
  it('follows the provider flag first', () => {
    expect(isSpent(0.1, true)).toBe(true)
    expect(isSpent(0.1, false)).toBe(false)
  })

  it('treats a full fraction as spent', () => {
    expect(isSpent(1)).toBe(true)
    expect(isSpent(0.999)).toBe(false)
  })

  it('does not call a missing reading spent', () => {
    expect(isSpent(null)).toBe(false)
    expect(isSpent(undefined)).toBe(false)
  })
})
