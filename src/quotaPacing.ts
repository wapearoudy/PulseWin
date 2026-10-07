import type { UsageWindow } from './types'

/** A reported cycle only; unknown durations and currency balances have no pace. */
export function quotaTimeFraction(window: UsageWindow | undefined, now=Date.now()): number | null {
  if (!window || window.kind==='balance' || window.kind==='topUp' || !window.resetsAt) return null
  const duration=window.windowSeconds
  if (duration==null || !Number.isFinite(duration) || duration<=0) return null
  const remaining=(Date.parse(window.resetsAt)-now)/1000
  if (!Number.isFinite(remaining) || remaining<=0 || remaining>duration) return null
  return 1-remaining/duration
}

export function quotaWarning(window: UsageWindow | undefined, threshold: number, now=Date.now()): boolean {
  if (window?.isExhausted) return true
  if (!window || window.kind==='balance' || window.kind==='topUp' || window.percentUsed==null || !Number.isFinite(window.percentUsed)) return false
  if (window.percentUsed>=100) return true
  const elapsed=quotaTimeFraction(window,now)
  return elapsed!==null && window.percentUsed>=threshold*100 && window.percentUsed/100>elapsed+1e-9
}
