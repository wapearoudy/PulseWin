export type SpendSpan = 'today' | 'week' | 'month'
export interface TokenTally { input: number; output: number; cacheWrite: number; cacheRead: number }
export interface SpendRecord {
  sourceTimestamp?: number | null
  agent: string; model: string; modelName: string | null; day: string; hour: number
  session: string; project: string | null; tally: TokenTally
  unclassifiedTokens?: number; deduplicationId?: string | null
  aggregateTiming?: boolean
  cost: number | null; costBreakdown: [number, number, number, number] | null
}
export interface SpendSource { id: string; name: string; status: 'counted' | 'no-data' | 'not-detected'; files: number; cachedFiles: number; records: number; origin?: 'native' | 'export'; roots?: string[] }
export interface SpendSnapshot {
  scannedAt: string; records: SpendRecord[]; sources: SpendSource[]; notes: string[]
  pricesAt: string | null; pricingStatus: 'cached' | 'fresh' | 'stale' | 'unavailable'
}
export interface SpendScan {
  id: string; status: 'running' | 'completed' | 'cancelled' | 'error'; currentSource: string | null
  sourceIndex: number; sourceCount: number; snapshot: SpendSnapshot | null; error: string | null
}
export interface SpendAmounts {
  tokens: number; tally: TokenTally; cost: number | null; costBreakdown: [number, number, number, number] | null; unpricedTokens: number; unclassifiedTokens: number
}
export interface SpendDay extends SpendAmounts { day: string }
export interface SpendGroup extends SpendAmounts { id: string; name: string; records: SpendRecord[] }
export interface SpendSummary extends SpendAmounts {
  records: SpendRecord[]; days: SpendDay[]; hours: number[]; models: SpendGroup[]; agents: SpendGroup[]
  sessions: SpendGroup[]; projects: SpendGroup[]; activeDays: number; peakHour: number | null
  hasAggregateTiming: boolean
}
