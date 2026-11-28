import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { qoderCreditHistory, type QoderCreditDay } from '../api'
import { dayKey } from '../token-spend/summary'
import type { DetailCardLayout } from './cardLayout'

function brieflyCached<T>(read: () => Promise<T>) {
  let cached: { until: number; value: T } | undefined
  let pending: Promise<T> | undefined
  return () => {
    if (cached && cached.until > performance.now()) return Promise.resolve(cached.value)
    if (pending) return pending
    pending = read().then(value => {
      cached = { until: performance.now() + 30_000, value }
      return value
    }).finally(() => { pending = undefined })
    return pending
  }
}

type CacheReading = { live: { expiresAt: string }[] }
const readClaudeCache = brieflyCached(() => invoke<CacheReading>('get_prompt_cache', { provider: 'claude-code' }))
const readQoderHistory = brieflyCached(qoderCreditHistory)

function useReading<T>(read: () => Promise<T>) {
  const [value, setValue] = useState<T | null>(null)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    let live = true
    read().then(value => { if (live) setValue(value) }).catch(() => { if (live) setFailed(true) })
    return () => { live = false }
  }, [read])
  return { value, failed }
}

export function ClaudeCacheSummary({ layout, now }: { layout: DetailCardLayout; now: number }) {
  const { value } = useReading(readClaudeCache)
  const expires = value?.live.map(session => Date.parse(session.expiresAt)).filter(at => at > now).sort((a,b) => a-b)[0]
  if (expires == null) return null
  const seconds = Math.ceil((expires - now) / 1000)
  const remaining = seconds >= 60 ? `${Math.floor(seconds/60)} 分 ${seconds%60} 秒` : `${seconds} 秒`
  return <div className="card__row-line" style={{ height: layout.rowTextLineHeight, fontSize: layout.rowFontSize }}>
    <span className="card__muted">提示词缓存</span><span className="card__model">最近到期 · {remaining}</span>
  </div>
}

const number = new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 2 })
export function QoderCardHistory({ layout, now }: { layout: DetailCardLayout; now: number }) {
  const { value, failed } = useReading(readQoderHistory)
  const today = new Date(now)
  const start = (days: number) => dayKey(new Date(today.getFullYear(), today.getMonth(), today.getDate() - days + 1))
  const days = (value ?? []).filter(day => day.date >= start(31) && day.date <= dayKey(today))
  function totals(count: number): Pick<QoderCreditDay, 'used' | 'charged'> {
    return days.filter(day => day.date >= start(count)).reduce((sum, day) => ({ used: sum.used + day.used, charged: sum.charged + day.charged }), { used: 0, charged: 0 })
  }
  const s = layout.width / 250
  const maximum = Math.max(1, ...days.map(day => day.used))
  return <section className="card__activity" aria-label="Qoder 积分历史" style={{ gap: layout.activitySpacing, height: layout.activityHeight, fontSize: layout.footnoteFontSize }}>
    <div className="card__divider" style={{ height: 1 }} />
    <div className="card__muted" style={{ height: 13*s }}>Qoder 积分</div>
    {failed ? <div className="card__muted">无法读取积分历史。</div> : value === null ? <div className="card__muted">正在读取积分历史…</div> : !days.length ? <div className="card__muted">暂无积分历史。</div> : <>
      {layout.parts.has('figures') && <div className="card__figures" style={{ height: 47*s, gap: 8*s }}>
        {([['今天', 1], ['7天', 7], ['31天', 31]] as const).map(([label, count]) => {
          const sum = totals(count)
          return <div key={label} aria-label={label} style={{ gap: 2*s }}>
            <span className="card__muted">{label}</span><strong style={{ fontSize: 14*s }}>{number.format(sum.used)}</strong>
            <span className="card__muted">扣减 {number.format(sum.charged)}</span>
          </div>
        })}
      </div>}
      {layout.parts.has('chart') && <div className="card__chart" role="img" aria-label="最近 31 天积分用量" style={{ height: 30*s, gap: 2*s }}>
        {days.map(day => <span key={day.date} title={`${day.date} · ${number.format(day.used)} 积分`} style={{ height: day.used ? `${Math.max(2, day.used / maximum * 100)}%` : '1px' }} />)}
      </div>}
    </>}
  </section>
}
