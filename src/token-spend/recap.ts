// Port of Pulse 1.9 RecapBuild/RecapPeriods/RecapDeck rules (Apache-2.0).
import { amounts, dayKey, summarizeBetween, totalTokens } from './summary'
import type { SpendRecord } from './types'
export type RecapCard='poster'|'opener'|'calendar'|'yearCalendar'|'months'|'timetable'|'payback'|'scorecard'
export const localDate=(key:string)=>{const [y,m=1,d=1]=key.split('-').map(Number);return new Date(y,m-1,d)}
export const addDay=(date:Date,n:number)=>new Date(date.getFullYear(),date.getMonth(),date.getDate()+n)
const daysBetween=(a:Date,b:Date)=>Math.round((Date.UTC(b.getFullYear(),b.getMonth(),b.getDate())-Date.UTC(a.getFullYear(),a.getMonth(),a.getDate()))/86400000)
export function recapPrice(text:string):number|null|'invalid'{
  const value=text.trim().replace(/^\$\s*/,'');if(!value)return null
  let number:string
  if(/^\d+(?:[.,]\d{1,2})?$/.test(value))number=value.replace(',','.')
  else if(/^\d{1,3}(?:,\d{3})+(?:\.\d{1,2})?$/.test(value))number=value.replace(/,/g,'')
  else return 'invalid'
  const amount=Number(number);return Number.isFinite(amount)&&amount<=10000?(amount===0?null:amount):'invalid'
}
export function recapPeriods(records:SpendRecord[],year:boolean,now=new Date()){
  const today=dayKey(now),first=records.filter(r=>totalTokens(r)>0&&r.day<=today).map(r=>r.day).sort()[0]
  const floor=first?localDate(first<'2020-01-01'?'2020-01-01':first):now
  const keys:string[]=[]
  if(year){for(let y=now.getFullYear();y>=floor.getFullYear();y--)keys.push(String(y))}
  else{for(let i=now.getFullYear()*12+now.getMonth(),last=floor.getFullYear()*12+floor.getMonth();i>=last;i--)keys.push(`${Math.floor(i/12)}-${String(i%12+1).padStart(2,'0')}`)}
  const desired=year?String(now.getFullYear()-(now.getMonth()===0&&now.getDate()<=7?1:0)):dayKey(new Date(now.getFullYear(),now.getMonth()-(now.getDate()<=7?1:0),1)).slice(0,7)
  return {keys,defaultKey:keys.includes(desired)?desired:keys[keys.length-1]??desired}
}
export function workdays(instants:number[]){
  const sorted=[...new Set(instants)].sort((a,b)=>a-b),out:{endMinute:number;crossesMidnight:boolean}[]=[]
  let first=sorted[0],last=first
  const close=()=>{if(first===undefined||last-first>20*3600)return;const d=new Date(first*1000),owner=d.getHours()<5?addDay(d,-1):d,midnight=new Date(owner.getFullYear(),owner.getMonth(),owner.getDate());out.push({endMinute:Math.floor((last*1000-midnight.getTime())/60000),crossesMidnight:last*1000>=addDay(midnight,1).getTime()})}
  for(const t of sorted.slice(1)){if(t-last>3*3600){close();first=t}last=t}close();return out
}
export function buildRecap(records:SpendRecord[],key:string,monthlyPrice:number|null,now=new Date(),isPartial=false){
  const isYear=/^\d{4}$/.test(key),valid=isYear?/^20\d{2}$/.test(key):/^20\d{2}-(?:0[1-9]|1[012])$/.test(key)
  if(!valid)throw new Error('回顾日期无效')
  const start=localDate(key),fullEnd=new Date(start.getFullYear()+(isYear?1:0),start.getMonth()+(isYear?0:1),1),tomorrow=addDay(now,1),end=tomorrow<start?start:tomorrow<fullEnd?new Date(tomorrow.getFullYear(),tomorrow.getMonth(),tomorrow.getDate()):fullEnd
  const summary=summarizeBetween(records,start,end),isInProgress=end<fullEnd&&end>start
  const first=records.filter(r=>totalTokens(r)>0&&r.day<dayKey(end)).map(r=>r.day).sort()[0],recordsBegin=first&&first>dayKey(start)?localDate(first):null
  const observedDays=daysBetween(recordsBegin??start,end),elapsedDays=daysBetween(start,end)
  const previousStart=new Date(start.getFullYear()-(isYear?1:0),start.getMonth()-(isYear?0:1),1),previousEnd=isInProgress?new Date(Math.min(addDay(previousStart,elapsedDays).getTime(),start.getTime())):start
  const previousFrom=first&&localDate(first)>previousStart?localDate(first):previousStart,previous=summarizeBetween(records,previousFrom,previousEnd)
  const change=previous.tokens>0&&observedDays>0&&previous.days.length>0?(summary.tokens/observedDays)/(previous.tokens/previous.days.length)-1:null
  const timed=summary.records.filter(r=>!r.aggregateTiming),hours=Array<number>(24).fill(0);for(const r of timed)hours[r.hour]+=totalTokens(r)
  const timedTokens=timed.reduce((n,r)=>n+totalTokens(r),0),untimedTokens=summary.tokens-timedTokens,peak=Math.max(...hours),peakHour=peak>0?hours.indexOf(peak):null
  const persona=timedTokens>0?([21,22,23,0,1,2,3,4].reduce((n,h)=>n+hours[h],0)>timedTokens/2?'夜猫子':hours.slice(5,10).reduce((a,b)=>a+b,0)>timedTokens/2?'早起鸟':hours.slice(10,18).reduce((a,b)=>a+b,0)>timedTokens/2?'日间工作者':'全天候'):null
  const stretches=workdays(timed.flatMap(r=>r.sourceTimestamp==null?[]:[r.sourceTimestamp])),latestMinute=stretches.length?Math.max(...stretches.map(s=>s.endMinute)):null
  let current=0,longest=0,run=0
  for(const day of summary.days){run=day.tokens>0?run+1:0;longest=Math.max(longest,run)}
  if(isInProgress){let i=summary.days.length-1;if(summary.days[i]?.tokens===0)i--;for(;i>=0&&summary.days[i].tokens>0;i--)current++}
  const busiest=summary.days.reduce<typeof summary.days[number]|null>((a,b)=>b.tokens>(a?.tokens??0)?b:a,null)
  const months=Array.from({length:12},(_,m)=>{const rows=summary.records.filter(r=>r.day.slice(5,7)===String(m+1).padStart(2,'0'));return {month:m+1,...amounts(rows),future:new Date(start.getFullYear(),m,1)>=end,activeDays:new Set(rows.map(r=>r.day)).size}})
  const cacheAgents=summary.agents.filter(a=>a.unclassifiedTokens===0&&!a.repliesWithoutCacheFields),cacheInput=cacheAgents.reduce((s,a)=>s+a.tally.input+a.tally.cacheWrite+a.tally.cacheRead,0),cacheRead=cacheAgents.reduce((s,a)=>s+a.tally.cacheRead,0),cacheHitRate=cacheInput>0?cacheRead/cacheInput:null
  const paidMonths=(isYear?12:1)*(isInProgress||recordsBegin?Math.min(observedDays,daysBetween(start,fullEnd))/daysBetween(start,fullEnd):1),paid=monthlyPrice?paidMonths*monthlyPrice:null,payback=summary.cost!==null&&summary.cost>0&&paid&&paid>0?summary.cost/paid:null
  const cards:RecapCard[]=summary.tokens>0?['poster',...(summary.agents.length?['opener' as const]:[]),...(isYear?['yearCalendar' as const,'months' as const]:['calendar' as const]),...(peakHour!==null?['timetable' as const]:[]),...(payback!==null?['payback' as const]:[]),'scorecard']:[]
  const savings=summary.records.flatMap(r=>r.cacheSavings==null?[]:[r.cacheSavings]);const cacheSavings=savings.length?savings.reduce((a,b)=>a+b,0):null
  return {key,isYear,isPartial,cacheSavings,start,end,fullEnd,summary,isInProgress,recordsBegin,observedDays,elapsedDays,change,months,hours,untimedTokens,peakHour,persona,latestMinute,lateNights:stretches.length?stretches.filter(s=>s.crossesMidnight).length:null,currentStreak:current,longestStreak:longest,busiest,cacheHitRate,cacheUnmeasuredTokens:summary.agents.filter(a=>!cacheAgents.includes(a)).reduce((s,a)=>s+a.tokens,0),paidMonths,paid,payback,cards}
}
export type Recap=ReturnType<typeof buildRecap>
export const recapFilename=(recap:Recap,card:RecapCard)=>`pulse-recap-${recap.key}-${String(recap.cards.indexOf(card)+1).padStart(2,'0')}-${card}.png`
