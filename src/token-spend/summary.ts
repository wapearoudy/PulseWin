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
export function summarize(all: SpendRecord[], span: SpendSpan, now = new Date(), agent?: string | null, model?: string | null): SpendSummary {
  return summarizeDays(all,spanDays(span,now),agent,model)
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
