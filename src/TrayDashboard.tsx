import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getSnapshot, onUsageUpdated, openSettings, refreshNow, refreshProvider } from './api'
import { usePreferences } from './preferences'
import { ProviderIcon } from './panel/ProviderIcon'
import { headlineWindow, percentFigure } from './usagePresentation'
import { creditText } from './creditAmount'
import type { ProviderUsage, Snapshot, UsageWindow } from './types'
import { amounts, dayKey, formatCost, shortTokens } from './token-spend/summary'
import { cardActivity, useCardHistory } from './panel/cardHistory'
import './tray.css'

const close=()=>invoke<void>('hide_usage_dashboard')
export function quotaFigure(w:UsageWindow|undefined,remaining:boolean){
  return w?.percentUsed!=null && Number.isFinite(w.percentUsed) ? `${percentFigure(w.percentUsed/100,remaining)}%` : '—'
}
function resetText(w:UsageWindow,now:number){
  if(!w.resetsAt)return '未提供重置时间'
  const reset=Date.parse(w.resetsAt);if(!Number.isFinite(reset))return '未提供重置时间'
  const minutes=Math.ceil((reset-now)/60000)
  if(minutes<=0)return '等待重置'
  if(minutes<60)return `${minutes} 分钟后重置`
  if(minutes<1440)return `${Math.floor(minutes/60)} 小时${minutes%60?` ${minutes%60} 分钟`:''}后重置`
  return `${Math.floor(minutes/1440)} 天 ${Math.floor(minutes%1440/60)} 小时后重置`
}
function updated(at:string,now:number){const age=now-Date.parse(at);return !Number.isFinite(age)?'更新时间未知':age<60000?'刚刚更新':`${Math.floor(age/60000)} 分钟前更新`}

export default function TrayDashboard(){
  const {preferences:prefs,loaded,error:prefError}=usePreferences()
  const [snapshot,setSnapshot]=useState<Snapshot|null>(null),[selected,setSelected]=useState<string|null>(null)
  const [pages,setPages]=useState<Record<string,string>>({}),[error,setError]=useState<string|null>(null)
  const [busy,setBusy]=useState(false),[visibilityBusy,setVisibilityBusy]=useState(false),[now,setNow]=useState(Date.now())
  const snapshotRevision=useRef(0)
  useEffect(()=>{
    let live=true;const stops:(()=>void)[]=[]
    const run=async()=>{
      try{
        const off=await onUsageUpdated(value=>{snapshotRevision.current++;if(live)setSnapshot(value)});if(!live){off();return}stops.push(off)
        const version=snapshotRevision.current,value=await getSnapshot();if(live&&version===snapshotRevision.current)setSnapshot(value)
      }catch(e){if(live)setError(String(e))}
    }
    void run();void invoke<Record<string,string>>('get_usage_pages').then(v=>{if(live)setPages(v)}).catch(()=>{})
    const key=(e:KeyboardEvent)=>{if(e.key==='Escape'){e.preventDefault();void close().catch(e=>setError(String(e)))}}
    window.addEventListener('keydown',key);const clock=setInterval(()=>setNow(Date.now()),30000)
    return()=>{live=false;stops.forEach(off=>off());clearInterval(clock);window.removeEventListener('keydown',key)}
  },[])
  const accounts=(snapshot?.providers??[]).filter(p=>prefs.enabledProviders.includes(p.id)).sort((a,b)=>prefs.providerOrder.indexOf(a.id)-prefs.providerOrder.indexOf(b.id))
  const account=accounts.find(p=>p.id===selected)
  useEffect(()=>{if(snapshot&&loaded&&selected&&!account)setSelected(null)},[snapshot,loaded,selected,account])
  useEffect(()=>{
    let live=true,off:(()=>void)|undefined
    void listen<{ids:string[];refreshing:boolean}>('refresh-changed',e=>{
      if(live && (!selected||e.payload.ids.includes(selected)))setBusy(e.payload.refreshing)
    }).then(stop=>{if(live)off=stop;else stop()})
    return()=>{live=false;off?.();setBusy(false)}
  },[selected])
  const action=async(work:()=>Promise<unknown>)=>{
    setError(null)
    try{await work()}catch(e){setError(String(e))}
  }
  const refresh=()=>action(async()=>{
    setBusy(true);try{await (account?refreshProvider(account.id):refreshNow());const revision=snapshotRevision.current,value=await getSnapshot();if(revision===snapshotRevision.current)setSnapshot(value)}finally{setBusy(false)}
  })
  const content=useRef<HTMLDivElement|null>(null)
  useEffect(()=>{
    const node=content.current;if(!node)return
    let last=0;const measure=()=>{
      const frame=node.closest('.tray-dashboard')!
      const height=Math.ceil(node.getBoundingClientRect().height+Array.from(frame.querySelectorAll('.tray-header,.tray-tabs,.tray-footer')).reduce((sum,e)=>sum+e.getBoundingClientRect().height,0)+26)
      if(height!==last){last=height;void invoke('resize_usage_dashboard',{height}).catch(()=>{})}
    }
    const observer=new ResizeObserver(measure);observer.observe(node);for(const e of node.closest('.tray-dashboard')!.querySelectorAll('.tray-header,.tray-tabs,.tray-footer'))observer.observe(e);measure();return()=>observer.disconnect()
  },[])
  const tabs=useRef<HTMLElement|null>(null)
  const tabKey=(e:React.KeyboardEvent)=>{
    const items=Array.from(tabs.current?.querySelectorAll<HTMLButtonElement>('[role=tab]')??[]),index=items.indexOf(e.target as HTMLButtonElement)
    let next=-1;if(e.key==='ArrowRight')next=(index+1)%items.length;if(e.key==='ArrowLeft')next=(index+items.length-1)%items.length;if(e.key==='Home')next=0;if(e.key==='End')next=items.length-1
    if(next>=0){e.preventDefault();items[next]?.focus();items[next]?.click()}
  }
  return <div className="tray-dashboard">
    <header className="tray-header"><strong>Pulse</strong><span>用量</span><button aria-label="关闭用量概览" onClick={()=>{void action(close)}}>×</button></header>
    <nav ref={tabs} className={`tray-tabs ${accounts.length>5?'icons-only':''}`} role="tablist" aria-label="用量账号" onKeyDown={tabKey}>
      <button role="tab" aria-selected={!account} tabIndex={!account?0:-1} aria-controls="tray-content" id="tab-overview" onClick={()=>setSelected(null)} title="概览"><span className="overview-mark" aria-hidden>▦</span><span>概览</span></button>
      {accounts.map(p=><button key={p.id} role="tab" aria-label={p.name} aria-selected={account?.id===p.id} tabIndex={account?.id===p.id?0:-1} id={`tab-${p.id}`} aria-controls="tray-content" title={p.name} onClick={()=>setSelected(p.id)}><span className="tray-provider-icon"><ProviderIcon provider={p.id}/></span><span>{p.name}</span></button>)}
    </nav>
    <main id="tray-content" role="tabpanel" aria-labelledby={account?`tab-${account.id}`:'tab-overview'} tabIndex={0}><div className="tray-content-inner" ref={content}>
      {(error||prefError)&&<p className="tray-error" role="alert">{error??prefError}</p>}
      {!loaded||!snapshot?<p className="tray-muted" role="status">正在读取用量…</p>:account?<AccountDetail key={account.id} account={account} remaining={prefs.showsRemaining} warningAt={prefs.warningThreshold} now={now} readsSpend={prefs.tokenSpendEnabled}/>:accounts.length?<div className="tray-overview">{accounts.map(p=>{
        const w=headlineWindow(p,prefs.pinnedWindows[p.id]);return <button key={p.id} onClick={()=>setSelected(p.id)} aria-label={`查看${p.name}用量`}>
          <span className="tray-provider-icon"><ProviderIcon provider={p.id}/></span><span className="tray-row-name"><strong>{p.name}</strong><small>{p.stale?'旧读数 · ':''}{w?`${w.label} · ${resetText(w,now)}`:p.error||'暂无用量数字'}</small></span>
          <span className={w?.isExhausted||(w?.percentUsed??0)>=prefs.warningThreshold*100?'tray-warning':''}>{w?.percentUsed!=null?quotaFigure(w,prefs.showsRemaining):creditText(p.creditRemaining)??'—'}</span>
        </button>
      })}<p className="tray-muted tray-reading-label">{prefs.showsRemaining?'显示剩余额度':'显示已用额度'}</p></div>:<p className="tray-muted">尚未选择监控账号。请在设置中添加。</p>}
    </div></main>
    <footer className="tray-footer">
      {account&&pages[account.id.split('--account-')[0]]&&<button onClick={()=>{void action(()=>invoke('open_usage_page',{account:account.id}))}}>打开官方用量页面 <span>↗</span></button>}
      <button disabled={busy||!accounts.length||!loaded} onClick={()=>{void refresh()}}>{busy?'正在刷新…':account?'刷新此账号':'刷新全部账号'}<span>↻</span></button>
      <button disabled={visibilityBusy||!loaded} aria-pressed={prefs.panelVisible} onClick={()=>{void action(async()=>{setVisibilityBusy(true);try{await invoke('set_panel_visible',{visible:!prefs.panelVisible})}finally{setVisibilityBusy(false)}})}}>{prefs.panelVisible?'隐藏桌面助手':'显示桌面助手'}<span>{prefs.panelVisible?'✓':''}</span></button>
      <div className="tray-footer-bottom"><button onClick={()=>{void action(()=>openSettings(account?.id))}}>设置…</button><button onClick={()=>{void action(()=>invoke('quit_application'))}}>退出</button></div>
    </footer>
  </div>
}
function AccountDetail({account:p,remaining,warningAt,now,readsSpend}:{account:ProviderUsage;remaining:boolean;warningAt:number;now:number;readsSpend:boolean}){
  const local=!p.id.includes('--account-')&&['claude-code','codex','kimi-code','grok','opencode-go','cursor','antigravity','command-code','copilot'].includes(p.id)
  const history=useCardHistory(p.id,local,readsSpend),credit=creditText(p.creditRemaining)
  return <article className="tray-detail">
    <div className="tray-detail-title"><h1>{p.name}</h1><span>{p.plan}</span></div>
    <p className="tray-muted">{updated(p.fetchedAt,now)}{p.stale?' · 旧读数':''}</p>
    {p.account&&<p className="tray-muted tray-account-address" title={p.account}>{p.account}</p>}
    {p.error&&<p className="tray-error">{p.error}</p>}
    {!p.windows.length&&!credit&&<p className="tray-muted">服务商尚未提供用量数字。</p>}
    {p.windows.map((w,i)=>{
      const used=w.percentUsed,known=used!=null&&Number.isFinite(used),spent=w.isExhausted,warning=spent||(used??0)>=warningAt*100
      const value=known?Math.max(0,Math.min(100,remaining&&!spent?100-used!:used!)):null
      return <section className="tray-quota" key={`${w.id??w.label}-${i}`}>
        <div><strong>{w.label}</strong><small>{resetText(w,now)}</small></div>
        {known?<div className={`tray-progress ${warning?'tray-warning':''}`} role="progressbar" aria-label={w.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={value!} aria-valuetext={spent?'已限额':`${quotaFigure(w,remaining)}${remaining?'剩余':'已用'}`}><span style={{width:`${value}%`}}/></div>:<div className="tray-progress" aria-label={`${w.label}暂无数字`}/>}
        <p className={warning?'tray-warning':'tray-muted'}>{quotaFigure(w,remaining)} {remaining?'剩余':'已用'}{spent?' · 已限额':''}</p>
        {w.detail&&<small className="tray-muted">{w.detail}</small>}
      </section>
    })}
    {credit&&<section className="tray-credit"><strong>余额</strong><p>{credit} 剩余</p></section>}
    {readsSpend&&local&&<section className="tray-spend">{history.status==='ready'&&history.snapshot?.records.length?<Spend history={history.snapshot}/>:<p className="tray-muted">{history.status==='reading'?'正在读取本机记录…':history.status==='failed'?'本机记录读取失败':history.status==='unavailable'?'没有可归属此账号的本机记录':'暂无消费历史'}</p>}</section>}
  </article>
}
function Spend({history}:{history:NonNullable<ReturnType<typeof useCardHistory>['snapshot']>}){
  const activity=cardActivity(history),recent=history.records.filter(r=>r.day>=dayKey(new Date(new Date().getFullYear(),new Date().getMonth(),new Date().getDate()-30)))
  const daily=new Map<string,typeof recent>();recent.forEach(r=>daily.set(r.day,[...(daily.get(r.day)??[]),r]))
  const busiest=[...daily.values()].map(rows=>amounts(rows)).sort((a,b)=>b.tokens-a.tokens)[0]
  const figures=[activity.figures[0],activity.figures[2],busiest,amounts(history.records)],titles=['今天','近 31 天','最忙的一天','全部记录']
  const max=Math.max(1,...activity.chart.map(d=>d.tokens))
  return <><h2>Token 消耗</h2><div className="tray-spend-figures">{figures.map((f,i)=><div key={i}><small>{titles[i]}</small><strong>{f?formatCost(f.cost):'—'}</strong><span>{f?shortTokens(f.tokens):'—'} tokens</span></div>)}</div>
    <div className="tray-spend-chart" role="img" aria-label="近 31 天 Token 消耗">{activity.chart.map(d=><span key={d.day} title={`${d.day} · ${shortTokens(d.tokens)} tokens`} style={{height:d.tokens?`${Math.max(3,d.tokens/max*100)}%`:'0%'}}/>)}</div>
    <p className="tray-muted">按 API 价格估算，并非订阅账单。{history.notes.length?'记录不完整，费用可能无法计算。':''}</p></>
}
