import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { UpdatePane } from './UpdatePane'

interface AppInfo { version: string; licenses: { title: string; text: string }[] }
const links = [
  ['source', 'PulseWin 源代码', 'Windows 移植、问题反馈与发布记录。'],
  ['upstream', 'Pulse 原版', 'qunqin24 与贡献者，原版功能、设计与资源。'],
  ['vinz', 'Vinz', '原版设计灵感。'],
  ['icons', 'LobeIcons', '服务商图标。'],
  ['morphbot', 'MorphBot', '机器人动画参考。'],
]
export function AboutPane() {
  const [info, setInfo] = useState<AppInfo | null>(null), [error, setError] = useState<string | null>(null), [opening, setOpening] = useState<string | null>(null)
  useEffect(() => { let live = true; void invoke<AppInfo>('get_app_info').then(v => { if (live) setInfo(v) }).catch(e => { if (live) setError(String(e)) }); return () => { live = false } }, [])
  const open = async (key: string) => { setOpening(key); setError(null); try { await invoke('open_about_link', { key }) } catch (e) { setError(String(e)) } finally { setOpening(null) } }
  return <>
    <h1>关于</h1><p className="page-description">Pulse 的 Windows 移植版。</p>
    <section className="group-wrap"><h2>应用</h2><div className="settings-group"><div className="setting-row"><div><strong>PulseWin</strong><small>在桌面边缘查看 AI 服务用量。</small></div><span className="about-version" role="status">{info ? `版本 ${info.version}` : '正在读取版本…'}</span></div><p className="group-copy">用量由已启用账号的服务接口提供；Token 消耗开启后读取本机历史记录。缺少读数会保留明确的错误或过期标记。费用根据模型价格估算，具体账单以服务商为准。</p></div></section>
    <UpdatePane/>
    {error && <p className="settings-alert" role="alert">{error}</p>}
    <section className="group-wrap"><h2>开源与致谢</h2><div className="settings-group">{links.map(([key, title, subtitle]) => <div className="setting-row" key={key}><div><strong>{title}</strong><small>{subtitle}</small></div><button disabled={!!opening} aria-label={`打开${title}`} onClick={() => { void open(key) }}>查看 ↗</button></div>)}</div></section>
    <section className="group-wrap"><h2>许可声明</h2><div className="settings-group">{info?.licenses.map(license => <details key={license.title}><summary>{license.title}</summary><pre className="license-copy">{license.text}</pre></details>)}</div></section>
  </>
}
