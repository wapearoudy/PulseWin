import { describe, expect, it } from 'vitest'
import { maximumRate, rateEntry, rateText } from './CurrencySettingsGroup'

describe('a rate the reader types', () => {
  it('reads a plain number with a point or a comma', () => {
    expect(rateEntry('7.12')).toEqual({ kind: 'rate', rate: 7.12 })
    expect(rateEntry('7,12')).toEqual({ kind: 'rate', rate: 7.12 })
    expect(rateEntry('  7.12  ')).toEqual({ kind: 'rate', rate: 7.12 })
  })

  it('reads thousands grouped with commas', () => {
    expect(rateEntry('1,339.81')).toEqual({ kind: 'rate', rate: 1339.81 })
    // A single separator is read as the decimal one, exactly as the original
    // reads it: the plain pattern is tried first, so "12,345" is 12.345 — a
    // rate far too small to be one of these currencies, but not mistyped as
    // 12345 either.
    expect(rateEntry('12,345')).toEqual({ kind: 'rate', rate: 12.345 })
  })

  it('refuses a rate with a sign rather than quietly dropping it', () => {
    // "-7.12" read as 7.12 once — a rate every estimate was then converted
    // at. Refusing is the only answer that cannot be mistaken for the typed one.
    expect(rateEntry('-7.12')).toEqual({ kind: 'refused' })
  })

  it('refuses a letter or a second separator', () => {
    expect(rateEntry('7.1.2')).toEqual({ kind: 'refused' })
    expect(rateEntry('7,1.2')).toEqual({ kind: 'refused' })
    expect(rateEntry('seven')).toEqual({ kind: 'refused' })
  })

  it('refuses zero, which would make every estimate read as free', () => {
    expect(rateEntry('0')).toEqual({ kind: 'refused' })
    expect(rateEntry('0.00')).toEqual({ kind: 'refused' })
  })

  it('refuses a rate past the ceiling, so a stray digit is not a rate', () => {
    expect(rateEntry(String(maximumRate))).toEqual({ kind: 'rate', rate: maximumRate })
    expect(rateEntry(String(maximumRate + 1))).toEqual({ kind: 'refused' })
  })

  it('reads a blank field as "use the fetched rate"', () => {
    expect(rateEntry('')).toEqual({ kind: 'blank' })
    expect(rateEntry('   ')).toEqual({ kind: 'blank' })
  })

  it('shows a rate with up to four places and no grouping', () => {
    expect(rateText(undefined)).toBe('')
    expect(rateText(7.123456)).toBe('7.1235')
    expect(rateText(1339.81)).toBe('1339.81')
    expect(rateText(151)).toBe('151')
  })
})
