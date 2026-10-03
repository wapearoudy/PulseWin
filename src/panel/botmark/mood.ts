/**
 * What a ring's mark is doing, and which of the upstream's 39 states says it.
 *
 * Port of the original's `BotMarkMood`. **This is the only place a Pulse fact
 * becomes a bot state.** The upstream catalogue is a character sheet — 39
 * moods, most of which Pulse has nothing to say about — and the temptation is
 * to reach into it from the view for whichever one looks nice. Five moods, each
 * standing for something the rail already knew, is the whole vocabulary: if the
 * mark is doing something, a reading somewhere says why.
 */
export type BotMarkMood = 'idle' | 'working' | 'fetching' | 'spent' | 'unavailable'

export const BOT_MARK_MOODS: BotMarkMood[] = [
  'idle',
  'working',
  'fetching',
  'spent',
  'unavailable',
]

/**
 * The upstream state each mood plays.
 *
 * `working` rather than `thinking`: thinking is a morph that replaces the body
 * with three dots, which at 16pt throws away both the face and the brand
 * colour — the two things that say *which* provider is busy.
 */
export const UPSTREAM_STATE: Record<BotMarkMood, string> = {
  idle: 'idle',
  working: 'working',
  fetching: 'searching',
  spent: 'sad',
  unavailable: 'confused',
}

/**
 * How much to exaggerate the rotation, so the mood can be seen at ring size.
 *
 * **Only the moods that are an event.** A ring that is idle should be quiet —
 * that is the reading — and a mark rocking away at nothing would be a rail that
 * always looks busy.
 */
export function rotationEmphasis(mood: BotMarkMood): number {
  switch (mood) {
    case 'working':
      return 2.4
    case 'fetching':
      return 1.6
    default:
      return 1
  }
}

/**
 * How hard the body breathes.
 *
 * **A pulse, not a pump.** Ten was tried and read as the mark resizing itself;
 * three is about 3% peak-to-peak against idle's 1.3% — visible when you look at
 * it, invisible when you are not.
 */
export function squashEmphasis(mood: BotMarkMood): number {
  switch (mood) {
    case 'working':
      return 3
    case 'fetching':
      return 2
    default:
      return 1
  }
}

/**
 * A multiplier on every wait the mark keeps — blinks, glances, changes of
 * expression. **Smaller is busier**, because upstream multiplies its gaps by
 * this. Half for working: it blinks and looks about twice as often.
 */
export function tempoEmphasis(mood: BotMarkMood): number {
  switch (mood) {
    case 'working':
      return 0.5
    case 'fetching':
      return 0.7
    default:
      return 1
  }
}

/**
 * Reads the mood off the same facts the ring is already drawn from.
 *
 * **Busy outranks spent.** A spent limit is a state of the world that will
 * still be true in a minute; a CLI turning is happening now, and it is the one
 * thing on this rail that answers "is it doing something". Nothing here looks
 * at the warning threshold: how close a limit is, is what the ring's colour
 * means, and saying it twice in two languages on one 36pt mark is how a rail
 * stops being readable at a glance.
 */
export function resolveMood(facts: {
  isBusy?: boolean
  isRefreshing?: boolean
  isSpent?: boolean
  hasReading?: boolean
}): BotMarkMood {
  if (facts.isBusy) return 'working'
  if (facts.isRefreshing) return 'fetching'
  if (facts.isSpent) return 'spent'
  if (!facts.hasReading) return 'unavailable'
  return 'idle'
}

/** Nil is not quiet: only a witnessed CLI write can start the 20-minute
 * quiet interval used by the sleepy persona's night routine. */
export function quietSince(lastWrite:number|null|undefined,now:number):boolean {
  return lastWrite!==null&&lastWrite!==undefined&&now-lastWrite>20*60*1000
}

/** A mark says what its ring's selected limit says, rather than borrowing
 * exhaustion from another budget the same login happens to carry. */
export function windowIsSpent(window:{isExhausted?:boolean;percentUsed?:number|null}|undefined):boolean {
  return !!window&&(!!window.isExhausted||(window.percentUsed??0)>=100)
}
