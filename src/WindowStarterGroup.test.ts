import { describe, expect, it } from 'vitest'
import { statusText } from './WindowStarterGroup'

/// The pane's only piece of logic: turning a stored outcome into a sentence.
/// It lives here rather than in Rust because the sentence is the pane's, and
/// the stored value is the starter's own string so a build that adds a case
/// still says something instead of printing a raw word.
describe('window starter status', () => {
  const at = 1_790_942_400 // 2026-10-02T12:00:00Z
  const stamp = new Date(at * 1000).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })

  it('says nothing has happened rather than a date of zero', () => {
    expect(statusText({ lastRun: null, lastOutcome: null })).toBe('尚未启动')
    // A run with no recorded outcome is still a run: the time is the fact.
    expect(statusText({ lastRun: at, lastOutcome: null })).toBe(stamp)
  })

  it('says a success as just the time', () => {
    expect(statusText({ lastRun: at, lastOutcome: 'sent' })).toBe(stamp)
  })

  it('says why a start did not go through, in the pane\'s own words', () => {
    expect(statusText({ lastRun: at, lastOutcome: 'toolMissing' })).toBe(`${stamp} · 找不到命令行工具`)
    expect(statusText({ lastRun: at, lastOutcome: 'failed' })).toBe(`${stamp} · 没有发出去，请检查工具是否已登录`)
    expect(statusText({ lastRun: at, lastOutcome: 'timedOut' })).toBe(`${stamp} · 没有及时应答`)
  })

  it('still shows the time for an outcome this build does not know', () => {
    // Written by a later build, read by an earlier one: the record is a run,
    // not a reason to print a raw token or hide that it happened.
    expect(statusText({ lastRun: at, lastOutcome: 'somethingNew' })).toBe(stamp)
  })
})
