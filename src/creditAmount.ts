import type { ProviderUsage } from './types'

type CreditAmount = NonNullable<ProviderUsage['creditRemaining']>

export function usableCredit(amount: CreditAmount | null | undefined): amount is CreditAmount {
  return !!amount && Number.isFinite(amount.amount) && /^[A-Za-z]{3}$/.test(amount.currency)
}

/** Original CreditAmount.railText: compact, truncated, never an invented quota. */
export function creditRailText(amount: CreditAmount | null | undefined, locale = 'zh-CN'): string | null {
  if (!usableCredit(amount)) return null
  const magnitude = Math.abs(amount.amount)
  const divisor = magnitude >= 1_000_000 ? 1_000_000 : magnitude >= 1_000 ? 1_000 : 1
  const suffix = divisor === 1_000_000 ? 'M' : divisor === 1_000 ? 'k' : ''
  const value = amount.amount / divisor
  const places = Math.abs(value) >= 100 ? 0 : suffix ? 1 : 2
  const scale = 10 ** places, shown = Math.trunc(value * scale) / scale
  const symbol = new Intl.NumberFormat(locale, {style:'currency', currency:amount.currency, currencyDisplay:'narrowSymbol', maximumFractionDigits:0})
    .formatToParts(0).find(part => part.type === 'currency')?.value ?? amount.currency
  return symbol + new Intl.NumberFormat(locale, {maximumFractionDigits:places, useGrouping:false}).format(shown) + suffix
}

export function creditText(amount: CreditAmount | null | undefined, locale = 'zh-CN'): string | null {
  if (!usableCredit(amount)) return null
  return `${amount.currency.toUpperCase()} ${new Intl.NumberFormat(locale, {minimumFractionDigits:2, maximumFractionDigits:2}).format(amount.amount)}`
}

export function balanceOnly(provider: ProviderUsage | undefined): boolean {
  return !!provider && provider.windows.length === 0 && usableCredit(provider.creditRemaining)
}
