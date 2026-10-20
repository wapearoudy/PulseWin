import { describe, expect, it } from 'vitest'
import { dayTitle, hostOf, periodEnd, periodLabel, uptimeText, type Reading } from './ServiceStatusGroup'

describe('official status pane', () => {
  it('shows the page a person opens, without its scheme', () => {
    expect(hostOf('https://status.openai.com')).toBe('status.openai.com')
    expect(hostOf('https://www.githubstatus.com')).toBe('www.githubstatus.com')
    // The feed's own address is never what the row shows.
    expect(hostOf('https://raw.githubusercontent.com/x/y/master/history/summary.json')).toBe('raw.githubusercontent.com')
  })

  it('counts a ninety-day page in days and a three-hour page in days too', () => {
    const day = (hours: number | null) => ({ date: '2026-10-13', state: null, colour: null, hours })
    expect(periodLabel(Array.from({ length: 90 }, () => day(null)))).toBe('90 天前')
    // OpenCode draws 56 bars of three hours: seven days, not fifty-six.
    expect(periodLabel(Array.from({ length: 56 }, () => day(3)))).toBe('7 天前')
  })

  it('says now for the page whose last bar is the one under way', () => {
    expect(periodEnd([{ date: '2026-10-13', state: null, colour: null, hours: null }])).toBe('今天')
    expect(periodEnd([{ date: '2026-10-13 09:00', state: null, colour: null, hours: 3 }])).toBe('现在')
  })

  it('prints the publisher figure, and nothing where it published none', () => {
    expect(uptimeText(99.95)).toBe('99.95% 正常运行 · 官方报告')
    expect(uptimeText(100)).toBe('100% 正常运行 · 官方报告')
    expect(uptimeText(null)).toBeNull()
    // A figure that isn't a number is not shown as one.
    expect(uptimeText(Number.NaN)).toBeNull()
    expect(uptimeText(Number.POSITIVE_INFINITY)).toBeNull()
  })

  it('names the day and the state a bar covers, with its hours', () => {
    expect(dayTitle({ date: '2026-10-13', state: 'fullOutage', colour: null, hours: null })).toBe('2026-10-13 · 全面故障')
    // A day with no record says so rather than looking clear.
    expect(dayTitle({ date: '2026-10-13', state: null, colour: null, hours: null })).toBe('2026-10-13 · 无数据')
    expect(dayTitle({ date: '2026-10-13 09:00', state: 'degraded', colour: null, hours: 3 })).toBe('2026-10-13 09:00–12:00 · 性能下降')
    // Midnight's bar runs to the end of the day, not to hour 24.
    expect(dayTitle({ date: '2026-10-13 21:00', state: 'operational', colour: null, hours: 3 })).toBe('2026-10-13 21:00–00:00 · 运行正常')
  })

  it('reads a page as the Rust side writes it', () => {
    const reading: Reading = {
      page: 'grok-bot',
      address: 'https://status.cursor.com',
      company: 'Cursor',
      palette: 'cursor',
      checkedAt: '2026-10-13T00:00:00Z',
      historyError: null,
      components: [{ id: 'sm5wkcnqkvr9', name: 'Grok Bot', state: 'operational', uptime: 99.9, group: null, days: [] }],
    }
    expect(reading.components[0].name).toBe('Grok Bot')
    expect(hostOf(reading.address)).toBe('status.cursor.com')
  })
})
