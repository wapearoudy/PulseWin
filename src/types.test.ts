import { describe, expect, it } from 'vitest'
import { formatReset, peakUsed, railProviders, severityOf, type ProviderUsage } from './types'

function provider(overrides: Partial<ProviderUsage> = {}): ProviderUsage {
  return {
    id: 'codex',
    name: 'Codex',
    plan: null,
    account: null,
    windows: [],
    error: null,
    configured: true,
    fetchedAt: '2026-01-01T00:00:00Z',
    ...overrides,
  }
}

describe('railProviders', () => {
  it('drops a tool that is not on this machine', () => {
    const rail = railProviders([provider({ id: 'a' }), provider({ id: 'b', configured: false })])
    expect(rail.map((p) => p.id)).toEqual(['a'])
  })

  it('keeps a tool that is present but failed', () => {
    // A ring that vanishes when a fetch fails disappears exactly when the
    // reader is looking for it.
    const failed = provider({ windows: [], error: 'timed out', configured: true })
    expect(railProviders([failed])).toHaveLength(1)
  })

  it('keeps a tool with a reading', () => {
    const answered = provider({
      windows: [{ label: '7d', percentUsed: 40, resetsAt: null, detail: null }],
    })
    expect(railProviders([answered])).toHaveLength(1)
  })
})

describe('peakUsed', () => {
  it('takes the tightest window', () => {
    expect(
      peakUsed(
        provider({
          windows: [
            { label: '5h', percentUsed: 20, resetsAt: null, detail: null },
            { label: '7d', percentUsed: 63, resetsAt: null, detail: null },
          ],
        }),
      ),
    ).toBe(63)
  })

  it('is null rather than zero when nothing reported a number', () => {
    // "Pulse does not invent usage percentages."
    expect(peakUsed(provider({ windows: [] }))).toBeNull()
    expect(
      peakUsed(
        provider({
          windows: [
            { label: 'balance', percentUsed: null, resetsAt: null, detail: '$12.40' },
          ],
        }),
      ),
    ).toBeNull()
  })
})

describe('severityOf', () => {
  it('ramps at the original thresholds', () => {
    expect(severityOf(null)).toBe('unknown')
    expect(severityOf(0)).toBe('ok')
    expect(severityOf(69)).toBe('ok')
    expect(severityOf(70)).toBe('warning')
    expect(severityOf(89)).toBe('warning')
    expect(severityOf(90)).toBe('critical')
  })
})

describe('formatReset', () => {
  const now = new Date('2026-01-01T12:00:00Z')

  it('is compact at every scale', () => {
    expect(formatReset('2026-01-01T12:45:00Z', now)).toBe('resets in 45m')
    expect(formatReset('2026-01-01T15:30:00Z', now)).toBe('resets in 3h 30m')
    expect(formatReset('2026-01-02T12:00:00Z', now)).toBe('resets in 1d')
    expect(formatReset('2026-01-03T18:00:00Z', now)).toBe('resets in 2d 6h')
  })

  it('says nothing when there is nothing to say', () => {
    expect(formatReset(null, now)).toBeNull()
    expect(formatReset('not a date', now)).toBeNull()
    expect(formatReset('2026-01-01T11:00:00Z', now)).toBe('resetting…')
  })
})
