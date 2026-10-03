import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import type { UsageWindow } from '../types'
import type { SpendSnapshot } from '../token-spend/types'
import { amounts, dayKey, totalTokens } from '../token-spend/summary'

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
  const request=invoke<CardHistory>('spend_card_history',{provider}).then(value=>{
    if(!value || !['disabled','unavailable','ready'].includes(value.status))throw new Error('Invalid history reply')
    if(revision===generation)retained.set(provider,{at:performance.now(),value})
    return value
  }).catch(()=>({status:'failed',snapshot:null} as CardHistory)).finally(()=>{if(pending.get(provider)===request)pending.delete(provider)})
  pending.set(provider,request);return request
}
export function useCardHistory(provider:string,detailed:boolean,tokenEnabled:boolean):CardHistory {
  const [owned,setOwned]=useState<{provider:string;value:CardHistory}>({provider,value:{status:'disabled',snapshot:null}})
  useEffect(()=>{
    let live=true
    const setValue=(value:CardHistory)=>setOwned({provider,value})
    if(!tokenEnabled){clearCardHistory();setValue({status:'disabled',snapshot:null});return}
    if(!detailed){setValue({status:'disabled',snapshot:null});return}
    setValue({status:'reading',snapshot:null})
    void read(provider).then(value=>{if(live)setValue(value)})
    return()=>{live=false}
  },[provider,detailed,tokenEnabled])
  // Never show the preceding account's totals during an effect handoff.
  if(!detailed || !tokenEnabled)return {status:'disabled',snapshot:null}
  return owned.provider===provider?owned.value:{status:'reading',snapshot:null}
}
export function cardActivity(snapshot:SpendSnapshot,now=new Date()) {
  const days=Array.from({length:31},(_,i)=>dayKey(new Date(now.getFullYear(),now.getMonth(),now.getDate()-30+i)))
  const records=snapshot.records.filter(r=>r.day>=days[0] && r.day<=days[30])
  const figures=[amounts(records.filter(r=>r.day===days[30])),amounts(records.filter(r=>r.day>=days[24])),amounts(records)]
  const chart=days.map(day=>({day,tokens:records.filter(r=>r.day===day).reduce((sum,r)=>sum+totalTokens(r),0)}))
  const models=new Map<string,number>();records.forEach(r=>models.set(r.modelName??r.model,(models.get(r.modelName??r.model)??0)+totalTokens(r)))
  const top=[...models].sort((a,b)=>b[1]-a[1])[0]
  const month=figures[2],input=month.tally.input+month.tally.cacheRead+month.tally.cacheWrite
  return {figures,chart,top:top?{name:top[0],share:top[1]/month.tokens}:null,
    cacheHitRate:!snapshot.notes.length && !month.unclassifiedTokens && input>0?month.tally.cacheRead/input:null,
    partial:snapshot.notes.length>0,exported:snapshot.sources.some(s=>s.origin==='export')}
}
/** Pulse BudgetEstimator: >=2% used, >=$0.20 measured, history before the
 * window, account-wide only. Captures with only a date cannot price a window. */
export function budgetEstimate(window:UsageWindow,snapshot:SpendSnapshot,now=Date.now()):{full:number;spent:number}|null {
  const percent=window.percentUsed,seconds=window.windowSeconds,reset=Date.parse(window.resetsAt??'')
  if(window.scope || percent==null || percent<2 || !Number.isFinite(percent) || !seconds || seconds<=0 || !Number.isFinite(reset) || snapshot.notes.length)return null
  const opened=reset/1000-seconds
  if(opened>=now/1000 || reset<=now || !snapshot.records.length || snapshot.records.some(r=>r.aggregateTiming || r.sourceTimestamp==null || r.unclassifiedTokens))return null
  const first=snapshot.records.reduce((first,r)=>Math.min(first,Math.floor(r.sourceTimestamp!/900)*900),Infinity)
  if(first>opened)return null
  const period=snapshot.records.filter(r=>r.sourceTimestamp!>=opened && r.sourceTimestamp!*1000<=now)
  if(period.some(r=>r.cost==null))return null
  const spent=period.reduce((sum,r)=>sum+(r.cost??0),0)
  return spent>=.2?{full:spent/(percent/100),spent}:null
}
