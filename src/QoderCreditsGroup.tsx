import { useCallback, useEffect, useState } from 'react'
import { qoderCreditHistory, type QoderCreditDay } from './api'

/** The account's own records: what the work cost and what Qoder deducted. */
export function QoderCreditsGroup({ enabled }: { enabled: boolean }) {
  const [days, setDays] = useState<QoderCreditDay[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const read = useCallback(async () => {
    if (!enabled) return
    setBusy(true)
    setError(null)
    try {
      setDays(await qoderCreditHistory())
    } catch (failure) {
      setDays(null)
      setError(String(failure))
    } finally {
      setBusy(false)
    }
  }, [enabled])
  useEffect(() => {
    if (enabled) void read()
    else {
      setDays(null)
      setBusy(false)
    }
  }, [enabled, read])
  const total = (count: number) =>
    (days ?? []).slice(-count).reduce(
      (sum, day) => ({ used: sum.used + day.used, charged: sum.charged + day.charged }),
      { used: 0, charged: 0 },
    )
  const peak = Math.max(1, ...(days ?? []).map(day => day.used))
  const columns: Array<[string, number]> = [
    ['今日', 1],
    ['最近 7 天', 7],
    ['最近 31 天', 31],
  ]
  return (
    <section className="group-wrap" aria-label="Qoder 积分用量">
      <h2>积分用量</h2>
      <div className="settings-group">
        <div className="setting-row">
          <div>
            <strong>账户自己的用量记录</strong>
            <small>按天汇总最近 31 天：消耗（折扣前）与实际扣减积分。</small>
          </div>
          <button disabled={busy || !enabled} onClick={() => void read()}>刷新</button>
        </div>
        {!enabled && <p className="group-copy">启用 Qoder 后读取积分历史。</p>}
        {error && <p className="group-copy" role="alert">{error}</p>}
        {busy && !days && <p className="group-copy" role="status">正在读取积分历史…</p>}
        {days && (
          <>
            <div className="group-copy" style={{ display: 'flex', gap: 24 }}>
              {columns.map(([label, count]) => {
                const sum = total(count)
                return (
                  <div key={label}>
                    <strong>{label}</strong>
                    <div>{sum.used.toLocaleString(undefined, { maximumFractionDigits: 2 })} 消耗</div>
                    <small>扣减 {sum.charged.toLocaleString(undefined, { maximumFractionDigits: 2 })}</small>
                  </div>
                )
              })}
            </div>
            <div
              role="img"
              aria-label="每日积分用量"
              style={{ display: 'flex', alignItems: 'flex-end', gap: 2, height: 64, marginTop: 12 }}
            >
              {days.map(day => (
                <span
                  key={day.date}
                  title={`${day.date}：消耗 ${day.used}，扣减 ${day.charged}`}
                  style={{
                    flex: 1,
                    height: `${Math.max(2, (day.used / peak) * 100)}%`,
                    background: '#007aff',
                    borderRadius: 2,
                  }}
                />
              ))}
            </div>
            <div className="group-copy" style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span>{days[0]?.date}</span>
              <span>{days[days.length - 1]?.date}</span>
            </div>
          </>
        )}
      </div>
    </section>
  )
}
