import { describe, expect, it } from 'vitest'
import { agentName, amounts, customDaysOf, formatCost, shortTokens, sortDays, spanBounds, spanDays, spanIsOneDay, summarize } from './summary'
import type { SpendRecord } from './types'
const record = (patch: Partial<SpendRecord> = {}): SpendRecord => ({ agent: 'claude', model: 'model-one', modelName: null, day: '2026-10-03', hour: 9, session: 's1', project: 'E:\\work', tally: { input: 100, output: 20, cacheWrite: 10, cacheRead: 70 }, cost: 0.02, costBreakdown: [0.01, 0.004, 0.002, 0.004], ...patch })
describe('Token Spend calendar and measured totals', () => {
  it('sorts adjacent counters beyond Number precision by their native decimal strings',()=>{
    const a=record({day:'2026-10-02',tally:{input:9007199254740992,output:0,cacheWrite:0,cacheRead:0},exactTally:{input:'9007199254740993',output:'0',cacheWrite:'0',cacheRead:'0'},cost:null,costBreakdown:null})
    const b=record({...a,day:'2026-10-03',exactTally:{input:'9007199254740992',output:'0',cacheWrite:'0',cacheRead:'0'}})
    const s=summarize([a,b],'week',new Date(2026,9,3));expect(s.exactTokens).toBe('18014398509481985');expect(sortDays(s.days,'tokens',true).slice(0,2).map(d=>d.day)).toEqual(['2026-10-02','2026-10-03']);expect(s.models[0].exactTokens).toBe(s.exactTokens)
  })
  it('keeps an unknown cache distinct from an explicit zero cache',()=>{
    expect(amounts([record({tally:{input:100,output:20,cacheRead:0,cacheWrite:0,repliesWithoutCacheFields:1}})]).repliesWithoutCacheFields).toBe(1)
    expect(amounts([record({tally:{input:100,output:20,cacheRead:0,cacheWrite:0}})]).repliesWithoutCacheFields).toBe(0)
  })
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
    expect(agentName('dsh')).toBe('DeepSeek Harness')
    expect(agentName('gemini')).toBe('Gemini CLI'); expect(agentName('antigravity')).toBe('Antigravity (export)')
    expect(agentName('opencode')).toBe('OpenCode'); expect(agentName('kilo')).toBe('Kilo CLI'); expect(agentName('micode')).toBe('MiMo Code')
    expect(agentName('hindsight')).toBe('Hindsight'); expect(agentName('mcode')).toBe('MiniMax Code')
    expect(agentName('not-implemented')).toBe('not-implemented')
  })
  it('names the thirteen sources ported after 0.1.17 exactly as the original does', () => {
    // Three Antigravity entries are separate products with separate stores;
    // their qualifiers are the whole difference between them.
    expect(agentName('grok')).toBe('Grok Build'); expect(agentName('devin-cli')).toBe('Devin')
    expect(agentName('devin-desktop')).toBe('Devin Desktop'); expect(agentName('senpi')).toBe('OmO Native')
    expect(agentName('prime-agent')).toBe('Prime Agent'); expect(agentName('codebuddy')).toBe('CodeBuddy')
    expect(agentName('workbuddy')).toBe('WorkBuddy'); expect(agentName('antigravity-cli')).toBe('Antigravity CLI')
    expect(agentName('antigravity-ide')).toBe('Antigravity IDE'); expect(agentName('freebuff')).toBe('Freebuff')
    expect(agentName('trae')).toBe('Trae'); expect(agentName('warp')).toBe('Warp')
    expect(agentName('copilot')).toBe('GitHub Copilot'); expect(agentName('fx')).toBe('FX')
  })
  it('includes Harness in totals and filters its models without losing cache usage', () => {
    const all = [record(), record({ agent: 'dsh', model: 'deepseek-chat', tally: { input: 100, output: 40, cacheWrite: 10, cacheRead: 200 } }),
      record({ agent: 'dsh', model: 'other-model', cost: null, costBreakdown: null })]
    const s = summarize(all, 'today', new Date(2026, 9, 3), 'dsh')
    expect(s.tokens).toBe(550); expect(s.tally.output).toBe(60); expect(s.tally.cacheRead).toBe(270)
    expect(s.agents[0].name).toBe('DeepSeek Harness'); expect(s.models).toHaveLength(2); expect(s.unpricedTokens).toBe(200)
    expect(summarize(all, 'today', new Date(2026, 9, 3), 'dsh', 'deepseek-chat').tokens).toBe(350)
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
    // A narrow symbol, not "US$": the card beside it is not quoting a
    // different currency. The figure itself is unchanged.
    expect(formatCost(null)).toBe('—'); expect(formatCost(0)).toBe('$0.00'); expect(formatCost(0.00001)).toBe('< $0.01')
    expect(shortTokens(12000)).toBe('1.20万'); expect(shortTokens(123)).toBe('123')
  })
  it('keeps bare measured totals unclassified and priced subsets marked partial', () => {
    const bare = record({ tally:{input:0,output:0,cacheRead:0,cacheWrite:0},unclassifiedTokens:500,cost:null,costBreakdown:null })
    const a = amounts([bare]); expect(a.tokens).toBe(500); expect(a.tally.input).toBe(0); expect(a.unclassifiedTokens).toBe(500); expect(a.cost).toBe(null)
    const b = amounts([record({unclassifiedTokens:100})]); expect(b.tokens).toBe(300); expect(b.unpricedTokens).toBe(100); expect(b.cost).toBe(0.02)
  })
})

describe('Token Spend spans', () => {
  const now = new Date(2026, 9, 3, 12)
  it('bounds every span the way the original does', () => {
    // Rolling spans count back from today inclusive; the calendar ones start at
    // the calendar's own first day, and last month ends where this one begins.
    expect(spanBounds('today', now)).toEqual({ from: '2026-10-03', until: null })
    expect(spanBounds('week', now)).toEqual({ from: '2026-09-27', until: null })
    expect(spanBounds('month', now)).toEqual({ from: '2026-09-04', until: null })
    expect(spanBounds('quarter', now)).toEqual({ from: '2026-07-06', until: null })
    expect(spanBounds('thisMonth', now)).toEqual({ from: '2026-10-01', until: null })
    expect(spanBounds('lastMonth', now)).toEqual({ from: '2026-09-01', until: '2026-10-01' })
    expect(spanBounds('thisYear', now)).toEqual({ from: '2026-01-01', until: null })
    expect(spanBounds('all', now)).toEqual({ from: null, until: null })
    expect(spanBounds('custom', now, ['2026-09-01', '2026-09-03'])).toEqual({ from: '2026-09-01', until: '2026-09-04' })
  })
  it('counts each span here and lets a custom range give its own last day', () => {
    expect(spanDays('today', now)).toHaveLength(1); expect(spanDays('quarter', now)).toHaveLength(90)
    expect(spanDays('thisMonth', now)).toHaveLength(3)
    expect(spanDays('lastMonth', now)).toHaveLength(30)
    expect(spanDays('thisYear', now)).toHaveLength(276)
    const custom = spanDays('custom', now, ['2026-09-28', '2026-10-01'])
    expect(custom).toEqual(['2026-09-28', '2026-09-29', '2026-09-30', '2026-10-01'])
  })
  it('keeps every day of the open span, from the first record to today', () => {
    const span = spanDays('all', now, null, [record({ day: '2026-09-30' })])
    expect(span).toEqual(['2026-09-30', '2026-10-01', '2026-10-02', '2026-10-03'])
    expect(spanDays('all', now)).toEqual(['2026-10-03'])
    // A record dated ahead of today extends the span rather than being dropped
    // into a day the series does not carry.
    expect(spanDays('week', now, null, [record({ day: '2026-10-09' })])).toContain('2026-10-09')
  })
  it('treats a one-day span differently from a week', () => {
    expect(spanIsOneDay('today')).toBe(true)
    expect(spanIsOneDay('week')).toBe(false)
    expect(spanIsOneDay('custom', ['2026-10-03', '2026-10-03'])).toBe(true)
    expect(spanIsOneDay('custom', ['2026-10-02', '2026-10-03'])).toBe(false)
  })
  it('accepts a stored range only as two real days in order', () => {
    expect(customDaysOf(['2026-09-01', '2026-09-30'])).toEqual(['2026-09-01', '2026-09-30'])
    expect(customDaysOf(['2026-09-30', '2026-09-01'])).toBe(null)
    expect(customDaysOf(['2026-02-30', '2026-03-01'])).toBe(null)
    expect(customDaysOf(['2026-09-01'])).toBe(null); expect(customDaysOf(null)).toBe(null)
  })
  it('reads the hour series only over a single day', () => {
    const hours = [record({ hour: 9 }), record({ hour: 11 }), record({ hour: 11, tally:{input:0,output:0,cacheWrite:0,cacheRead:0},unclassifiedTokens:600,cost:null,costBreakdown:null })]
    const one = summarize(hours, 'today', now)
    expect(one.activeHours).toBe(2); expect(one.peakHour).toBe(11); expect(one.hoursComplete).toBe(true)
    expect(one.busiestDay?.tokens).toBe(1000)
    // A record that states only a day cannot fill an hour, and the profile is
    // then a partial figure that must not be presented as the whole.
    const partial = summarize([...hours, record({ aggregateTiming: true })], 'today', now)
    expect(partial.hoursComplete).toBe(false); expect(partial.peakHour).toBe(null)
    const week = summarize([record()], 'week', now)
    expect(week.busiestDay?.day).toBe('2026-10-03'); expect(week.activeDays).toBe(1)
    // Every quiet day ties at zero and none of them is a busiest day.
    expect(summarize([], 'week', now).busiestDay).toBe(null)
  })
})
