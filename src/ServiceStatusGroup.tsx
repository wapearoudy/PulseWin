import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import './service-status.css'

type Health = 'operational' | 'degraded' | 'partialOutage' | 'fullOutage' | 'maintenance' | 'unrecognised'

/** Which status page a provider's rows are drawn from, as the Rust side names
 *  the palette its own page uses. */
type Palette = 'openCode' | 'openAi' | 'claude' | 'cursor' | 'statuspage' | 'incidentIo' | 'default'

interface StatusDay { date: string; state: Health | null; colour: string | null; hours: number | null }

/** Mirrors `Reading` in service_status.rs. The page is the provider it belongs
 *  to; a company's page (status.openai.com) serves several providers. */
export interface Reading {
  page: string
  address: string
  company: string
  palette: Palette
  checkedAt: string
  historyError: string | null
  components: { id: string; name: string; state: Health; uptime: number | null; group: string | null; days: StatusDay[] }[]
}

export const labels: Record<Health, string> = { operational: '运行正常', degraded: '性能下降', partialOutage: '部分故障', fullOutage: '全面故障', maintenance: '计划维护', unrecognised: '未知状态' }

/** The providers that have a status page at all. One request for the pane,
 *  shared by every provider it opens on; the Rust catalogue is the only place
 *  that list is written down. */
let pages: Promise<string[]> | null = null
export function statusPages(): Promise<string[]> {
  return (pages ??= invoke<string[]>('status_pages'))
}

/** The page a person opens, without its scheme — what the row shows. */
export function hostOf(address: string): string {
  return address.split('://').slice(-1)[0].split('/')[0]
}

/** How long ago the bars start. A page drawing sub-day bars says so in days
 *  too; one of exactly ninety says "90 days ago", not "90". */
export function periodLabel(days: StatusDay[]): string {
  const hours = days[0]?.hours
  if (hours) return `${Math.trunc((days.length * hours) / 24)} 天前`
  return `${days.length} 天前`
}

/** The right-hand end of the period line: a sub-day page's last bar is the one
 *  under way, so it reads "now" rather than "today". */
export function periodEnd(days: StatusDay[]): string {
  return days[0]?.hours ? '现在' : '今天'
}

/** The publisher's own figure — never our arithmetic. */
export function uptimeText(uptime: number | null): string | null {
  if (uptime === null || !Number.isFinite(uptime)) return null
  return `${uptime.toLocaleString(undefined, { maximumFractionDigits: 2 })}% 正常运行 · 官方报告`
}

/** A bar's tooltip: the day it covers and what the page said about it. */
export function dayTitle(day: StatusDay): string {
  const state = day.state ? (labels[day.state] ?? labels.unrecognised) : '无数据'
  if (!day.hours) return `${day.date} · ${state}`
  const [date, from] = day.date.split(' ')
  const start = Number(from?.slice(0, 2))
  const to = Number.isFinite(start) ? `${String((start + day.hours) % 24).padStart(2, '0')}:00` : ''
  return `${date} ${from}–${to} · ${state}`
}

const drawn = (day: StatusDay) => day.state !== null && /^#[0-9a-f]{6}$/i.test(day.colour ?? '')

export function ServiceStatusGroup({ provider, base }: { provider: string; base: string }) {
  const [reading, setReading] = useState<Reading | null>(null), [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true), [known, setKnown] = useState<boolean | null>(null)
  const [attempt, setAttempt] = useState(0)

  useEffect(() => { let live = true
    void statusPages().then(ids => { if (live) setKnown(ids.includes(base)) }).catch(() => { if (live) setKnown(false) })
    return () => { live = false }
  }, [base])

  useEffect(() => {
    if (known !== true) return
    let live = true
    const check = async () => {
      try {
        const result = await invoke<Reading>('get_service_status', { provider })
        if (live) { setReading(result ?? null); setError(result ? null : '无法读取官方状态页') }
      } catch (e) { if (live) { setReading(null); setError(String(e)) } }
      finally { if (live) setLoading(false) }
    }
    setReading(null); setError(null); setLoading(true); void check()
    const timer = window.setInterval(() => { void check() }, 300_000)
    return () => { live = false; window.clearInterval(timer) }
  }, [provider, known, attempt])

  if (known !== true) return null
  const host = reading?.address ? hostOf(reading.address) : null
  const days = reading?.components.find(c => c.days.length > 0)?.days ?? []
  const palette = reading?.palette ?? 'default'

  return <section className="group-wrap service-status" data-palette={palette} aria-label="官方服务状态"><h2>服务状态</h2><div className="settings-group">
    {loading && !reading ? <p className="group-copy" role="status">正在读取官方状态…</p> : error ? <p className="group-copy" role="status">{error}</p> : reading?.components.map(c => <div className="status-component" key={c.id}>
      <div className="status-heading"><strong>{c.name}</strong><span data-state={c.state}>{labels[c.state] ?? labels.unrecognised}</span></div>
      {c.days.length > 0 && <><div className="status-days" role="list" aria-label={`${c.name} 历史状态`}>{c.days.map((d, i) => <span role="listitem" key={`${d.date}:${i}`} data-state={d.state ?? 'missing'} style={drawn(d) ? { backgroundColor: d.colour! } : undefined} title={dayTitle(d)} />)}</div>
        <div className="status-period"><span>{periodLabel(c.days)}</span>{uptimeText(c.uptime) !== null && <span>{uptimeText(c.uptime)}</span>}<span>{periodEnd(c.days)}</span></div></>}
    </div>)}
    {reading?.historyError && <p className="group-copy" role="status">{reading.historyError}；上方仍为本次读取的当前状态。</p>}
    {host && <div className="setting-row"><div><strong>官方状态页</strong><small>{host}</small></div><div className="toolbar"><button disabled={loading} onClick={() => { setAttempt(n => n + 1) }}>{loading ? '读取中…' : '刷新'}</button><a href={`${reading!.address}/`} target="_blank" rel="noreferrer" onClick={e => { e.preventDefault(); void invoke('plugin:opener|open_url', { url: `${reading!.address}/` }).catch(err => setError(String(err))) }}>打开</a></div></div>}
  </div><p className="group-copy">由{reading?.company ? ` ${reading.company} ` : '服务商'}官方报告；进入此页面及停留期间每五分钟读取。{reading && `最近检查：${new Date(reading.checkedAt).toLocaleString()}`}</p>
    {days.length === 0 && reading && <p className="group-copy" role="status">此页面不发布历史柱，只报告当前状态。</p>}</section>
}
