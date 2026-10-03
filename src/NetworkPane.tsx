import { useEffect, useId, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { type NetworkProxySettings, type Preferences } from './preferences'

interface NetworkStatus { settings: NetworkProxySettings; source: string; endpoint: string | null; manualReady: boolean }
export function NetworkPane({ preferences, disabled, onChange }: { preferences: Preferences; disabled: boolean; onChange: (patch: Partial<Preferences>) => void }) {
  const [status, setStatus] = useState<NetworkStatus | null>(null)
  const [host, setHost] = useState(''), [port, setPort] = useState('')
  const [busy, setBusy] = useState(false), [error, setError] = useState<string | null>(null)
  const live = useRef(false), pending = useRef(false), committed = useRef<NetworkProxySettings | null>(null), revision = useRef(0)
  const errorId = useId(), preferenceKey = JSON.stringify(preferences.networkProxy)
  const accept = (value: NetworkStatus) => {
    if (!live.current) return
    committed.current = value.settings; setStatus(value); setHost(value.settings.host); setPort(value.settings.port?.toString() ?? '')
  }
  const read = async () => {
    const version = ++revision.current
    setError(null)
    try { const value = await invoke<NetworkStatus>('get_network_status'); if (version === revision.current) accept(value) }
    catch (e) { if (live.current && version === revision.current) setError(String(e)) }
  }
  useEffect(() => { live.current = true; void read(); return () => { live.current = false; revision.current++ } }, [])
  useEffect(() => {
    if (committed.current && JSON.stringify(committed.current) !== preferenceKey && !pending.current) void read()
  }, [preferenceKey])
  const save = async (value: NetworkProxySettings) => {
    if (pending.current || !status) return
    pending.current = true; setBusy(true); setError(null); revision.current++
    try { accept(await invoke<NetworkStatus>('save_network_settings', { value })) }
    catch (e) { if (live.current) setError(String(e)) }
    finally { pending.current = false; if (live.current) setBusy(false) }
  }
  const dirty = !!status && (host !== status.settings.host || port !== (status.settings.port?.toString() ?? ''))
  const commitEndpoint = () => {
    if (!dirty || pending.current || !status) return
    if (!host.trim() || !/^\d+$/.test(port) || Number(port) < 1 || Number(port) > 65535) {
      setError('请填写代理主机和 1–65535 的整数端口；原设置已保留。'); return
    }
    void save({ ...status.settings, host: host.trim(), port: Number(port) })
  }
  const blocked = disabled || busy || !status
  return <>
    <h1>网络与刷新</h1><p className="page-description">配置用量请求的代理与检查频率。</p>
    <section className="group-wrap"><h2>网络代理</h2><div className="settings-group">
      <div className="setting-row"><div><strong>代理方式</strong><small>用量请求、登录交换、价格下载与本机助手命令使用此设置。</small></div><div className="setting-control">
        <select aria-label="代理方式" value={status?.settings.mode ?? 'system'} disabled={blocked} onChange={e => { if (committed.current) void save({ ...committed.current, mode: e.target.value as NetworkProxySettings['mode'] }) }}><option value="system">跟随系统</option><option value="manual">手动代理</option></select>
      </div></div>
      {status?.settings.mode === 'manual' && <>
        <div className="setting-row"><div><strong>代理类型</strong><small>支持 HTTP CONNECT 或 SOCKS5，无需用户名和密码。</small></div><div className="setting-control"><select aria-label="代理类型" value={status.settings.kind} disabled={blocked} onChange={e => { if (committed.current) void save({ ...committed.current, kind: e.target.value as NetworkProxySettings['kind'] }) }}><option value="http">HTTP</option><option value="socks5">SOCKS5</option></select></div></div>
        <div className="network-endpoint" onBlur={e => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) commitEndpoint() }} onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); commitEndpoint() } }}>
          <div className="setting-row"><label htmlFor={`${errorId}-host`}><strong>主机</strong><small>主机名或 IP 地址，不包含协议与端口。</small></label><div className="setting-control"><input id={`${errorId}-host`} aria-label="代理主机" aria-describedby={error ? errorId : undefined} aria-invalid={!!error} autoComplete="off" spellCheck={false} placeholder="127.0.0.1" disabled={blocked} value={host} onChange={e => { setHost(e.target.value); setError(null) }}/></div></div>
          <div className="setting-row"><label htmlFor={`${errorId}-port`}><strong>端口</strong><small>主机与端口一起保存，按 Enter 或离开输入区域生效。</small></label><div className="setting-control"><input id={`${errorId}-port`} aria-label="代理端口" aria-describedby={error ? errorId : undefined} aria-invalid={!!error} inputMode="numeric" autoComplete="off" placeholder="7897" disabled={blocked} value={port} onChange={e => { setPort(e.target.value); setError(null) }}/></div></div>
          <div className="group-actions"><span>无效输入不会替换原来的代理。</span><button disabled={blocked || !dirty} onClick={commitEndpoint}>保存代理</button></div>
        </div>
      </>}
      {error && <div className="group-copy network-error" role="alert" id={errorId}>{error}{!status && <button disabled={busy} onClick={() => { void read() }}>重新读取</button>}</div>}
      <div className="group-copy" role="status" aria-live="polite">{!status ? '正在读取代理设置…' : status.settings.mode === 'manual' && !status.manualReady ? '尚未填写有效的手动代理，暂时沿用系统设置。' : `当前来源：${status.source}`}{status?.endpoint && <code>{status.endpoint.replace('socks5h://', 'socks5://')}</code>}</div>
      <p className="group-copy">跟随系统时使用环境变量或 Windows 手动代理设置。手动代理绕过本机回环请求；软件更新独立跟随系统代理。</p>
    </div></section>
    <section className="group-wrap"><h2>刷新</h2><div className="settings-group"><div className="setting-row"><div><strong>检查间隔</strong><small>自动按工作活动和读数变化调整，仅检查已启用的账号。</small></div><div className="setting-control"><select aria-label="检查间隔" disabled={disabled || busy} value={preferences.refreshAutomatic ? 'automatic' : String(preferences.refreshSeconds)} onChange={e => onChange(e.target.value === 'automatic' ? { refreshAutomatic: true } : { refreshAutomatic: false, refreshSeconds: Number(e.target.value) })}><option value="automatic">自动（2–30 分钟）</option>{[120, 300, 600, 1800].map(v => <option key={v} value={v}>{v / 60} 分钟</option>)}</select></div></div></div></section>
  </>
}
