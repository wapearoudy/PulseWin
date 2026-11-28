import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { UsageWindow } from '../types'
import type { SpendAmounts, SpendRecord, SpendSnapshot } from '../token-spend/types'
import { amounts, dayKey, formatCost, totalTokens } from '../token-spend/summary'
import { format } from '../money'

export interface CardHistory { status:'disabled'|'unavailable'|'ready'|'reading'|'failed'; snapshot:SpendSnapshot|null }
const retained = new Map<string,{at:number;value:CardHistory}>()
const pending = new Map<string,Promise<CardHistory>>()
let generation=0
export function clearCardHistory(){generation++;retained.clear();pending.clear()}
function read(provider:string):Promise<CardHistory> {
  const cached=retained.get(provider)
  if(cached && performance.now()-cached.at<5*60*1000)return Promise.resolve(cached.value)
  const active=pending.get(provider);if(active)return active
  const revision=generation
  const command=provider==='deepseek'?'deepseek_card_history':provider==='opencode-go'?'opencode_card_history':'spend_card_history'
  const request=invoke<CardHistory>(command,command==='spend_card_history'?{provider}:{}).then(value=>{
    if(!value || !['disabled','unavailable','ready'].includes(value.status))throw new Error('Invalid history reply')
    if(revision===generation)retained.set(provider,{at:performance.now(),value})
    return value
  }).catch(()=>({status:'failed',snapshot:null} as CardHistory)).finally(()=>{if(pending.get(provider)===request)pending.delete(provider)})
  pending.set(provider,request);return request
}
export function useCardHistory(provider:string,detailed:boolean,tokenEnabled:boolean):CardHistory {
  const consoleLedger=provider==='deepseek'||provider==='opencode-go'
  const [owned,setOwned]=useState<{provider:string;value:CardHistory}>({provider,value:{status:'disabled',snapshot:null}})
  useEffect(()=>{
    let live=true
    const setValue=(value:CardHistory)=>setOwned({provider,value})
    if(!tokenEnabled&&!consoleLedger){clearCardHistory();setValue({status:'disabled',snapshot:null});return}
    if(!detailed){setValue({status:'disabled',snapshot:null});return}
    setValue({status:'reading',snapshot:null})
    void read(provider).then(value=>{if(live)setValue(value)})
    return()=>{live=false}
  },[provider,detailed,tokenEnabled,consoleLedger])
  // Never show the preceding account's totals during an effect handoff.
  if(!detailed || (!tokenEnabled&&!consoleLedger))return {status:'disabled',snapshot:null}
  return owned.provider===provider?owned.value:{status:'reading',snapshot:null}
}
interface ConsoleCharge {
  currency: string | null
  amount: number | null
  partial: boolean
}

export type CardFigure = SpendAmounts & { charge?: ConsoleCharge }

export function isConsoleHistory(snapshot: SpendSnapshot | null): boolean {
  return snapshot?.pricingStatus === 'console'
}

export function cardFigure(snapshot: SpendSnapshot, records: SpendRecord[]): CardFigure {
  const figure = amounts(records)
  if (!isConsoleHistory(snapshot)) return figure

  const billing = snapshot.consoleBilling
  // Intl accepts ISO-style codes. A missing or malformed currency cannot be
  // labeled as USD or converted using the model-price currency.
  const currency = billing?.currency?.trim().toUpperCase()
  const missingDays = new Set(billing?.daysWithMissingCharges)
  const charge: ConsoleCharge = { currency: currency && /^[A-Z]{3}$/.test(currency) ? currency : null, amount: null, partial: snapshot.notes.length > 0 }
  for (const record of records) {
    const cost = record.cost !== null && Number.isFinite(record.cost) ? record.cost : null
    if (cost === null || missingDays.has(record.day)) charge.partial = true
    if (charge.currency && cost !== null) charge.amount = (charge.amount ?? 0) + cost
  }
  return { ...figure, cost: null, costBreakdown: null, unpricedTokens: 0, charge }
}

export function cardFigureCost(figure: CardFigure): string {
  const { charge } = figure
  if (!charge) return formatCost(figure.cost)
  if (charge.amount === null || !charge.currency) return '—'
  return `${format(charge.amount, charge.currency, Infinity)}${charge.partial ? ' *' : ''}`
}

export function consoleBillingNote(figure: CardFigure): string {
  const { charge } = figure
  if (!charge?.currency) return '控制台未报告扣费币种。'
  return `控制台实际扣费（${charge.currency}）。${charge.partial ? '账单可能不完整。' : ''}`
}

export function cardActivity(snapshot:SpendSnapshot,now=new Date()) {
  const days=Array.from({length:31},(_,i)=>dayKey(new Date(now.getFullYear(),now.getMonth(),now.getDate()-30+i)))
  const records=snapshot.records.filter(r=>r.day>=days[0] && r.day<=days[30])
  const figures=[cardFigure(snapshot, records.filter(r=>r.day===days[30])), cardFigure(snapshot, records.filter(r=>r.day>=days[24])), cardFigure(snapshot, records)]
  const chart=days.map(day=>({day,tokens:records.filter(r=>r.day===day).reduce((sum,r)=>sum+totalTokens(r),0)}))
  const models = new Map<string, number>()
  for (const record of records) {
    const tokens = totalTokens(record)
    if (tokens <= 0) continue
    const model = record.modelName ?? record.model
    models.set(model, (models.get(model) ?? 0) + tokens)
  }
  const top=[...models].sort((a,b)=>b[1]-a[1])[0]
  const month=figures[2],input=month.tally.input+month.tally.cacheRead+month.tally.cacheWrite
  return {figures,chart,speeds:outputSpeeds(snapshot.records,now.getTime()),top:top?{name:top[0],share:top[1]/month.tokens}:null,
    cacheHitRate:!snapshot.notes.length && !month.unclassifiedTokens && !month.repliesWithoutCacheFields && input>0?month.tally.cacheRead/input:null,
    partial:snapshot.notes.length>0,exported:snapshot.sources.some(s=>s.origin==='export')}
}
export function outputSpeeds(records:SpendSnapshot['records'],now=Date.now()){
  const groups=new Map<string,{output:number;seconds:number;replies:number;first:number;turns:number}>()
  for(const r of records){const t=r.replyTiming;if(!t||r.sourceTimestamp==null||Math.floor(r.sourceTimestamp/900)*900<(now/1000-86400)||r.sourceTimestamp>now/1000)continue;const key=r.modelName??r.model,g=groups.get(key)??{output:0,seconds:0,replies:0,first:0,turns:0};if(t.seconds>0&&t.replies>0){g.output+=t.outputTokens;g.seconds+=t.seconds;g.replies+=t.replies}if(t.firstTokenSeconds>0&&t.firstTokenTurns>0){g.first+=t.firstTokenSeconds;g.turns+=t.firstTokenTurns}groups.set(key,g)}
  return [...groups].flatMap(([name,g])=>{const speed=g.replies>=5&&g.seconds>0?g.output/g.seconds:null,ttft=g.turns>=5?g.first/g.turns:null;return speed!==null||ttft!==null?[{name,speed,ttft,output:speed!==null?g.output:0}]:[]}).sort((a,b)=>b.output-a.output||a.name.localeCompare(b.name))
}
/** Pulse BudgetEstimator: >=5% used, >=$0.20 measured, history before the
 * window, account-wide only. Captures with only a date cannot price a window. */
export function budgetEstimate(window:UsageWindow,snapshot:SpendSnapshot,now=Date.now()):{full:number;spent:number}|null {
  const percent=window.percentUsed,seconds=window.windowSeconds,reset=Date.parse(window.resetsAt??'')
  if(window.scope || percent==null || percent<5 || !Number.isFinite(percent) || !seconds || seconds<=0 || !Number.isFinite(reset) || snapshot.notes.length)return null
  const opened=reset/1000-seconds
  if(opened>=now/1000 || reset<=now || !snapshot.records.length || snapshot.records.some(r=>r.aggregateTiming || r.sourceTimestamp==null || r.unclassifiedTokens))return null
  const first=snapshot.records.reduce((first,r)=>Math.min(first,Math.floor(r.sourceTimestamp!/900)*900),Infinity)
  if(first>opened)return null
  const period=snapshot.records.filter(r=>r.sourceTimestamp!>=opened && r.sourceTimestamp!*1000<=now)
  if(period.some(r=>r.cost==null))return null
  const spent=period.reduce((sum,r)=>sum+(r.cost??0),0)
  return spent>=.2?{full:spent/(percent/100),spent}:null
}
