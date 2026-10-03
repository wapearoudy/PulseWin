import { describe, expect, it } from 'vitest'
import { creditRailText, creditText, usableCredit } from './creditAmount'

describe('reported money on the assistant', () => {
  it('keeps missing, nonfinite and noncurrency values unavailable', () => {
    expect(creditRailText(undefined)).toBeNull()
    expect(usableCredit({amount:NaN,currency:'USD'})).toBe(false)
    expect(usableCredit({amount:20,currency:'积分'})).toBe(false)
  })
  it('does not mistake a reported zero for missing money', () => {
    expect(creditRailText({amount:0,currency:'USD'},'en-US')).toBe('$0')
    expect(creditText({amount:0,currency:'USD'},'en-US')).toBe('USD 0.00')
  })
  it('truncates cents instead of promising more remaining credit', () => {
    expect(creditRailText({amount:19.999,currency:'USD'},'en-US')).toBe('$19.99')
  })
  it('drops fractions at three digits and avoids the thousand rollover', () => {
    expect(creditRailText({amount:999.99,currency:'CNY'},'en-US')).toBe('¥999')
    expect(creditRailText({amount:999999,currency:'CNY'},'en-US')).toBe('¥999k')
  })
  it('preserves the currency and compact million magnitude', () => {
    expect(creditRailText({amount:1_234_567,currency:'EUR'},'en-US')).toBe('€1.2M')
    expect(creditText({amount:5_000,currency:'CNY'},'en-US')).toBe('CNY 5,000.00')
  })
})
