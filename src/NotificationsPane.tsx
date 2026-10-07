import { useEffect, useState } from 'react'
import type { Preferences } from './preferences'

export function LowBalanceAlert({ value, currency, disabled, onChange }: {
  value?: number; currency: string; disabled: boolean; onChange: (value: number | undefined) => void
}) {
  const [text,setText]=useState(value===undefined?'':String(value))
  const [invalid,setInvalid]=useState(false)
  useEffect(()=>{setText(value===undefined?'':String(value));setInvalid(false)},[value])
  const commit=()=>{
    const next=text.trim()?Number(text):undefined
    if(next!==undefined&&(!Number.isFinite(next)||next<=0)){setInvalid(true);return}
    setInvalid(false);if(next!==value)onChange(next)
  }
  return <section className="group-wrap"><h2>余额提醒</h2><div className="settings-group">
    <div className="setting-row"><div><strong>余额低于</strong><small>{invalid?'请输入大于 0 的金额。':'留空关闭；按此账号报告的币种判断。'}</small></div>
      <div className="setting-control balance-input"><span>{currency}</span><input type="number" aria-label="余额提醒金额" aria-invalid={invalid} min="0" step="0.01" disabled={disabled} value={text} placeholder="关闭" onChange={e=>{setText(e.target.value);setInvalid(false)}} onBlur={commit} onKeyDown={e=>{if(e.key==='Enter')commit()}}/></div>
    </div>
  </div></section>
}

export function NotificationsPane({ alerts, disabled, onChange }: {
  alerts: Preferences['alerts']; disabled: boolean; onChange: (patch: Partial<Preferences['alerts']>) => void
}) {
  return <>
    <h1>通知</h1><p className="page-description">在 Windows 通知中心提醒额度状态。所有提醒默认关闭。</p>
    <section className="group-wrap"><h2>额度提醒</h2><div className="settings-group">
      <div className="setting-row"><div><strong>用量提醒最低门槛</strong><small>达到门槛且用量进度高于时间进度时提醒。</small></div><div className="setting-control"><select aria-label="额度提醒阈值" value={alerts.threshold ?? ''} disabled={disabled} onChange={e=>onChange({ threshold: e.target.value ? Number(e.target.value) : null })}>
        <option value="">关闭</option>{[60,70,75,80,85,90,95].map(v=><option value={v} key={v}>{v}%</option>)}
      </select></div></div>
      <label className="setting-row"><div><strong>额度重置时通知</strong><small>仅提醒此前已发出警告的窗口。</small></div><input type="checkbox" role="switch" aria-label="额度重置时通知" checked={alerts.onReset} disabled={disabled || alerts.threshold === null} onChange={e=>onChange({onReset:e.target.checked})}/></label>
      <label className="setting-row"><div><strong>连续检查失败时通知</strong><small>连续三次失败才提醒；有效旧读数在前 30 分钟受到保护。</small></div><input type="checkbox" role="switch" aria-label="连续检查失败时通知" checked={alerts.onFailure} disabled={disabled} onChange={e=>onChange({onFailure:e.target.checked})}/></label>
    </div></section>
    <p className="group-copy">例如门槛为 75%：时间已过 75%、额度已用 75% 不提醒；额度已用 80% 才提醒。周期进度由窗口时长和重置时间计算，缺少信息时不发送超前提醒；额度确实用尽仍会提醒。</p>
    <p className="group-copy">开启时也会检查当前有效读数，同一窗口不会反复提醒。声音和通知显示方式由 Windows 的应用通知设置控制。</p>
  </>
}
