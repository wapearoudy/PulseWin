import type { UsageWindow } from './types'

/** Pulse BurnRate: the average since this reported window opened. No history. */
export function burnRate(window: UsageWindow, now=Date.now()): { exhaustsBeforeReset:boolean; timeToExhaustion:number|null } | null {
  if(window.percentUsed==null||!Number.isFinite(window.percentUsed)||!window.resetsAt||!window.windowSeconds||!Number.isFinite(window.windowSeconds)||window.windowSeconds<=0)return null
  const untilReset=(Date.parse(window.resetsAt)-now)/1000
  if(!Number.isFinite(untilReset)||untilReset<=0)return null
  const elapsed=Math.max(0,Math.min(1,1-untilReset/window.windowSeconds))
  if(elapsed<.03||elapsed>=1)return null
  const used=Math.max(0,Math.min(1,window.percentUsed/100)),elapsedSeconds=untilReset/(1-elapsed)*elapsed
  if(elapsedSeconds<=0)return null
  const rate=used/elapsedSeconds
  if(used>=1||rate<=0)return {exhaustsBeforeReset:used>=1,timeToExhaustion:used>=1?0:null}
  const untilEmpty=(1-used)/rate,first=untilEmpty<untilReset
  return {exhaustsBeforeReset:first,timeToExhaustion:first&&untilEmpty<=7200?untilEmpty:null}
}
export function approximate(seconds:number):string {
  const minutes=Math.round(seconds/60)
  if(minutes<15)return '不足 15 分钟'
  if(minutes<68){const quarters=Math.max(1,Math.round(minutes/15));return quarters===4?'约 1 小时':`约 ${quarters*15} 分钟`}
  return `约 ${Math.round(minutes/60*2)/2} 小时`
}
export function forecastText(window:UsageWindow,now=Date.now()):string|null {
  const reading=burnRate(window,now);if(!reading)return null
  if(!reading.exhaustsBeforeReset)return '按当前速度，预计可用至重置'
  if(reading.timeToExhaustion===0)return '额度已用尽'
  if(reading.timeToExhaustion!=null)return `按当前速度，预计 ${approximate(reading.timeToExhaustion)}后用尽`
  return '按当前速度，预计在重置前用尽'
}
