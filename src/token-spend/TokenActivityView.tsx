import { useMemo } from 'react'
import { activityStep, tokenActivity, type ActivityView } from './activity'
import { dayKey, exactTokenText, shortTokens } from './summary'
import type { SpendRecord } from './types'
import './activity.css'
const colours=['rgba(128,128,136,.12)','rgba(0,122,255,.30)','rgba(0,122,255,.52)','rgba(0,122,255,.76)','#007aff']
export function TokenActivityView({records,view,onChange,agent,model,partial}:{records:SpendRecord[];view:ActivityView;onChange:(view:ActivityView)=>void;agent:string|null;model:string|null;partial:boolean}){
  const activity=useMemo(()=>tokenActivity(records,new Date(),agent,model),[records,agent,model])
  const width=Math.max(activity.weeks.length,1)*10,height=70
  let running=0,exact:bigint|null=BigInt(0)
  const points=activity.weeks.flatMap((week,column)=>week.days.flatMap((d,row)=>{if(!d)return [];running+=d.tokens;exact=exact===null||d.exactTokens===null?null:exact+BigInt(d.exactTokens);return[{date:d.date,total:running,exact:exact?.toString(),x:(column+(row+.5)/7)*10,y:height-(activity.total>0?running/activity.total*height:0)}]}))
  const dateAt=(start:string,row:number)=>{const [y,m,d]=start.split('-').map(Number);return dayKey(new Date(y,m-1,d+row))}
  const path=points.map((p,i)=>`${i?'L':'M'}${p.x},${p.y}`).join(' '),peak=Math.max(1,...activity.weeks.map(w=>w.tokens))
  const caption=`最近 12 个月 ${shortTokens(activity.total)} Token，${activity.activeDays} 个活跃日`
  return <section className="spend-card token-activity"><div className="activity-heading"><h2>Token 活动</h2><div className="spend-segments" role="group" aria-label="Token 活动视图">{([['daily','每日'],['weekly','每周'],['cumulative','累计']] as const).map(([id,label])=><button key={id} aria-pressed={view===id} onClick={()=>onChange(id)}>{label}</button>)}</div></div>
    {!activity.weeks.length?<p className="spend-empty">尚无历史记录。</p>:<><p className="spend-footnote" title={exactTokenText(activity.total,activity.exactTotal??undefined)}>{caption}</p><svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={caption}>
      {view==='daily'?activity.weeks.flatMap((week,column)=>week.days.map((d,row)=>d||week.unrecorded[row]?<rect key={`${column}-${row}`} x={column*10+.7} y={row*10+.7} width="8.6" height="8.6" rx="1.3" fill={d?colours[activityStep(d.tokens,activity.cuts)]:'rgba(128,128,136,.055)'}><title>{d?`${d.date} · ${exactTokenText(d.tokens,d.exactTokens??undefined)} Token`:`${dateAt(week.start,row)} · 未记录`}</title></rect>:null)):
        view==='weekly'?activity.weeks.map((w,i)=>w.days.some(Boolean)?<rect key={w.start} x={i*10+1} y={height-Math.max(w.tokens>0?1:0,w.tokens/peak*(height-1))} width="8" height={Math.max(w.tokens>0?1:0,w.tokens/peak*(height-1))} rx="1" fill="#007aff"><title>{w.days.find(Boolean)?.date} 至 {[...w.days].reverse().find(Boolean)?.date} · {exactTokenText(w.tokens,w.exactTokens??undefined)} Token</title></rect>:null):<>
          {points.length>0&&<path d={`M${points[0].x},${height} ${path.replace(/^M/,'L')} L${points[points.length-1].x},${height} Z`} fill="rgba(0,122,255,.16)"/>}<path d={path} fill="none" stroke="#007aff" strokeWidth="1.2" strokeLinejoin="round"/>{points.map(p=><circle key={p.date} cx={p.x} cy={p.y} r="2" fill="transparent"><title>{p.date} 累计 · {exactTokenText(p.total,p.exact)} Token</title></circle>)}
        </>}
    </svg><div className="activity-months">{activity.marks.map(m=><span key={`${m.label}-${m.position}`} style={{left:`${Math.min(96,m.position/Math.max(activity.weeks.length,1)*100)}%`}}>{m.label}</span>)}</div>{view==='daily'&&<div className="activity-legend">少{colours.map(c=><i key={c} style={{background:c}}/>)}多</div>}{partial&&<p className="spend-footnote">计数可能不完整。</p>}<p className="spend-footnote">独立于上方时间范围。首次记录之前为未记录；之后没有工作的日期为零。</p></>}
  </section>
}
