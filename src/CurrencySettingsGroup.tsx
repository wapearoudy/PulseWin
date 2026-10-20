/**
 * Settings → General → Currency: the currency estimates are shown in, and, for
 * any but the dollar, the rate they are converted at.
 *
 * **Estimates only**, and the subtitle says so: a balance or a charge a service
 * reports is shown in the currency it gave (`money.reported`).
 *
 * **The source is credited here**, as its terms ask of a page that uses its
 * rates: "Rates By Exchange Rate API", linked. Written as the terms give it —
 * it is a credit, not copy.
 */
import { useEffect, useState, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import * as money from './money'
import type { Preferences } from './preferences'

const titles: Record<money.Course, string> = {
  usd: '美元', cny: '人民币', hkd: '港币', twd: '新台币', jpy: '日元', krw: '韩元', rub: '卢布',
}

/**
 * What was typed, read whole: digits with one decimal point or comma
 * ("7.12", "7,12"), or thousands grouped with commas before a point
 * ("1,339.81"). **Nothing is stripped to make it fit** — "-7.12" read as 7.12
 * once, a rate every estimate was then converted at — so a sign, a letter or a
 * second separator refuses it, also zero and a rate past `maximumRate`.
 */
export type RateEntry = { kind: 'blank' } | { kind: 'rate'; rate: number } | { kind: 'refused' }

/** Well past any unit of these currencies to a dollar (the won is about
 *  1,400), and low enough that a stray digit is refused. */
export const maximumRate = 100_000

export function rateEntry(typed: string): RateEntry {
  const text = typed.trim()
  if (!text) return { kind: 'blank' }
  const plain = /^([0-9]+)(?:[.,]([0-9]+))?$/.exec(text)
  const grouped = /^([0-9]{1,3}(?:,[0-9]{3})+)(?:\.([0-9]+))?$/.exec(text)
  const number = plain
    ? plain[1] + (plain[2] ? '.' + plain[2] : '')
    : grouped
      ? grouped[1].replace(/,/g, '') + (grouped[2] ? '.' + grouped[2] : '')
      : null
  if (number === null) return { kind: 'refused' }
  const rate = Number(number)
  return Number.isFinite(rate) && rate > 0 && rate <= maximumRate ? { kind: 'rate', rate } : { kind: 'refused' }
}

/** A rate as the field shows it: up to four places, no grouping. */
export function rateText(rate: number | undefined): string {
  if (rate === undefined) return ''
  return rate.toLocaleString('zh-CN', { maximumFractionDigits: 4, useGrouping: false })
}

export function CurrencySettingsGroup({ preferences, disabled, onChange }: {
  preferences: Preferences; disabled: boolean; onChange: (patch: Partial<Preferences>) => void
}) {
  const currency = preferences.displayCurrency
  const rates = money.current
  const code = money.codeOf(currency)
  const manual = preferences.manualExchangeRates[code]
  const fetched = rates.rates[code]
  const [typed, setTyped] = useState(rateText(manual))
  // The field follows a change of currency, and a rate saved from elsewhere.
  useEffect(() => { setTyped(rateText(manual)) }, [currency, manual])

  const commit = () => {
    const entry = rateEntry(typed)
    const next = { ...preferences.manualExchangeRates }
    if (entry.kind === 'blank') delete next[code]
    else if (entry.kind === 'rate') next[code] = entry.rate
    else { setTyped(rateText(manual)); return }
    onChange({ manualExchangeRates: next })
  }

  const subtitle = manual !== undefined
    ? '使用你输入的汇率。清空输入框即改用当天汇率。'
    : fetched !== undefined && rates.updatedAt !== null
      ? `当天汇率，更新于 ${new Date(rates.updatedAt * 1000).toLocaleDateString('zh-CN', { year: 'numeric', month: 'short', day: 'numeric' })}。输入汇率即可使用自己的。`
      : '还没有汇率，估算仍以美元显示。输入汇率，或等待当天汇率。'

  return (
    <section className="group-wrap"><h2>货币</h2><div className="settings-group">
      <Row title="估算的显示货币" subtitle="由 API 价格算出的金额：Token 消耗、估算价值、用量历史与回顾。服务商自己报告的余额与扣费，仍按它给出的货币显示。">
        <select aria-label="估算的显示货币" disabled={disabled} value={currency} onChange={event => onChange({ displayCurrency: event.target.value as money.Course })}>
          {money.courses.map(one => <option key={one} value={one}>{titles[one]}（{money.symbol(money.codeOf(one))}）</option>)}
        </select>
      </Row>
      {currency !== 'usd' && <>
        <Row title="汇率" subtitle={subtitle}>
          <div className="toolbar" style={{ gap: 6 }}>
            <span style={{ color: 'grey' }}>1 USD =</span>
            <input aria-label="汇率" inputMode="decimal" disabled={disabled} style={{ width: 90, textAlign: 'right' }} value={typed} placeholder={fetched === undefined ? '' : rateText(fetched)} onChange={event => setTyped(event.target.value)} onBlur={commit} onKeyDown={event => { if (event.key === 'Enter') commit() }} />
            <span style={{ color: 'grey' }}>{code}</span>
          </div>
        </Row>
        <Row title="汇率来源">
          <button type="button" style={{ border: 0, background: 'transparent', padding: 0, color: '#007aff', textDecoration: 'underline', cursor: 'pointer' }} onClick={() => { void invoke('plugin:opener|open_url', { url: rates.attribution }) }}>Rates By Exchange Rate API</button>
        </Row>
      </>}
    </div></section>
  )
}

function Row({ title, subtitle, children }: { title: string; subtitle?: string; children: ReactNode }) {
  return <div className="setting-row"><div><strong>{title}</strong>{subtitle && <small>{subtitle}</small>}</div><div className="setting-control">{children}</div></div>
}

export default CurrencySettingsGroup
