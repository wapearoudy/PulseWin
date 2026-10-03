/** Mirrors `src-tauri/src/model.rs` — serde serializes these as camelCase. */

export interface UsageWindow {
  id?: string | null
  scope?: string | null
  kind?: 'other' | 'limit' | 'balance' | 'topUp' | 'credits' | 'sharedCredits'
  isExhausted?: boolean
  windowSeconds?: number | null
  label: string
  /** 0..100, or null when the provider did not report a usable number. */
  percentUsed: number | null
  /** RFC3339 timestamp of the next reset. */
  resetsAt: string | null
  detail: string | null
}

export interface ProviderUsage {
  stale?: boolean
  source?: string | null
  creditRemaining?: { amount: number; currency: string } | null
  id: string
  name: string
  plan: string | null
  account: string | null
  windows: UsageWindow[]
  /** Set instead of `windows` when the fetch failed. */
  error: string | null
  /**
   * Whether this tool is present on the machine at all.
   *
   * Not the same question as "did it answer": a tool with a credential that
   * failed to fetch still belongs on the rail, while one that is not installed
   * must not be drawn at all, because a ring at zero is a reading and "you do
   * not have this" is not.
   */
  configured: boolean
  fetchedAt: string
}

export interface Snapshot {
  providers: ProviderUsage[]
  fetchedAt: string
}

export type Severity = 'ok' | 'warning' | 'critical' | 'unknown'

/** Traffic-light thresholds, matching Pulse's colour ramp. */
export function severityOf(percentUsed: number | null): Severity {
  if (percentUsed === null || Number.isNaN(percentUsed)) return 'unknown'
  if (percentUsed >= 90) return 'critical'
  if (percentUsed >= 70) return 'warning'
  return 'ok'
}

export function percentLeft(percentUsed: number | null): number | null {
  if (percentUsed === null || Number.isNaN(percentUsed)) return null
  return Math.max(0, Math.min(100, 100 - percentUsed))
}

/** "resets in 3h 20m" / "resets in 4d" — compact, like the original. */
export function formatReset(resetsAt: string | null, now: Date = new Date()): string | null {
  if (!resetsAt) return null
  const target = new Date(resetsAt)
  const ms = target.getTime() - now.getTime()
  if (Number.isNaN(ms)) return null
  if (ms <= 0) return 'resetting…'

  const minutes = Math.floor(ms / 60_000)
  if (minutes < 60) return `resets in ${minutes}m`

  const hours = Math.floor(minutes / 60)
  if (hours < 24) {
    const rem = minutes % 60
    return rem > 0 ? `resets in ${hours}h ${rem}m` : `resets in ${hours}h`
  }

  const days = Math.floor(hours / 24)
  const rem = hours % 24
  return rem > 0 ? `resets in ${days}d ${rem}h` : `resets in ${days}d`
}

/** Highest utilization across a provider's windows — drives the card ring. */
export function peakUsed(provider: ProviderUsage): number | null {
  const values = provider.windows
    .map((w) => w.percentUsed)
    .filter((v): v is number => v !== null && !Number.isNaN(v))
  if (values.length === 0) return null
  return Math.max(...values)
}

/**
 * The providers the rail draws.
 *
 * Present on this machine, whether or not it answered. Dropping the ones that
 * failed would take a ring away exactly when it has the most to say: a tool
 * that went quiet is the thing the reader is looking for.
 */
export function railProviders(providers: ProviderUsage[]): ProviderUsage[] {
  return providers.filter((p) => p.configured)
}
