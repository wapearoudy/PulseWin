import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

interface UpdateSettings {source:string;automatic:boolean;backupAutomatic?:boolean;notesLanguage?:string}
interface UpdateStatus {
  currentVersion:string;settings:UpdateSettings
  phase:'idle'|'checking'|'available'|'current'|'downloading'|'installing'|'error'
  version:string|null;notes:string|null;notesByLanguage?:Record<string,string>;checkedSource?:string|null;downloaded:number;total:number|null;checkedAt:string|null;error:string|null
}
export function UpdatePane({checkOnOpen=false}:{checkOnOpen?:boolean}) {
  const [status,setStatus]=useState<UpdateStatus|null>(null),[source,setSource]=useState(''),[saving,setSaving]=useState(false),[problem,setProblem]=useState<string|null>(null)
  const alive=useRef(false),sourceDirty=useRef(false)
  const accept=(value:UpdateStatus)=>{
    if(!alive.current||!value)return
    setStatus(value);if(!sourceDirty.current)setSource(value.settings.source)
  }
  useEffect(()=>{
    alive.current=true;let unlisten:(()=>void)|undefined
    void listen<UpdateStatus>('update-status',event=>accept(event.payload)).then(async stop=>{
      if(!alive.current){stop();return}unlisten=stop
      try{accept(await invoke<UpdateStatus>('get_update_status'));if(checkOnOpen&&alive.current)accept(await invoke<UpdateStatus>('check_app_update'))}catch(error){if(alive.current)setProblem(String(error))}
    }).catch(error=>{if(alive.current)setProblem(String(error))})
    return()=>{alive.current=false;unlisten?.()}
  },[checkOnOpen])
  const action=async(command:string,args?:Record<string,unknown>)=>{
    setProblem(null)
    try{const value=await invoke<UpdateStatus|undefined>(command,args);if(value)accept(value)}
    catch(error){if(alive.current)setProblem(String(error))}
  }
  const save=async(patch:Partial<UpdateSettings>={})=>{
    setSaving(true);setProblem(null)
    try{const value=await invoke<UpdateStatus>('save_update_settings',{settings:{automatic:true,backupAutomatic:true,notesLanguage:'auto',...status?.settings,source,...patch}});sourceDirty.current=false;accept(value)}
    catch(error){if(alive.current)setProblem(String(error))}
    finally{if(alive.current)setSaving(false)}
  }
  const busy=saving||!!status&&['checking','downloading','installing'].includes(status.phase)
  const language=status?.settings.notesLanguage??'auto'
  const requested=language==='auto'?(navigator.language.toLowerCase().startsWith('zh')?(navigator.language.toLowerCase().includes('tw')||navigator.language.toLowerCase().includes('hk')?'zh-TW':'zh-CN'):navigator.language.split('-')[0]):language
  const translated=status?.notesByLanguage?.[requested],notes=translated??status?.notes
  const message=!status?'正在读取更新状态…':status.phase==='checking'?'正在检查新版本…':status.phase==='available'?`新版本 ${status.version} 可用`:status.phase==='current'?'当前已是更新来源中的最新版本':status.phase==='downloading'?status.total?`正在下载 ${Math.min(100,Math.floor(status.downloaded/status.total*100))}%`:'正在下载更新…':status.phase==='installing'?'正在安装，完成后会自动重启…':status.phase==='error'?status.error:'可以检查新版本'
  return <section className="group-wrap" aria-label="软件更新">
    <h2>软件更新</h2><div className="settings-group">
      <div className="setting-row"><div><strong>PulseWin {status?.currentVersion??''}</strong><small role="status" aria-live="polite">{message}</small>{status?.checkedAt&&<small>上次检查：{new Date(status.checkedAt).toLocaleString()}</small>}</div>
        <div className="toolbar"><button disabled={!status||busy||sourceDirty.current} onClick={()=>{void action('check_app_update')}}>检查更新</button>
          {status?.phase==='available'&&<button className="primary" disabled={busy||sourceDirty.current} onClick={()=>{void action('install_app_update',{version:status.version})}}>更新并重启</button>}
        </div>
      </div>
      <div className="setting-row"><div><strong>自动检查更新</strong><small>启动后和运行期间检查，有新版时由你选择更新。</small></div><div className="setting-control"><input type="checkbox" role="switch" aria-label="自动检查更新" checked={status?.settings.automatic??false} disabled={!status||busy||sourceDirty.current} onChange={event=>{void save({automatic:event.target.checked})}}/></div></div>
      <div className="setting-row"><div><strong>自动尝试备用通道</strong><small>GitHub 主通道读取失败时，尝试同一仓库的官方 API；更新包仍须通过签名校验。</small></div><div className="setting-control"><input type="checkbox" role="switch" aria-label="自动尝试备用通道" checked={status?.settings.backupAutomatic??true} disabled={!status||busy||sourceDirty.current} onChange={event=>{void save({backupAutomatic:event.target.checked})}}/></div></div>
      <div className="setting-row"><div><strong>发布说明语言</strong><small>没有所选语言的译文时显示发布者原文。</small></div><select aria-label="发布说明语言" value={language} disabled={!status||busy||sourceDirty.current} onChange={event=>{void save({notesLanguage:event.target.value})}}>{[['auto','跟随系统'],['zh-CN','简体中文'],['zh-TW','繁體中文'],['en','English'],['ja','日本語'],['ko','한국어']].map(([value,label])=><option key={value} value={value}>{label}</option>)}</select></div>
      <div className="setting-row"><div><strong>更新来源</strong><small>本机更新目录或 HTTPS 发布地址。保存后检查即可。</small></div></div>
      <div className="group-copy" style={{display:'flex',gap:8,paddingBottom:12}}><input className="update-source" style={{flex:1,minWidth:0}} aria-label="更新来源" value={source} disabled={!status||busy} onChange={event=>{sourceDirty.current=true;setSource(event.target.value)}}/><button disabled={!status||busy||!sourceDirty.current} onClick={()=>{void save()}}>保存来源</button></div>
      {status?.phase==='downloading'&&<div className="group-copy"><progress aria-label="更新下载进度" max={status.total??undefined} value={status.total?status.downloaded:undefined}/></div>}
      {status?.checkedSource==='github-api'&&<p className="group-copy">已通过 GitHub 官方 API 备用通道检查。</p>}
      {status?.phase==='available'&&notes&&<div className="group-copy"><small>{translated?'发布说明':requested==='zh-CN'?'发布说明 · 原文':'发布说明 · 所选语言未提供，显示原文'}</small><p style={{whiteSpace:'pre-wrap'}}>{notes}</p></div>}
      {(problem||status?.phase==='error')&&<p className="group-copy" role="alert">{problem??status?.error}</p>}
      <p className="group-copy">更新包会校验签名；账号、凭据和面板设置保留。</p>
    </div>
  </section>
}
