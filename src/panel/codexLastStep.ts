import type { SpendRecord } from '../token-spend/types'

/** The latest completed, timed Codex call in the past hour. Older cached
 * records without its actual end time cannot stand in for this measurement. */
export function lastCodexStep(records: readonly SpendRecord[], now: number) {
  let latest: { tokensPerSecond: number; finishedAt: number } | null = null
  for (const record of records) {
    const timing = record.replyTiming
    if (record.agent !== 'codex' || !timing || timing.replies !== 1) continue
    const finishedAt = timing.finishedAt
    if (finishedAt == null || !Number.isFinite(finishedAt) || finishedAt > now || now - finishedAt >= 3_600_000) continue
    const tokensPerSecond = timing.outputTokens / timing.seconds
    if (tokensPerSecond <= 0 || !Number.isFinite(tokensPerSecond)) continue
    if (latest === null || finishedAt > latest.finishedAt) latest = { tokensPerSecond, finishedAt }
  }
  return latest
}
