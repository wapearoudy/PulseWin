import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { usePreferences, savePreferences, type Preferences } from './preferences'
import { providerSettings, type ProviderSetting } from './api'

export function TrayPreferencesPane(){
  const {preferences:p,loaded,error:readError}=usePreferences(),[accounts,setAccounts]=useState<ProviderSetting[]>([]),[busy,setBusy]=useState(false),[error,setError]=useState<string|null>(null)
  useEffect(()=>{let live=true;void providerSettings().then(v=>{if(live)setAccounts(v)}).catch(e=>{if(live)setError(String(e))});return()=>{live=false}},[p.accounts.length])
  const save=async(patch:Partial<Preferences>)=>{setBusy(true);setError(null);try{await savePreferences({...p,...patch})}catch(e){setError(String(e))}finally{setBusy(false)}}
  return <section className="group-wrap"><h2>托盘与桌面助手</h2><div className="settings-group">
    {(error||readError)&&<p className="settings-alert" role="alert">{error??readError}</p>}
    <div className="setting-row"><div><strong>显示桌面助手</strong><small>关闭后继续在托盘监测用量；重启后保留选择。</small></div><input type="checkbox" role="switch" aria-label="显示桌面助手" checked={p.panelVisible} disabled={!loaded||busy} onChange={e=>{const visible=e.target.checked;setBusy(true);setError(null);void invoke('set_panel_visible',{visible}).catch(e=>setError(String(e))).finally(()=>setBusy(false))}}/></div>
    <div className="setting-row"><div><strong>托盘显示用量</strong><small>左键打开用量概览，右键打开应用菜单。</small></div><input type="checkbox" role="switch" aria-label="托盘显示用量" checked={p.trayShowsUsage} disabled={!loaded||busy} onChange={e=>{void save({trayShowsUsage:e.target.checked})}}/></div>
    {p.trayShowsUsage&&<>
      <div className="setting-row"><div><strong>托盘账号</strong><small>自动选择使用最多的主配额，或指定账号。</small></div><select aria-label="托盘账号" value={p.trayAccount??''} disabled={!loaded||busy} onChange={e=>{void save({trayAccount:e.target.value||null})}}><option value="">自动 · 使用最多</option>{accounts.filter(a=>p.enabledProviders.includes(a.id)).map(a=><option key={a.id} value={a.id}>{p.accountLabels[a.id]??a.name}</option>)}</select></div>
      <div className="setting-row"><div><strong>托盘用量样式</strong><small>Windows 使用图标内数字；双额度仅显示服务商报告的两种时间窗口。</small></div><select aria-label="托盘用量样式" value={p.trayStyle} disabled={!loaded||busy} onChange={e=>{void save({trayStyle:e.target.value as Preferences['trayStyle']})}}><option value="figure">数字</option><option value="ring">迷你圆环</option><option value="split">双额度</option></select></div>
    </>}
    <div className="setting-row"><div><strong>用量概览</strong><small>所有已启用账号的配额、余额和重置时间。</small></div><button disabled={!loaded} onClick={()=>{void invoke('show_usage_dashboard').catch(e=>setError(String(e)))}}>打开概览</button></div>
  </div></section>
}
