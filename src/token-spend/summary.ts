import type { SpendAmounts, SpendGroup, SpendRecord, SpendSpan, SpendSummary } from './types'

export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}
export function spanDays(span: SpendSpan, now = new Date()): string[] {
  const count = span === 'today' ? 1 : span === 'month' ? 30 : 7
  const result: string[] = []
  for (let i = count - 1; i >= 0; i--) { const date = new Date(now.getFullYear(), now.getMonth(), now.getDate() - i); result.push(dayKey(date)) }
  return result
}
export function totalTokens(record: SpendRecord): number {
  return record.tally.input + record.tally.output + record.tally.cacheWrite + record.tally.cacheRead + (record.unclassifiedTokens ?? 0)
}
export function amounts(records: SpendRecord[]): SpendAmounts {
  const result: SpendAmounts = { tokens: 0, tally: { input: 0, output: 0, cacheWrite: 0, cacheRead: 0 }, cost: records.length ? null : 0, costBreakdown: records.length ? null : [0, 0, 0, 0], unpricedTokens: 0, unclassifiedTokens: 0 }
  for (const record of records) {
    const tokens = totalTokens(record); result.tokens += tokens
    result.unclassifiedTokens += record.unclassifiedTokens ?? 0
    result.tally.input += record.tally.input; result.tally.output += record.tally.output
    result.tally.cacheWrite += record.tally.cacheWrite; result.tally.cacheRead += record.tally.cacheRead
    if (record.cost === null || record.costBreakdown === null) result.unpricedTokens += tokens
    else {
      result.unpricedTokens += record.unclassifiedTokens ?? 0
      result.cost = (result.cost ?? 0) + record.cost
      const breakdown = result.costBreakdown ?? [0, 0, 0, 0]
      result.costBreakdown = breakdown.map((n, i) => n + record.costBreakdown![i]) as [number, number, number, number]
    }
  }
  return result
}
function groups(records: SpendRecord[], key: (record: SpendRecord) => string | null, name: (record: SpendRecord) => string): SpendGroup[] {
  const buckets = new Map<string, { name: string; records: SpendRecord[] }>()
  for (const record of records) {
    const id = key(record); if (id === null) continue
    const bucket = buckets.get(id) ?? { name: name(record), records: [] }; bucket.records.push(record); buckets.set(id, bucket)
  }
  return [...buckets].map(([id, bucket]) => ({ id, ...bucket, ...amounts(bucket.records) })).sort((a, b) => b.tokens - a.tokens || a.name.localeCompare(b.name))
}
const agentNames: Record<string, string> = { claude: 'Claude Code', codex: 'Codex', qwen: 'Qwen Code', gemini: 'Gemini CLI', cursor: 'Cursor', antigravity: 'Antigravity', hindsight: 'Hindsight', mcode: 'MCode', opencode: 'OpenCode', kilo: 'Kilo CLI', micode: 'MiMo Code', dsh: 'DeepSeek Harness' }
export const agentName = (id: string) => agentNames[id] ?? id
export const modelKey = (record: SpendRecord) => record.modelName ?? record.model
function projectKey(record: SpendRecord): string | null {
  if (!record.project) return null
  const normalized = record.project.replace(/\\/g, '/').replace(/\/+$/, '')
  // Explicit absolute Windows cwd identifies one project across agents. A bare
  // label or a relative path remains scoped to the source that reported it.
  return /^[a-z]:\//i.test(normalized) || normalized.startsWith('//') ? normalized.toLowerCase() : `${record.agent}:${normalized}`
}
export function summarize(all: SpendRecord[], span: SpendSpan, now = new Date(), agent?: string | null, model?: string | null): SpendSummary {
  const days = spanDays(span, now), allowed = new Set(days)
  const records = all.filter(record => allowed.has(record.day) && (!agent || record.agent === agent) && (!model || modelKey(record) === model))
  const dayRecords = new Map<string, SpendRecord[]>(); const hours = Array<number>(24).fill(0)
  const hasAggregateTiming = records.some(record => record.aggregateTiming)
  for (const record of records) { const bucket = dayRecords.get(record.day) ?? []; bucket.push(record); dayRecords.set(record.day, bucket); if (!record.aggregateTiming) hours[record.hour] += totalTokens(record) }
  const activeDays = dayRecords.size, maxHour = Math.max(...hours)
  return { ...amounts(records), records, days: days.map(day => ({ day, ...amounts(dayRecords.get(day) ?? []) })), hours,
    models: groups(records, modelKey, modelKey), agents: groups(records, r => r.agent, r => agentName(r.agent)),
    sessions: groups(records, r => r.session ? `${r.agent}:${r.session}` : null, r => `${agentName(r.agent)} · ${r.session}`),
    projects: groups(records, projectKey, r => r.project ?? ''),
    activeDays, hasAggregateTiming, peakHour: !hasAggregateTiming && maxHour > 0 ? hours.indexOf(maxHour) : null }
}
export function shortTokens(value: number): string {
  if (value >= 1e8) return `${(value / 1e8).toFixed(value >= 1e9 ? 1 : 2).replace(/\.0+$/, '')}亿`
  if (value >= 1e4) return `${(value / 1e4).toFixed(value >= 1e5 ? 1 : 2).replace(/\.0+$/, '')}万`
  return value.toLocaleString('zh-CN')
}
export function formatCost(value: number | null): string {
  if (value === null) return '—'
  if (value > 0 && value < 0.01) return '< US$0.01'
  return `US$${value.toLocaleString('zh-CN', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`
}
/** Unknown amounts stay last in either direction; date resolves numeric ties. */
export function sortDays<T extends { day: string; tokens: number; cost: number | null }>(days: T[], key: 'day' | 'tokens' | 'cost', descending: boolean): T[] {
  return [...days].sort((a, b) => {
    if (key === 'cost') { if (a.cost === null && b.cost !== null) return 1; if (a.cost !== null && b.cost === null) return -1 }
    const left = a[key], right = b[key]
    const delta = typeof left === 'string' && typeof right === 'string' ? left.localeCompare(right) : Number(left) - Number(right)
    return delta ? (descending ? -delta : delta) : b.day.localeCompare(a.day)
  })
}
