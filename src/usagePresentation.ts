import type { ProviderUsage, UsageWindow } from './types'

export function headlineWindow(provider: ProviderUsage, pin?: string): UsageWindow | undefined {
  return (pin ? provider.windows.find(w => w.id === pin || w.label === pin) : undefined) ?? [...provider.windows].sort((a,b) => (b.percentUsed ?? -1) - (a.percentUsed ?? -1))[0]
}

/** The second limit belongs to the headline's actual model pool where possible. */
export function secondWindow(provider: ProviderUsage, pin?: string): UsageWindow | undefined {
  const headline = headlineWindow(provider,pin)
  const rest = provider.windows.filter(window => window !== headline)
  const sameGroup = rest.filter(window => (window.scope ?? null) === (headline?.scope ?? null))
  return headlineWindow({...provider, windows:sameGroup.length ? sameGroup : rest})
}

export function clockFraction(window: UsageWindow | undefined, now: number, remaining: boolean): number | null {
  if (!window?.resetsAt || !window.windowSeconds || !Number.isFinite(window.windowSeconds) || window.windowSeconds <= 0) return null
  const reset = Date.parse(window.resetsAt)
  if (!Number.isFinite(reset)) return null
  const left = Math.min(1, Math.max(0, (reset - now) / (window.windowSeconds * 1000)))
  return remaining ? left : 1 - left
}

/** Pulse UsageWindow.figure: a positive reading never becomes zero and
 * an incomplete window never becomes 100 through rounding. */
export function percentFigure(fraction:number, remaining=false):number {
  const value=remaining?1-fraction:fraction
  if(!Number.isFinite(value))return 0
  if(value<=0)return 0
  if(value>=1)return 100
  return Math.min(99,Math.max(1,Math.round(value*100)))
}
