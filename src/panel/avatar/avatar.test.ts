import { describe, expect, it } from 'vitest'
import { validateAvatar } from 'open-bot-avatar'
import { avatarDefinition, robotFigures, spreadRobotFigures } from './catalog'
import { AvatarPlayback, avatarState, resolveRobotMood } from './playback'

describe('Open Bot Avatar integration', () => {
  it('mounts each upstream silhouette with valid, stable account-specific seeds', () => {
    expect(robotFigures).toHaveLength(23)
    expect(new Set(robotFigures.map(([id]) => id)).size).toBe(23)
    for (const [figure] of robotFigures) {
      const first = avatarDefinition(figure, 'codex')
      const second = avatarDefinition(figure, 'codex--account-test')
      expect(validateAvatar(first).ok, figure).toBe(true)
      expect(first).toEqual(avatarDefinition(figure, 'codex'))
      expect(first?.id).not.toBe(second?.id)
      expect(first?.body.path).toBe(second?.body.path)
    }
  })

  it('preserves chosen figures while avoiding their immediate automatic neighbours', () => {
    expect(spreadRobotFigures(['legacy', 'mug', null, 'cat'])).toEqual(['ghost', 'mug', 'lightbulb', 'cat'])
  })

  it('shows each account state and plays only new gestures, then returns to its state', () => {
    expect(avatarState).toEqual({ idle: 'idle', working: 'working', fetching: 'thinking', spent: 'blocked', unavailable: 'waiting' })
    const player = new AvatarPlayback(1_000, 1_000)
    expect(player.update(1_000, 1_000, true, 'idle', 1_001)).toBeNull()
    expect(player.update(1_002, 1_002, true, 'idle', 1_003)).toEqual({ kind: 'reset', until: 5_503 })
    expect(player.update(1_003, 1_002, true, 'idle', 1_004)?.kind).toBe('reset')
    expect(player.update(1_003, 1_002, true, 'idle', 5_503)).toBeNull()
    expect(player.update(5_504, 1_002, true, 'idle', 5_504)?.kind).toBe('finish')
    expect(player.update(5_504, 1_002, true, 'working', 5_510)).toBeNull()
    expect(player.update(5_504, 5_511, false, 'idle', 5_511)).toBeNull()
    expect(player.update(5_504, 5_511, true, 'idle', 5_512)).toBeNull()
  })

  it('gives explicit refresh feedback and does not show an unreported limit as exhausted', () => {
    const facts = { hasReading: false, isSpent: true, isBusy: true, isRefreshing: true }
    expect(resolveRobotMood(facts)).toBe('fetching')
    expect(resolveRobotMood({ ...facts, isRefreshing: false })).toBe('working')
    expect(resolveRobotMood({ ...facts, isRefreshing: false, isBusy: false })).toBe('unavailable')
    expect(resolveRobotMood({ ...facts, isRefreshing: false, isBusy: false, hasReading: true })).toBe('spent')
  })
})
