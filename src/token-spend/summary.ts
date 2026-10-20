import * as money from '../money'
import type { SpendAmounts, SpendDay, SpendGroup, SpendRecord, SpendSpan, SpendSummary } from './types'

export function dayKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}
/** A span that keeps the last `count` days, today included. */
export const rollingSpans: Partial<Record<SpendSpan, number>> = { today: 1, week: 7, month: 30, quarter: 90 }
/** The most days one series may hold: a decade. Only 「全部」 can reach it. */
const MAX_DAYS = 3700
function startOfDay(date: Date): Date { return new Date(date.getFullYear(), date.getMonth(), date.getDate()) }
function addDays(date: Date, days: number): Date { return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days) }
function parseDay(key: string): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(key)
  if (!match) return null
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]))
  // `new Date(2026, 1, 31)` rolls into March; a stored 31 February is not a day.
  return Number.isFinite(date.getTime()) && date.getMonth() === Number(match[2]) - 1 && date.getDate() === Number(match[3]) ? date : null
}
/** The reader's own range, as two `YYYY-MM-DD` days, in order. */
export type CustomDays = readonly [string, string]
/** `from` is the first day counted and `until` the day AFTER the last one, so a
 *  calendar month ends where the next one starts. `null` means open-ended — a
 *  rolling span has no end, and 「全部」 has neither edge. */
export interface SpanBounds { from: string | null; until: string | null }
/**
 * Where a span starts and stops, as the original's `SpendSpan.bounds` reads it:
 * the rolling spans count back from today, the calendar spans follow the
 * calendar (a bill's 「上月」 is a month, never the last 30 days), 「全部」 is
 * open at both ends, and 「自定义」 is the reader's own first and last day.
 */
export function spanBounds(span: SpendSpan, now = new Date(), custom?: CustomDays | null): SpanBounds {
  const today = startOfDay(now)
  const rolling = rollingSpans[span]
  if (rolling !== undefined) return { from: dayKey(addDays(today, -(rolling - 1))), until: null }
  switch (span) {
    case 'thisMonth': return { from: dayKey(new Date(today.getFullYear(), today.getMonth(), 1)), until: null }
    case 'lastMonth':
      return { from: dayKey(new Date(today.getFullYear(), today.getMonth() - 1, 1)), until: dayKey(new Date(today.getFullYear(), today.getMonth(), 1)) }
    case 'thisYear': return { from: dayKey(new Date(today.getFullYear(), 0, 1)), until: null }
    case 'all': return { from: null, until: null }
  }
  const last = parseDay(custom?.[1] ?? '') ?? today
  const first = parseDay(custom?.[0] ?? '') ?? addDays(today, -6)
  return { from: dayKey(first > last ? last : first), until: dayKey(addDays(last, 1)) }
}
/** True for a span of a single day: 「今天」 always, 「自定义」 when both ends are
 *  the same day. This is what decides between an hour profile and day bars. */
/** The stored two days, if they really are two days in order. */
export function customDaysOf(value: readonly string[] | null | undefined): CustomDays | null {
  if (!value || value.length !== 2) return null
  const first = parseDay(value[0]), last = parseDay(value[1])
  return first && last && first <= last ? [dayKey(first), dayKey(last)] : null
}
export function spanIsOneDay(span: SpendSpan, custom?: CustomDays | null): boolean {
  if (span === 'today') return true
  if (span !== 'custom') return false
  const first = parseDay(custom?.[0] ?? ''), last = parseDay(custom?.[1] ?? '')
  return first !== null && last !== null && first.getTime() === last.getTime()
}
/**
 * Every day the span covers, oldest first, so quiet days stay in the series and
 * the chart keeps its gaps. A rolling span ends at today, but a record dated
 * ahead of today extends it rather than being counted in a day the series omits.
 */
export function spanDayList(bounds: SpanBounds, now = new Date(), records: readonly SpendRecord[] = []): string[] {
  const today = startOfDay(now)
  const newest = records.reduce((last, record) => record.day > last ? record.day : last, '')
  const newestDay = parseDay(newest)
  const from = bounds.from === null ? null : parseDay(bounds.from)
  const first = from ?? newestDay ?? today
  const last = bounds.until !== null ? addDays(parseDay(bounds.until) ?? today, -1) : newestDay && newestDay > today ? newestDay : today
  const end = last < first ? first : last
  // A decade of history keeps its most recent days, not its oldest.
  const start = addDays(end, -(MAX_DAYS - 1))
  const days: string[] = []
  for (let cursor = first < start ? start : first; cursor <= end; cursor = addDays(cursor, 1)) days.push(dayKey(cursor))
  return days
}
export function spanDays(span: SpendSpan, now = new Date(), custom?: CustomDays | null, records: readonly SpendRecord[] = []): string[] {
  return spanDayList(spanBounds(span, now, custom), now, records)
}
export function totalTokens(record: SpendRecord): number {
  return record.tally.input + record.tally.output + record.tally.cacheWrite + record.tally.cacheRead + (record.unclassifiedTokens ?? 0)
}
function exactCounter(value:number,exact?:string):bigint|null{
  if(exact!==undefined&&/^\d+$/.test(exact))return BigInt(exact)
  return Number.isSafeInteger(value)&&value>=0?BigInt(value):null
}
export function exactRecordTokens(record:SpendRecord):bigint|null{
  const counts=[exactCounter(record.tally.input,record.exactTally?.input),exactCounter(record.tally.output,record.exactTally?.output),exactCounter(record.tally.cacheWrite,record.exactTally?.cacheWrite),exactCounter(record.tally.cacheRead,record.exactTally?.cacheRead),exactCounter(record.unclassifiedTokens??0,record.exactUnclassifiedTokens)]
  return counts.some(n=>n===null)?null:counts.reduce<bigint>((sum,n)=>sum+n!,BigInt(0))
}
export const exactTokenText=(value:number,exact?:string)=>exact!==undefined?BigInt(exact).toLocaleString('zh-CN'):value.toLocaleString('zh-CN')
export function amounts(records: SpendRecord[]): SpendAmounts {
  const result: SpendAmounts = { tokens: 0, tally: { input: 0, output: 0, cacheWrite: 0, cacheRead: 0 }, cost: records.length ? null : 0, costBreakdown: records.length ? null : [0, 0, 0, 0], unpricedTokens: 0, unclassifiedTokens: 0 }
  let exact:bigint|null=BigInt(0),unclassified:bigint|null=BigInt(0)
  for (const record of records) {
    const tokens = totalTokens(record); result.tokens += tokens
    result.unclassifiedTokens += record.unclassifiedTokens ?? 0
    result.tally.input += record.tally.input; result.tally.output += record.tally.output
    result.tally.cacheWrite += record.tally.cacheWrite; result.tally.cacheRead += record.tally.cacheRead
    const count=exactRecordTokens(record),bare=exactCounter(record.unclassifiedTokens??0,record.exactUnclassifiedTokens)
    exact=exact===null||count===null?null:exact+count;unclassified=unclassified===null||bare===null?null:unclassified+bare
    result.repliesWithoutCacheFields=(result.repliesWithoutCacheFields??0)+(record.tally.repliesWithoutCacheFields??0)
    if (record.cost === null || record.costBreakdown === null) result.unpricedTokens += tokens
    else {
      result.unpricedTokens += record.unclassifiedTokens ?? 0
      result.cost = (result.cost ?? 0) + record.cost
      const breakdown = result.costBreakdown ?? [0, 0, 0, 0]
      result.costBreakdown = breakdown.map((n, i) => n + record.costBreakdown![i]) as [number, number, number, number]
    }
  }
  if(exact!==null)result.exactTokens=exact.toString()
  if(unclassified!==null)result.exactUnclassifiedTokens=unclassified.toString()
  return result
}
function groups(records: SpendRecord[], key: (record: SpendRecord) => string | null, name: (record: SpendRecord) => string): SpendGroup[] {
  const buckets = new Map<string, { name: string; records: SpendRecord[] }>()
  for (const record of records) {
    const id = key(record); if (id === null) continue
    const bucket = buckets.get(id) ?? { name: name(record), records: [] }; bucket.records.push(record); buckets.set(id, bucket)
  }
  return [...buckets].map(([id, bucket]) => ({ id, ...bucket, ...amounts(bucket.records) })).sort((a, b) => compareTokens(b,a) || a.name.localeCompare(b.name))
}
const agentNames: Record<string, string> = { claude: 'Claude Code', codex: 'Codex', qwen: 'Qwen Code', gemini: 'Gemini CLI', cursor: 'Cursor', antigravity: 'Antigravity (export)', hindsight: 'Hindsight', mcode: 'MiniMax Code', opencode: 'OpenCode', kilo: 'Kilo CLI', micode: 'MiMo Code', dsh: 'DeepSeek Harness',pi:'Pi',omp:'Oh My Pi',kimchi:'Kimchi',amp:'Amp',droid:'Droid',openclaw:'OpenClaw',goose:'Goose',hermes:'Hermes',zed:'Zed',crush:'Crush',kiro:'Kiro',kimi:'Kimi CLI',
  roocode:'Roo Code',kilocode:'Kilo Code',cline:'Cline',cherrystudio:'Cherry Studio',commandcode:'Command Code',opencodereview:'OpenCodeReview',zcode:'ZCode',unsloth:'Unsloth',mux:'Mux',codebuff:'Codebuff',jcode:'JCode',augment:'Augment',gjc:'Gajae Code',junie:'Junie',fx:'FX',lmstudio:'LM Studio',reasonix:'Reasonix',
  // Ported from Pulse 1.9's remaining Groups A–F. Three Antigravity entries are
  // separate products with separate stores and are never pooled, so the
  // qualifiers are load-bearing. `devin` stays "Devin" rather than "Devin CLI":
  // Desktop embeds the same core, and the database cannot say which client
  // drove it — calling the rows CLI usage would name a program the reader may
  // never have installed.
  grok:'Grok Build','devin-cli':'Devin',senpi:'OmO Native','prime-agent':'Prime Agent',codebuddy:'CodeBuddy',workbuddy:'WorkBuddy','antigravity-cli':'Antigravity CLI','antigravity-ide':'Antigravity IDE','devin-desktop':'Devin Desktop',freebuff:'Freebuff',trae:'Trae',warp:'Warp',copilot:'GitHub Copilot' }
export const agentName = (id: string) => agentNames[id] ?? id
export const modelKey = (record: SpendRecord) => record.modelName ?? record.model
function projectKey(record: SpendRecord): string | null {
  if (!record.project) return null
  const normalized = record.project.replace(/\\/g, '/').replace(/\/+$/, '')
  // Explicit absolute Windows cwd identifies one project across agents. A bare
  // label or a relative path remains scoped to the source that reported it.
  return /^[a-z]:\//i.test(normalized) || normalized.startsWith('//') ? normalized.toLowerCase() : `${record.agent}:${normalized}`
}
export function summarize(all: SpendRecord[], span: SpendSpan, now = new Date(), agent?: string | null, model?: string | null, custom?: CustomDays | null): SpendSummary {
  return summarizeDays(all,spanDays(span,now,custom,all),agent,model)
}
export function summarizeBetween(all:SpendRecord[],start:Date,end:Date,agent?:string|null,model?:string|null):SpendSummary{
  const days:string[]=[]
  for(let d=new Date(start.getFullYear(),start.getMonth(),start.getDate());d<end&&days.length<370;d=new Date(d.getFullYear(),d.getMonth(),d.getDate()+1))days.push(dayKey(d))
  return summarizeDays(all,days,agent,model)
}
function summarizeDays(all:SpendRecord[],days:string[],agent?:string|null,model?:string|null):SpendSummary{
  const allowed = new Set(days)
  const records = all.filter(record => allowed.has(record.day) && (!agent || record.agent === agent) && (!model || modelKey(record) === model))
  const dayRecords = new Map<string, SpendRecord[]>(); const hours = Array<number>(24).fill(0)
  const hasAggregateTiming = records.some(record => record.aggregateTiming)
  for (const record of records) { const bucket = dayRecords.get(record.day) ?? []; bucket.push(record); dayRecords.set(record.day, bucket); if (!record.aggregateTiming) hours[record.hour] += totalTokens(record) }
  const activeDays = dayRecords.size, maxHour = Math.max(...hours)
  const daySeries = days.map(day => ({ day, ...amounts(dayRecords.get(day) ?? []) }))
  const hourTotal = hours.reduce((sum, value) => sum + value, 0)
  return { ...amounts(records), records, days: daySeries, hours,
    models: groups(records, modelKey, modelKey), agents: groups(records, r => r.agent, r => agentName(r.agent)),
    sessions: groups(records, r => r.session ? `${r.agent}:${r.session}` : null, r => `${agentName(r.agent)} · ${r.session}`),
    projects: groups(records, projectKey, r => r.project ?? ''),
    activeDays, hasAggregateTiming, peakHour: !hasAggregateTiming && maxHour > 0 ? hours.indexOf(maxHour) : null,
    activeHours: hours.filter(value => value > 0).length, busiestDay: busiest(daySeries), hoursComplete: hourTotal === totalTokensSum(records) }
}
/** The heaviest day, and only a day that had something on it: every quiet day
 *  ties at zero, and naming one of them would invent an event. */
function busiest(days: SpendDay[]): SpendDay | null {
  let best: SpendDay | null = null
  for (const day of days) if (day.tokens > 0 && (!best || day.tokens > best.tokens)) best = day
  return best
}
function totalTokensSum(records: readonly SpendRecord[]): number { return records.reduce((sum, record) => sum + totalTokens(record), 0) }
export function shortTokens(value: number): string {
  if (value >= 1e8) return `${(value / 1e8).toFixed(value >= 1e9 ? 1 : 2).replace(/\.0+$/, '')}亿`
  if (value >= 1e4) return `${(value / 1e4).toFixed(value >= 1e5 ? 1 : 2).replace(/\.0+$/, '')}万`
  return value.toLocaleString('zh-CN')
}
/**
 * An amount worked out from US-dollar API prices, written in the currency
 * chosen in Settings — the estimate, Token spend, the cards' history and
 * the recap are all this one figure. `wholeFrom` is where the written figure
 * stops carrying cents: 1000 in Settings and the cards, 10 in the recap,
 * whose figures are small.
 *
 * **Converted, unlike a balance a service reports** (`money.reported`):
 * that one is the service's own figure and stays in the currency it gave.
 */
export function formatCost(value: number | null, wholeFrom = 1000): string {
  if (value === null) return '—'
  return money.estimate(value, wholeFrom)
}
/** Unknown amounts stay last in either direction; date resolves numeric ties. */
function compareTokens(a:{tokens:number;exactTokens?:string},b:{tokens:number;exactTokens?:string}):number{
  if(a.exactTokens!==undefined&&b.exactTokens!==undefined){const left=BigInt(a.exactTokens),right=BigInt(b.exactTokens);return left<right?-1:left>right?1:0}return a.tokens-b.tokens
}
export function sortDays<T extends { day: string; tokens: number; cost: number | null;exactTokens?:string }>(days: T[], key: 'day' | 'tokens' | 'cost', descending: boolean): T[] {
  return [...days].sort((a, b) => {
    if (key === 'cost') { if (a.cost === null && b.cost !== null) return 1; if (a.cost !== null && b.cost === null) return -1 }
    const left = a[key], right = b[key]
    const delta = key==='tokens'?compareTokens(a,b):typeof left === 'string' && typeof right === 'string' ? left.localeCompare(right) : Number(left) - Number(right)
    return delta ? (descending ? -delta : delta) : b.day.localeCompare(a.day)
  })
}
