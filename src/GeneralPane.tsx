import { useEffect, useId, useRef, useState, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { captureShortcut, heldModifiers, shortcutLabel } from './shortcut'
import { TrayPreferencesPane } from './TrayPreferencesPane'
import { UpdatePane } from './UpdatePane'
type Action='openSettings'|'togglePanel'
interface ApplicationSettings {
  openSettingsShortcut:string|null;togglePanelShortcut:string|null
  unavailable:Partial<Record<Action,string>>;autostartEnabled:boolean|null;autostartError:string|null
}
const rows:{action:Action;title:string;subtitle:string}[]=[
  {action:'openSettings',title:'打开设置',subtitle:'从其他应用直接打开这个设置窗口。'},
  {action:'togglePanel',title:'显示或隐藏面板',subtitle:'显示用量面板，或将它隐藏。'},
]
export interface GeneralPaneProps {onError?:(error:string|null)=>void}
export function GeneralPane({onError}:GeneralPaneProps={}) {
  const [settings,setSettings]=useState<ApplicationSettings|null>(null),[loading,setLoading]=useState(true)
  const [readError,setReadError]=useState<string|null>(null),[errors,setErrors]=useState<Partial<Record<Action|'autostart',string>>>({})
  const [busy,setBusy]=useState<Action|'autostart'|null>(null),[recording,setRecording]=useState<Action|null>(null),[held,setHeld]=useState(''),[needsModifier,setNeedsModifier]=useState(false)
  const mounted=useRef(true),id=useId(),onErrorRef=useRef(onError);onErrorRef.current=onError
  const stop=()=>{setRecording(null);setHeld('');setNeedsModifier(false)}
  const read=async()=>{
    setLoading(true);setReadError(null)
    try{const value=await invoke<ApplicationSettings>('get_application_settings');if(mounted.current)setSettings(value)}
    catch(error){if(mounted.current){setReadError(String(error));onErrorRef.current?.(String(error))}}
    finally{if(mounted.current)setLoading(false)}
  }
  useEffect(()=>{mounted.current=true;void read();return()=>{mounted.current=false}},[])
  const save=async(action:Action|'autostart',value:string|null|boolean)=>{
    stop();setBusy(action);setErrors(previous=>({...previous,[action]:undefined}));onErrorRef.current?.(null)
    try{
      const next=await invoke<ApplicationSettings>(action==='autostart'?'set_autostart':'set_application_shortcut',action==='autostart'?{enabled:value}:{action,value})
      if(mounted.current)setSettings(next)
    }catch(error){if(mounted.current){setErrors(previous=>({...previous,[action]:String(error)}));onErrorRef.current?.(String(error))}}
    finally{if(mounted.current)setBusy(null)}
  }
  const saveRef=useRef(save);saveRef.current=save
  useEffect(()=>{
    if(!recording)return
    const down=(event:KeyboardEvent)=>{
      // Capture is local to this settings window. During recording even a
      // partial combination must not reach a text field or a browser menu.
      event.preventDefault();event.stopImmediatePropagation()
      const result=captureShortcut(event)
      if(result.kind==='held'){setHeld(result.label);if(event.ctrlKey||event.altKey||event.metaKey)setNeedsModifier(false)}
      else if(result.kind==='cancel')stop()
      else if(result.kind==='clear')void saveRef.current(recording,null)
      else if(result.kind==='invalid')setNeedsModifier(true)
      else void saveRef.current(recording,result.value)
    }
    const up=(event:KeyboardEvent)=>{event.preventDefault();event.stopImmediatePropagation();setHeld(heldModifiers(event))}
    const visibility=()=>{if(document.hidden)stop()}
    window.addEventListener('keydown',down,true);window.addEventListener('keyup',up,true);window.addEventListener('blur',stop);document.addEventListener('visibilitychange',visibility)
    return()=>{window.removeEventListener('keydown',down,true);window.removeEventListener('keyup',up,true);window.removeEventListener('blur',stop);document.removeEventListener('visibilitychange',visibility)}
  },[recording])
  const disabled=loading||!settings||busy!==null,autostartProblem=errors.autostart??settings?.autostartError
  return <>
    <h1>通用</h1><p className="page-description">启动方式、键盘快捷键与软件更新。</p>
    {readError&&<div className="settings-alert" role="alert"><span>{readError}</span><button disabled={loading} onClick={()=>{void read()}}>重新读取</button></div>}
    <Group title="应用"><Row title="登录时启动 Pulse" subtitle={autostartProblem??(loading?'正在读取登录启动状态…':settings?.autostartEnabled===null?'登录启动状态未知':'登录 Windows 时自动打开 Pulse。')} problem={!!autostartProblem}>
      <input type="checkbox" role="switch" aria-label="登录时启动 Pulse" checked={settings?.autostartEnabled===true} disabled={disabled||settings?.autostartEnabled===null} onChange={event=>{void save('autostart',event.target.checked)}}/>
    </Row></Group>
    <Group title="快捷键">{rows.map(row=>{
      const value=row.action==='openSettings'?settings?.openSettingsShortcut:settings?.togglePanelShortcut,problem=errors[row.action]??settings?.unavailable[row.action],active=recording===row.action,description=`${id}-${row.action}`
      return <Row key={row.action} title={row.title} subtitle={problem??row.subtitle} subtitleId={description} problem={!!problem}>
        <div className="toolbar" style={{gap:6}}>
          <button type="button" aria-label={`录入${row.title}快捷键`} aria-describedby={description} aria-pressed={active} disabled={disabled} title="点击后按下组合键；Esc 取消，Backspace 或 Delete 清除。" style={{minWidth:130,color:active?'#007aff':undefined}} onClick={()=>{if(active)stop();else{setRecording(row.action);setHeld('');setNeedsModifier(false)}}}>{active?(needsModifier?'请加 Ctrl、Alt 或 Win':held||'请按组合键…'):shortcutLabel(value)}</button>
          <button type="button" aria-label={`清除${row.title}快捷键`} disabled={disabled||!value||active} style={{visibility:value&&!active?'visible':'hidden',padding:'5px 9px'}} onClick={()=>{void save(row.action,null)}}>×</button>
        </div>
      </Row>
    })}</Group>
    <TrayPreferencesPane/>
    <UpdatePane/>
  </>
}
function Group({title,children}:{title:string;children:ReactNode}){return <section className="group-wrap"><h2>{title}</h2><div className="settings-group">{children}</div></section>}
function Row({title,subtitle,subtitleId,problem,children}:{title:string;subtitle?:string;subtitleId?:string;problem?:boolean;children:ReactNode}){return <div className="setting-row"><div><strong>{title}</strong>{subtitle&&<small id={subtitleId} role={problem?'alert':undefined}>{subtitle}</small>}</div><div className="setting-control">{children}</div></div>}
export default GeneralPane
