import type { AvatarState } from 'open-bot-avatar'
import type { BotMarkMood } from '../botmark/mood'

export const avatarState: Record<BotMarkMood, AvatarState> = {
  idle: 'idle',
  working: 'working',
  fetching: 'thinking',
  spent: 'blocked',
  unavailable: 'waiting',
}

export function resolveRobotMood({ hasReading, isSpent, isBusy, isRefreshing }: {
  hasReading: boolean; isSpent: boolean; isBusy: boolean; isRefreshing: boolean
}): BotMarkMood {
  if (isRefreshing) return 'fetching'
  if (isBusy) return 'working'
  if (!hasReading) return 'unavailable'
  return isSpent ? 'spent' : 'idle'
}

type Gesture = { kind: 'reset' | 'finish'; until: number }

// Only a timestamp that changes after mount can start a gesture. Cached
// readings and routine renders cannot replay a previous celebration.
export class AvatarPlayback {
  private gesture: Gesture | null = null

  constructor(private finishedAt: number, private resetAt: number) {}

  update(finishedAt: number, resetAt: number, allowReset: boolean, mood: BotMarkMood, now: number): Gesture | null {
    if (this.gesture && (this.gesture.until <= now || (this.gesture.kind === 'reset' && !allowReset)
      || (this.gesture.kind === 'finish' && (mood === 'working' || mood === 'fetching')))) this.gesture = null
    const recent = (at: number) => Number.isFinite(at) && at > 0 && now >= at && now - at <= 20_000
    if (finishedAt > this.finishedAt && recent(finishedAt) && this.gesture?.kind !== 'reset'
      && mood !== 'working' && mood !== 'fetching') {
      this.gesture = { kind: 'finish', until: now + 3000 }
    }
    if (resetAt > this.resetAt && allowReset && recent(resetAt)) this.gesture = { kind: 'reset', until: now + 4500 }
    this.finishedAt = Math.max(this.finishedAt, finishedAt)
    this.resetAt = Math.max(this.resetAt, resetAt)
    return this.gesture
  }
}
