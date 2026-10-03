/**
 * Port of Pulse's `UsageTint` / `WarningThreshold` (Sources/Pulse/Panel/UsageTint.swift).
 *
 * Colour here means ONE thing only — how close this limit is to running out.
 * The original used to draw rings in each provider's brand colour, "which read
 * as a status even though it never was one: Claude Code's orange-red looked
 * like a warning at 3% used."
 */

export type UsageTintName = 'good' | 'caution' | 'warning' | 'exhausted'

/**
 * Picked to sit on the panel's black: bright enough to read at ring size,
 * without the neon cast that fully saturated values take on there.
 */
export const USAGE_TINT: Record<UsageTintName, string> = {
  /** `.pulseGood` */
  good: '#00E68C',
  /** `.pulseCaution` */
  caution: '#FFC226',
  /** `.pulseWarning` */
  warning: '#FF4F42',
  /**
   * Deeper and flatter than the warning red, "so a spent limit doesn't just
   * look like a slightly redder nearly-spent one".
   */
  exhausted: '#D91721',
}

/** Comfortable below this. */
export const CAUTION_THRESHOLD = 0.5

/**
 * Where the ring turns red. A short list rather than a slider: "this is the one
 * step in the colour language that means 'pay attention'". Every option sits
 * above `CAUTION_THRESHOLD` so moving it never has to push the yellow step out
 * of its way.
 */
export const WARNING_THRESHOLDS = [0.6, 0.7, 0.75, 0.8, 0.85, 0.9] as const

export const DEFAULT_WARNING_THRESHOLD = 0.75

export interface TintInput {
  /** 0..1. `null`/`undefined` when the provider reported no figure. */
  usedFraction: number | null | undefined
  /**
   * Whether the provider itself says this limit is spent.
   *
   * **Not derived from the fraction.** The original is explicit: "a lock can
   * happen well short of 100%". `UsageRingView.isSpent` comes from the
   * provider's own flag.
   */
  isExhausted?: boolean
  warningAt?: number
}

/**
 * The colour for how much of a limit is gone.
 *
 * There is deliberately **no default for `warningAt`** in the original — "a
 * default here is how one ring on the panel comes to disagree with the one
 * beside it — silently, and only for whoever moved the figure". The default
 * here is the shipped value, and callers that render more than one ring should
 * thread one number through all of them.
 */
export function usageTint({
  usedFraction,
  isExhausted = false,
  warningAt = DEFAULT_WARNING_THRESHOLD,
}: TintInput): UsageTintName {
  // A locked limit is a different state, not "more red".
  if (isExhausted || (usedFraction !== null && usedFraction !== undefined && usedFraction >= 1)) {
    return 'exhausted'
  }

  // No reading: the original draws an empty track and never inverts nil into a
  // full "100% left" circle.
  if (usedFraction === null || usedFraction === undefined || Number.isNaN(usedFraction)) {
    return 'good'
  }

  if (usedFraction < CAUTION_THRESHOLD) return 'good'
  if (usedFraction < warningAt) return 'caution'
  return 'warning'
}

/** True when a limit is spent, from the provider's own flag or a full fraction. */
export function isSpent(
  usedFraction: number | null | undefined,
  isExhausted = false,
): boolean {
  if (isExhausted) return true
  return usedFraction !== null && usedFraction !== undefined && usedFraction >= 1
}
