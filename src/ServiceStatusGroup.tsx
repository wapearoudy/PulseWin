import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import './service-status.css'

type Health = 'operational' | 'degraded' | 'partialOutage' | 'fullOutage' | 'maintenance' | 'unrecognised'
interface StatusDay {date:string;state:Health|null;colour:string|null}
interface Reading {page:'openAI'|'claude'|'deepSeek';checkedAt:string;historyError:string|null;components:{id:string;name:string;state:Health;uptime:number|null;days:StatusDay[]}[]}
const labels:Record<Health,string>={operational:'运行正常',degraded:'性能下降',partialOutage:'部分故障',fullOutage:'全面故障',maintenance:'计划维护',unrecognised:'未知状态'}
const hosts:Record<string,string>={codex:'status.openai.com','claude-code':'status.claude.com',deepseek:'status.deepseek.com'}

export function ServiceStatusGroup({provider,base}:{provider:string;base:string}) {
  const [reading,setReading]=useState<Reading|null>(null),[error,setError]=useState<string|null>(null),[loading,setLoading]=useState(true)
  useEffect(()=>{
    let live=true
    const check=async()=>{try{const result=await invoke<Reading>('get_service_status',{provider});if(live){setReading(result??null);setError(result?null:'无法读取官方状态页')}}catch(e){if(live){setReading(null);setError(String(e))}}finally{if(live)setLoading(false)}}
    setReading(null);setError(null);setLoading(true);void check()
    const timer=window.setInterval(()=>{void check()},300_000)
    return()=>{live=false;window.clearInterval(timer)}
  },[provider])
  const host=hosts[base]
  if(!host)return null
  return <section className="group-wrap service-status" data-page={reading?.page} aria-label="官方服务状态"><h2>服务状态</h2><div className="settings-group">
    {loading?<p className="group-copy" role="status">正在读取官方状态…</p>:error?<p className="group-copy" role="status">{error}</p>:reading?.components.map(c=><div className="status-component" key={c.id}>
      <div className="status-heading"><strong>{c.name}</strong><span data-state={c.state}>{labels[c.state]??labels.unrecognised}</span></div>
      {c.days.length>0&&<><div className="status-days" role="list" aria-label={`${c.name} 历史状态`}>{c.days.map(d=><span role="listitem" key={d.date} data-state={d.state??'missing'} style={d.state&&/^#[\da-f]{6}$/i.test(d.colour??'')?{backgroundColor:d.colour!}:undefined} title={`${d.date} · ${d.state?(labels[d.state]??labels.unrecognised):'无数据'}`} />)}</div><div className="status-period"><span>{c.days[0].date}</span>{c.uptime!==null&&Number.isFinite(c.uptime)&&<span>{c.uptime.toLocaleString(undefined,{maximumFractionDigits:2})}% 正常运行 · 官方报告</span>}<span>{c.days[c.days.length-1]?.date}</span></div></>}
    </div>)}
    {reading?.historyError&&<p className="group-copy" role="status">{reading.historyError}；上方仍为本次读取的当前状态。</p>}
    <div className="setting-row"><div><strong>官方状态页</strong><small>{host}</small></div><a href={`https://${host}/`} target="_blank" rel="noreferrer" onClick={e=>{e.preventDefault();void invoke('plugin:opener|open_url',{url:`https://${host}/`}).catch(err=>setError(String(err)))}}>打开</a></div>
  </div><p className="group-copy">由服务商官方报告；进入此页面及停留期间每五分钟读取。{reading&&`最近检查：${new Date(reading.checkedAt).toLocaleString()}`}</p></section>
}
