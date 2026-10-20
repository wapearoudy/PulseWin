import { describe, expect, it } from 'vitest'
import { apply, codeOf, converted, courses, exactEstimate, format, hasCents, rateFor, reported, symbol, type Rates } from './money'

const dollars = (manual: Record<string, number> = {}, rates: Record<string, number> = {}, currency = 'usd'): Rates => ({
  currency, rates, manual, updatedAt: null, source: 'https://open.er-api.com/v6/latest/USD', attribution: 'https://www.exchangerate-api.com',
})

describe('the currency estimates are shown in', () => {
  it('offers the seven currencies the original offers, one per language', () => {
    expect(courses).toEqual(['usd', 'cny', 'hkd', 'twd', 'jpy', 'krw', 'rub'])
    expect(courses.map(codeOf)).toEqual(['USD', 'CNY', 'HKD', 'TWD', 'JPY', 'KRW', 'RUB'])
  })

  it('has cents except in yen and won', () => {
    expect(hasCents('USD')).toBe(true)
    expect(hasCents('JPY')).toBe(false)
    expect(hasCents('KRW')).toBe(false)
  })

  it('gives the dollar a rate of exactly one without asking for a table', () => {
    expect(rateFor(dollars(), 'USD')).toBe(1)
  })

  it('uses the rate the reader typed before the fetched one', () => {
    expect(rateFor(dollars({ CNY: 7.5 }, { CNY: 7.1 }), 'CNY')).toBe(7.5)
    expect(rateFor(dollars({}, { CNY: 7.1 }), 'CNY')).toBe(7.1)
  })

  it('is no rate at all for zero, a negative or a NaN', () => {
    // Converting at one of these would print a figure that is not money, so
    // each falls through to the next source and then to the dollar.
    expect(rateFor(dollars({ CNY: 0 }), 'CNY')).toBeNull()
    expect(rateFor(dollars({ CNY: -7 }), 'CNY')).toBeNull()
    expect(rateFor(dollars({ CNY: Number.NaN }), 'CNY')).toBeNull()
    expect(rateFor(dollars({ CNY: 0 }, { CNY: 7.1 }), 'CNY')).toBe(7.1)
  })

  it('falls back to dollars when there is no rate, rather than to a made-up figure', () => {
    expect(converted(dollars({}, {}, 'cny'), 10)).toEqual({ amount: 10, code: 'USD' })
    expect(converted(dollars({}, { CNY: 7.1 }, 'cny'), 10)).toEqual({ amount: 71, code: 'CNY' })
  })
})

describe('writing an amount', () => {
  it('writes a narrow symbol rather than US$', () => {
    expect(format(3.4, 'USD')).toBe('$3.40')
    expect(format(3.4, 'CNY')).toBe('¥3.40')
  })

  it('keeps HK$ and NT$', () => {
    expect(symbol('HKD')).toBe('HK$')
    expect(symbol('TWD')).toBe('NT$')
  })

  it('drops cents from a thousand up, and always for yen and won', () => {
    expect(format(999.5, 'USD')).toBe('$999.50')
    expect(format(1342.4, 'USD')).toBe('$1,342')
    expect(format(151.3, 'JPY')).toBe('¥151')
    expect(format(1339.8, 'KRW')).toBe('₩1,340')
  })

  it('never prints a positive amount too small to show as free', () => {
    // Rounded, this is "$0.00" — which reads as nothing spent.
    expect(format(0.000001, 'USD')).toBe('< $0.01')
    expect(format(0.4, 'JPY')).toBe('< ¥1')
  })

  it('prints a real zero as zero', () => {
    // Unlike the line above: this one is a measurement, not a rounding.
    expect(format(0, 'USD')).toBe('$0.00')
  })

  it('honours a wholeFrom the caller picks', () => {
    // The recap's figures are small, so it carries cents up to ten.
    expect(format(47.2, 'USD')).toBe('$47.20')
    expect(format(47.2, 'USD', 10)).toBe('$47')
  })

  it('writes an unknown amount as a dash, never as zero', () => {
    expect(format(Number.NaN, 'USD')).toBe('—')
  })
})

describe('an estimate against an amount a service reported', () => {
  it('converts an estimate', () => {
    apply(dollars({}, { CNY: 7.1 }, 'cny'))
    expect(format(converted(dollars({}, { CNY: 7.1 }, 'cny'), 12.5).amount, 'CNY')).toBe('¥88.75')
  })

  it('never converts a reported amount: it stays in the currency it was given', () => {
    apply(dollars({}, { CNY: 7.1 }, 'cny'))
    expect(reported(20, 'USD')).toBe('$20.00')
    expect(reported(20, 'CNY')).toBe('¥20.00')
  })

  it('keeps every significant digit for a help tag', () => {
    expect(exactEstimate(0.000001234, dollars())).toBe('$0.000001234')
    expect(exactEstimate(0, dollars())).toBe('$0.00')
  })
})

describe('the table in effect', () => {
  it('is dollars with no rates before the first read', () => {
    // A converting UI that assumes a rate is worse than one that says dollars.
    expect(converted(dollars(), 5)).toEqual({ amount: 5, code: 'USD' })
  })
})
