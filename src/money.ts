/**
 * How money is written, and what an estimate is worth in the chosen currency.
 * A port of the original's `Money.swift` / `DisplayCurrency.swift`.
 *
 * **A symbol, not "US$".** Chinese and Korean write a dollar "US$" in the
 * standard style, which on a card reads as a different currency from the figure
 * beside it; the narrow symbol is "$" in every language Pulse speaks. Hong Kong
 * and New Taiwan dollars keep their prefix ("HK$", "NT$").
 *
 * **Only estimates are converted** — the money worked out from US-dollar API
 * prices: the value estimate, Token spend, the cards' history, the recap. What
 * a service reports itself (a balance, credits, what a console charged) is shown
 * in the currency it was given in, or it would disagree with the service's own
 * page. Those call sites use [`reported`], never [`estimate`].
 *
 * Global rather than a hook because money is formatted by plain functions that
 * are not components ([`formatCost`] in `token-spend/summary.ts` is the main
 * one). `useMoney` is what keeps the globals in step with preferences.
 */
import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

/** One per language the original speaks; the dollar is what estimates are
 *  worked out in and the other six are conversions of it. */
export type Course = 'usd' | 'cny' | 'hkd' | 'twd' | 'jpy' | 'krw' | 'rub'
export const courses: Course[] = ['usd', 'cny', 'hkd', 'twd', 'jpy', 'krw', 'rub']

/** The ISO code, as `rates` and `manual` are keyed by. */
export const codeOf = (currency: Course): string => currency.toUpperCase()

export interface Rates {
  currency: string
  rates: Record<string, number>
  manual: Record<string, number>
  updatedAt: number | null
  source: string
  attribution: string
}

/** Yen and won are written in whole units: they have no cents. */
export function hasCents(code: string): boolean {
  return code !== 'JPY' && code !== 'KRW'
}

/** Units of `code` to one US dollar: the reader's own rate, else the fetched
 *  one; null when there is neither. Zero, a negative and a NaN are all "no
 *  rate" — converting with one would print a figure that is not money. */
export function rateFor(rates: Rates, code: string): number | null {
  if (code === 'USD') return 1
  const typed = rates.manual[code]
  if (typed !== undefined && Number.isFinite(typed) && typed > 0) return typed
  const fetched = rates.rates[code]
  return fetched !== undefined && Number.isFinite(fetched) && fetched > 0 ? fetched : null
}

/** An estimate in US dollars, in the chosen currency. **Dollars still when there
 *  is no rate** — before the first fetch, or offline with none kept — rather
 *  than a figure converted at a rate Pulse does not have. */
export function converted(rates: Rates, usd: number): { amount: number; code: string } {
  const code = rates.currency.toUpperCase()
  const rate = rateFor(rates, code)
  return rate === null ? { amount: usd, code: 'USD' } : { amount: usd * rate, code }
}

/** The code estimates are written in now. */
export function shownCode(rates: Rates): string {
  return converted(rates, 0).code
}

const locale = 'zh-CN'

/** The symbol for `code`, alone where it is unambiguous. Derived from Intl so
 *  the glyph is the one this platform actually writes, then filtered to the non
 *  digits and whitespace of a formatted zero — never a hardcoded table. */
export function symbol(code: string): string {
  const narrow = code !== 'HKD' && code !== 'TWD'
  const written = new Intl.NumberFormat(code === 'TWD' ? 'zh-Hans' : locale, {
    style: 'currency', currency: code, currencyDisplay: narrow ? 'narrowSymbol' : 'symbol', maximumFractionDigits: 0,
  }).formatToParts(0)
  return written.map(part => part.value).join('').replace(/[\d\s]/g, '') || code
}

function grouped(amount: number, places: number): string {
  return new Intl.NumberFormat(locale, { minimumFractionDigits: places, maximumFractionDigits: places }).format(amount)
}

/**
 * An amount already in `code` — a balance a service reported, or an estimate
 * [`converted`] — written the way the original writes it: two places below
 * `wholeFrom` (in the currency written), whole units from there, and always
 * whole for yen and won.
 *
 * **A positive amount too small to show is still not free.** Rounded it would
 * print "$0.00", which reads as nothing spent; it says "< $0.01" (or "< ¥1")
 * instead. A real zero prints as zero: that one is a measurement.
 */
export function format(amount: number, code: string, wholeFrom = 1000): string {
  const cents = hasCents(code)
  const unit = cents ? 0.01 : 1
  if (!Number.isFinite(amount)) return '—'
  if (amount > 0 && amount < unit) return `< ${symbol(code)}${grouped(unit, cents ? 2 : 0)}`
  const places = cents && Math.abs(amount) < wholeFrom ? 2 : 0
  return `${symbol(code)}${grouped(amount, places)}`
}

/** An estimate in US dollars, written in the chosen currency. */
export function estimate(usd: number, wholeFrom = 1000, rates = current): string {
  const { amount, code } = converted(rates, usd)
  return format(amount, code, wholeFrom)
}

/** The same estimate with every significant digit, for a help tag where the
 *  visible rounding hides the digits being checked. */
export function exactEstimate(usd: number, rates = current): string {
  const { amount, code } = converted(rates, usd)
  if (amount === 0) return format(amount, code, Number.POSITIVE_INFINITY)
  return `${symbol(code)}${amount.toPrecision(15).replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '')}`
}

/** An amount a service reported — a balance, credits, a charge — in the currency
 *  it gave: **never converted**, cents where the currency has them. */
export function reported(amount: number, code: string): string {
  if (!Number.isFinite(amount)) return '—'
  return `${symbol(code)}${grouped(amount, hasCents(code) ? 2 : 0)}`
}

/** What the app draws with. Dollars and no rates until the first read, which is
 *  the same thing the original does before its keeper has looked. */
export let current: Rates = { currency: 'usd', rates: {}, manual: {}, updatedAt: null, source: 'https://open.er-api.com/v6/latest/USD', attribution: 'https://www.exchangerate-api.com' }

/** Listeners redrawn at a new rate — the globals are read by plain functions
 *  that React cannot observe, so the panes that print money subscribe. */
const listeners = new Set<() => void>()

/** Put a table in effect. Called by `useMoney` and by the rates-changed event. */
export function apply(rates: Rates) {
  const same = rates.currency === current.currency
    && rates.updatedAt === current.updatedAt
    && JSON.stringify(rates.manual) === JSON.stringify(current.manual)
    && JSON.stringify(rates.rates) === JSON.stringify(current.rates)
  if (same) return
  current = rates
  for (const listener of listeners) listener()
}

export function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

/**
 * Keeps [`current`] in step with the backend, and redraws the window when a
 * rate arrives: [`current`] is read by plain functions React cannot observe,
 * so the one component that calls this in each window re-renders its tree at a
 * new table.
 *
 * Called once per window (App, Settings, TrayDashboard). Reads the table once,
 * then again whenever the backend stores a fresh one or the preferences change
 * — a new currency or a typed rate changes the answer with no fetch.
 */
export function useMoney(): Rates {
  const [rates, setRates] = useState(current)
  useEffect(() => {
    let live = true
    const off = subscribe(() => setRates(current))
    const read = async () => {
      try {
        const value = await invoke<Rates>('get_exchange_rates')
        if (live) { apply(value); setRates(current) }
      } catch { /* dollars stay: a failed read is no rate, not an error pane */ }
    }
    void read()
    const pending = [
      listen('exchange-rates-changed', () => { if (live) void read() }),
      listen('preferences-changed', () => { if (live) void read() }),
    ]
    return () => {
      live = false
      off()
      for (const one of pending) void one.then(fn => fn())
    }
  }, [])
  return rates
}
