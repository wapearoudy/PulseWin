import { useState } from 'react'

export interface PrimerHours { start: number; end: number }

export interface PrimerStatus { lastRun: number | null; lastOutcome: string | null }

/// What the starter last did, in words rather than an outcome token: the
/// stored value is the starter's own string, and a build that adds a case must
/// still say something rather than print the raw word.
export function statusText(status: PrimerStatus): string {
  if (status.lastRun === null) return '尚未启动'
  const when = new Date(status.lastRun * 1000).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
  switch (status.lastOutcome) {
    case 'sent': return when
    case 'toolMissing': return `${when} · 找不到命令行工具`
    case 'failed': return `${when} · 没有发出去，请检查工具是否已登录`
    case 'timedOut': return `${when} · 没有及时应答`
    default: return when
  }
}

const HOURS = Array.from({ length: 24 }, (_, hour) => hour)
const hourText = (hour: number) => `${String(hour).padStart(2, '0')}:00`

/// The window starter's switches, on the provider's own pane.
///
/// **Switching it on goes through a confirmation that says plainly this is not
/// the provider's feature and may cost the account.** The original puts that in
/// an alert rather than a subtitle so the reader decides with it in front of
/// them; this does the same, in the page instead of a modal because the pane is
/// already a window.
export function WindowStarterGroup({ provider, primed, hours, status, disabled, onToggle, onHours }: {
  provider: 'claude-code' | 'codex'
  primed: boolean
  hours: PrimerHours
  status?: PrimerStatus
  disabled: boolean
  onToggle: (on: boolean) => void
  onHours: (hours: PrimerHours) => void
}) {
  const [confirming, setConfirming] = useState(false)
  const fiveHour = provider === 'claude-code'
    ? '5 小时额度重置时，通过 Claude Code 向 Haiku 发送一句“hi”，让下一个窗口从那时开始计时，而不是等你下次说话。不会保存任何内容。'
    : '5 小时或每周额度重置时，通过 Codex 向最便宜的模型发送一句“hi”，让下一个窗口从那时开始计时，而不是等你下次说话。不会保存任何内容。'
  return <section className="group-wrap"><h2>自动开启窗口</h2><div className="settings-group">
    <div className="setting-row"><div><strong>每次重置后开启新窗口</strong><small>{fiveHour}</small></div>
      <div className="setting-control"><input type="checkbox" role="switch" aria-label="每次重置后开启新窗口" checked={primed} disabled={disabled}
        onChange={e => { if (e.target.checked) setConfirming(true); else onToggle(false) }}/></div></div>
    {confirming && <>
      <div className="setting-row"><div><strong>请自行承担风险</strong><small>这不是 Anthropic 或 OpenAI 提供的功能。自动开启用量窗口可能被视为绕过用量限制，并可能导致账号被限制或停用。Pulse 只会通过你已经登录的工具发送一条短消息，不对你的账号因此发生的任何问题负责。只有接受这一点才请开启。</small></div>
        <div className="setting-control toolbar"><button disabled={disabled} onClick={() => setConfirming(false)}>取消</button><button className="primary" disabled={disabled} onClick={() => { setConfirming(false); onToggle(true) }}>我了解风险，开启</button></div></div>
    </>}
    {primed && <>
      <div className="setting-row"><div><strong>只在</strong><small>这个时间段之外的重置会等到它开始时再启动。Claude Code 与 Codex 共用。</small></div>
        <div className="setting-control toolbar">
          <select aria-label="开始时间" disabled={disabled} value={hours.start} onChange={e => onHours({ ...hours, start: Number(e.target.value) })}>{HOURS.map(hour => <option key={hour} value={hour}>{hourText(hour)}</option>)}</select>
          <span aria-hidden="true">–</span>
          <select aria-label="结束时间" disabled={disabled} value={hours.end} onChange={e => onHours({ ...hours, end: Number(e.target.value) })}>{HOURS.map(hour => <option key={hour} value={hour}>{hourText(hour)}</option>)}</select>
        </div></div>
      <div className="setting-row"><div><strong>上次启动</strong></div>
        <div className="setting-control"><span>{status ? statusText(status) : '尚未启动'}</span></div></div>
    </>}
  </div></section>
}
