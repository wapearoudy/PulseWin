import { describe, expect, it } from 'vitest'
import {
  MODEL_GROUP_BUDGET,
  canHideGroup,
  defaultPreferences,
  groupSlotId,
  groupsShown,
  modelGroups,
  splitsIntoGroups,
} from './preferences'
import { SOURCE_OPTIONS } from './AntigravitySettingsGroup'

// The rail's rules live in TypeScript because the panel is what draws the
// slots: the backend reports one window per bucket with its group as `scope`,
// and how many rings those scopes become is a layout decision.

const argon = [
  { id: 'gemini-5h', scope: 'Gemini' },
  { id: 'gemini-weekly', scope: 'Gemini' },
  { id: '3p-5h', scope: 'Claude and GPT' },
  { id: '3p-weekly', scope: 'Claude and GPT' },
  { id: 'gemini-argon-wtu-weekly', scope: 'Gemini 4 Argon' },
]

describe('model groups', () => {
  it('keeps the groups in the order the provider reported them', () => {
    expect(modelGroups(argon)).toEqual(['Gemini', 'Claude and GPT', 'Gemini 4 Argon'])
  })

  it('a reading that mixes scoped and unscoped windows has no groups', () => {
    // Splitting it would drop the unscoped rows, so it stays one ring.
    expect(modelGroups([{ scope: 'Gemini' }, { scope: null }])).toEqual([])
    expect(modelGroups([{ scope: 'Gemini' }, {}])).toEqual([])
  })

  it('no windows means no groups', () => {
    expect(modelGroups([])).toEqual([])
  })

  it('a group named Argon stays whole', () => {
    // "Gemini 4 Argon" carries no trailing "models", so the name is the scope.
    expect(modelGroups([{ id: 'gemini-argon-wtu-weekly', scope: 'Gemini 4 Argon' }])).toEqual(['Gemini 4 Argon'])
  })
})

describe('splitting', () => {
  it('splits two or three groups when the switch is on', () => {
    expect(splitsIntoGroups(true, ['Gemini', 'Claude and GPT'])).toBe(true)
    expect(splitsIntoGroups(true, ['Gemini', 'Claude and GPT', 'Gemini 4 Argon'])).toBe(true)
  })

  it('more groups than the budget keeps the account whole', () => {
    // Dropping the extra group would hide an allowance.
    expect(splitsIntoGroups(true, ['a', 'b', 'c', 'd'])).toBe(false)
    expect(MODEL_GROUP_BUDGET).toBe(3)
  })

  it('one group is one ring, and the switch being off is no split', () => {
    expect(splitsIntoGroups(true, ['Gemini'])).toBe(false)
    expect(splitsIntoGroups(false, ['Gemini', 'Claude and GPT'])).toBe(false)
  })
})

describe('the groups a split account draws', () => {
  it('no choice at all draws every group', () => {
    const groups = ['Gemini', 'Claude and GPT', 'Gemini 4 Argon']
    expect(groupsShown(groups, [])).toEqual(groups)
    expect(groupsShown(groups, undefined)).toEqual(groups)
  })

  it('a choice filters the groups, in the provider order', () => {
    const groups = ['Gemini', 'Claude and GPT', 'Gemini 4 Argon']
    expect(groupsShown(groups, ['Gemini 4 Argon', 'Gemini'])).toEqual(['Gemini', 'Gemini 4 Argon'])
  })

  it('a stale choice that matches nothing keeps every group', () => {
    // An account must not end up with no rings because a group was renamed.
    const groups = ['Gemini', 'Claude and GPT']
    expect(groupsShown(groups, ['Gemini 3 Pro'])).toEqual(groups)
  })

  it('the last drawn group cannot be switched off', () => {
    const groups = ['Gemini', 'Claude and GPT']
    expect(canHideGroup(groups, ['Gemini', 'Claude and GPT'], 'Gemini')).toBe(true)
    expect(canHideGroup(groups, ['Gemini'], 'Gemini')).toBe(false)
    // A group that is already off can always be turned back on.
    expect(canHideGroup(groups, ['Gemini'], 'Claude and GPT')).toBe(true)
  })

  it('an unsplit slot keeps the account id and a split one adds the group', () => {
    expect(groupSlotId('antigravity')).toBe('antigravity')
    expect(groupSlotId('antigravity', 'Gemini 4 Argon')).toBe('antigravity@Gemini 4 Argon')
    // `@` cannot collide with the `#` an added account uses.
    expect(groupSlotId('antigravity', null)).toBe('antigravity')
  })
})

describe('the route picker', () => {
  it('offers automatic, CLI and desktop in that order', () => {
    expect(SOURCE_OPTIONS.map(option => option.value)).toEqual(['auto', 'cli', 'desktop'])
    expect(SOURCE_OPTIONS.map(option => option.label)).toEqual(['自动', 'CLI（agy）', '桌面端'])
  })

  it('every route says what it reads', () => {
    for (const option of SOURCE_OPTIONS) expect(option.subtitle.length).toBeGreaterThan(0)
  })

  it('the default is automatic and nothing is split until asked for', () => {
    expect(defaultPreferences.antigravityUsageSource).toBe('auto')
    expect(defaultPreferences.splitAccounts).toEqual([])
    expect(defaultPreferences.shownModelGroups).toEqual({})
  })
})
