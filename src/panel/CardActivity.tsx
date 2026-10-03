import type { CardHistory } from './cardHistory'
import { cardActivity } from './cardHistory'
import type { DetailCardLayout } from './cardLayout'
import { formatCost, shortTokens } from '../token-spend/summary'
import { useMemo } from 'react'

export function CardActivity({history,layout,now}:{history:CardHistory;layout:DetailCardLayout;now:number}) {
  const day=new Date(now).toDateString()
  const activity=useMemo(()=>history.snapshot?cardActivity(history.snapshot,new Date(now)):null,[history.snapshot,day])
  if(history.status==='disabled'||history.status==='unavailable')return null
  const s=layout.width/250
  const records=history.snapshot?.records.length??0
  const maximum=activity?activity.chart.reduce((most,day)=>Math.max(most,day.tokens),1):1
  const labels=['今天','7天','31天']
  const priced=!!activity && (activity.figures[2].cost??0)>0
  return <section className="card__activity" aria-label="本机用量记录" style={{gap:layout.activitySpacing,height:layout.activityHeight,fontSize:layout.footnoteFontSize}}>
    <div className="card__divider" style={{height:1}}/>
    <div className="card__muted" style={{height:13*s}}>{activity?.exported?'导入记录':'本机记录'}{activity?.partial?' · 计数可能不完整':''}</div>
    {history.status==='reading'?<div className="card__muted">正在读取本机记录…</div>:history.status==='failed'?<div className="card__muted">无法读取用量历史。</div>:!records?<div className="card__muted">暂无用量历史。</div>:activity&&<>
      <div className="card__figures" style={{height:(priced?47:32)*s,gap:8*s}}>{activity.figures.map((amount,i)=><div key={labels[i]} aria-label={labels[i]} style={{gap:2*s}}>
        <span className="card__muted" style={{height:13*s}}>{labels[i]}</span>
        <strong style={{height:17*s,fontSize:14*s}}>{shortTokens(amount.tokens)}</strong>
        {priced&&<span className="card__muted" style={{height:13*s}}>{amount.cost===null?'—':amount.cost>0?`≈${formatCost(amount.cost)}`:' '}{amount.unpricedTokens>0&&amount.cost!==null?' *':''}</span>}
      </div>)}</div>
      <div className="card__chart" role="img" aria-label="最近31天 Token 消耗" style={{height:30*s,gap:2*s}}>{activity.chart.map(day=><span key={day.day} title={`${day.day} · ${day.tokens.toLocaleString()} Token`} style={{height:day.tokens?`${Math.max(2,day.tokens/maximum*100)}%`:'1px'}}/>)}</div>
      <div className="card__row-line" style={{height:layout.rowTextLineHeight,fontSize:layout.rowFontSize}}><span className="card__muted">主要模型</span><span className="card__model" title={activity.top?.name}>{activity.top?`${activity.top.name} · ${Math.round(activity.top.share*100)}%`:'—'}</span></div>
      {activity.cacheHitRate!==null&&<div className="card__row-line" style={{height:layout.rowTextLineHeight,fontSize:layout.rowFontSize}}><span className="card__muted">缓存命中率</span><span className="card__model">{Math.round(activity.cacheHitRate*100)}%</span></div>}
      <div className="card__muted card__footnote" style={{height:13*s}} title={history.snapshot?.notes.join('；')}>{activity.partial?'计数可能不完整；金额仅为已读记录估算。':'金额按 API 价格估算。'}{activity.figures[2].unpricedTokens>0?'部分 Token 无公开价格。':''}</div>
    </>}
  </section>
}
