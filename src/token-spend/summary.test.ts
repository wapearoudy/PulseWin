import { describe, expect, it } from 'vitest'
import { agentName, amounts, formatCost, shortTokens, sortDays, spanDays, summarize } from './summary'
import type { SpendRecord } from './types'
const record = (patch: Partial<SpendRecord> = {}): SpendRecord => ({ agent: 'claude', model: 'model-one', modelName: null, day: '2026-10-03', hour: 9, session: 's1', project: 'E:\\work', tally: { input: 100, output: 20, cacheWrite: 10, cacheRead: 70 }, cost: 0.02, costBreakdown: [0.01, 0.004, 0.002, 0.004], ...patch })
describe('Token Spend calendar and measured totals', () => {
  it('defaults to seven calendar dates and pads quiet days', () => {
    const now = new Date(2026, 9, 3, 12); expect(spanDays('week', now)).toHaveLength(7)
    const summary = summarize([record()], 'week', now)
    expect(summary.days).toHaveLength(7); expect(summary.days[0].tokens).toBe(0); expect(summary.days[0].cost).toBe(0)
    expect(summary.days[6].tokens).toBe(200); expect(summary.tokens).toBe(200)
  })
  it('windows resumed sessions by the records own day', () => {
    const summary = summarize([record(), record({ day: '2026-10-02' })], 'today', new Date(2026, 9, 3))
    expect(summary.tokens).toBe(200); expect(summary.sessions[0].tokens).toBe(200)
  })
  it('keeps unpriced tokens and distinguishes unknown and published zero amounts', () => {
    const partial = amounts([record(), record({ cost: null, costBreakdown: null })])
    expect(partial.tokens).toBe(400); expect(partial.cost).toBe(0.02); expect(partial.unpricedTokens).toBe(200)
    expect(amounts([record({ cost: null, costBreakdown: null })]).cost).toBe(null)
    expect(amounts([record({ cost: 0, costBreakdown: [0, 0, 0, 0] })]).cost).toBe(0)
  })
  it('prices raw IDs before combining their shared published display name', () => {
    const summary = summarize([record({ model: 'raw-a', modelName: 'Model', cost: 0.1 }), record({ model: 'raw-b', modelName: 'Model', cost: 0.8 })], 'today', new Date(2026, 9, 3))
    expect(summary.models).toHaveLength(1); expect(summary.models[0].tokens).toBe(400); expect(summary.models[0].cost).toBe(0.9)
  })
  it('isolates model details and an agent model scope without rereading', () => {
    const all = [record(), record({ agent: 'codex' }), record({ model: 'model-two' })]
    expect(summarize(all, 'today', new Date(2026, 9, 3), null, 'model-one').tokens).toBe(400)
    expect(summarize(all, 'today', new Date(2026, 9, 3), 'claude', 'model-one').tokens).toBe(200)
  })
  it('reconciles hours, daily rows, models and agent totals', () => {
    const s = summarize([record(), record({ agent: 'codex', hour: 14 })], 'today', new Date(2026, 9, 3))
    expect(s.hours.reduce((a,b) => a+b, 0)).toBe(s.tokens); expect(s.agents.reduce((a,b) => a+b.tokens,0)).toBe(s.tokens)
    expect(s.models.reduce((a,b) => a+b.tokens,0)).toBe(s.tokens); expect(s.days.reduce((a,b) => a+b.tokens,0)).toBe(s.tokens)
  })
  it('keeps aggregate export totals on their day without fabricating a peak hour', () => {
    const records = [record(), record({ agent: 'cursor', hour: 0, aggregateTiming: true, session: '' })]
    const summary = summarize(records, 'today', new Date(2026, 9, 3))
    expect(summary.tokens).toBe(400); expect(summary.days[0].tokens).toBe(400)
    expect(summary.hasAggregateTiming).toBe(true); expect(summary.peakHour).toBe(null)
    expect(summary.hours[0]).toBe(0); expect(summary.hours[9]).toBe(200)
    expect(summary.sessions).toHaveLength(1)
    const native = summarize(records, 'today', new Date(2026, 9, 3), 'claude')
    expect(native.hasAggregateTiming).toBe(false); expect(native.peakHour).toBe(9)
  })
  it('does not fabricate Cursor sessions where the export states no conversation', () => {
    const summary = summarize([record({ agent: 'cursor', session: '', project: null })], 'today', new Date(2026, 9, 3))
    expect(summary.tokens).toBe(200); expect(summary.sessions).toHaveLength(0); expect(summary.projects).toHaveLength(0)
  })
  it('names the actual native and captured readers without replacing unknown ids', () => {
    expect(agentName('gemini')).toBe('Gemini CLI'); expect(agentName('antigravity')).toBe('Antigravity')
    expect(agentName('opencode')).toBe('OpenCode'); expect(agentName('kilo')).toBe('Kilo CLI'); expect(agentName('micode')).toBe('MiMo Code')
    expect(agentName('hindsight')).toBe('Hindsight'); expect(agentName('mcode')).toBe('MCode')
    expect(agentName('not-implemented')).toBe('not-implemented')
  })
  it('does not invent a project for sessions without cwd', () => {
    const s = summarize([record({ project: null })], 'today', new Date(2026, 9, 3)); expect(s.sessions).toHaveLength(1); expect(s.projects).toHaveLength(0)
  })
  it('combines explicit absolute project paths across agents without merging bare labels', () => {
    const s = summarize([record(), record({ agent:'codex', project:'e:/work/' }), record({ project:'folder' }), record({ agent:'codex', project:'folder' })], 'today', new Date(2026,9,3))
    expect(s.projects).toHaveLength(3); expect(s.projects[0].tokens).toBe(400)
  })
  it('keeps missing costs last ascending and descending with exact fractional order', () => {
    const rows = [{ day: 'a', tokens: 1, cost: null }, { day: 'b', tokens: 2, cost: 0.001 }, { day: 'c', tokens: 3, cost: 0.002 }]
    expect(sortDays(rows, 'cost', false).map(r => r.day)).toEqual(['b', 'c', 'a'])
    expect(sortDays(rows, 'cost', true).map(r => r.day)).toEqual(['c', 'b', 'a'])
  })
  it('formats small positive dollars without misrepresenting zero', () => {
    expect(formatCost(null)).toBe('—'); expect(formatCost(0)).toBe('US$0.00'); expect(formatCost(0.00001)).toBe('< US$0.01')
    expect(shortTokens(12000)).toBe('1.20万'); expect(shortTokens(123)).toBe('123')
  })
  it('keeps bare measured totals unclassified and priced subsets marked partial', () => {
    const bare = record({ tally:{input:0,output:0,cacheRead:0,cacheWrite:0},unclassifiedTokens:500,cost:null,costBreakdown:null })
    const a = amounts([bare]); expect(a.tokens).toBe(500); expect(a.tally.input).toBe(0); expect(a.unclassifiedTokens).toBe(500); expect(a.cost).toBe(null)
    const b = amounts([record({unclassifiedTokens:100})]); expect(b.tokens).toBe(300); expect(b.unpricedTokens).toBe(100); expect(b.cost).toBe(0.02)
  })
})
