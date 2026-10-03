import { describe, expect, it } from 'vitest'
import { headlineWindow, secondWindow, clockFraction } from './usagePresentation'
import type { ProviderUsage, UsageWindow } from './types'
const window = (percentUsed: number, label: string): UsageWindow => ({percentUsed,label,resetsAt:null,detail:null})
const provider = (windows: UsageWindow[]): ProviderUsage => ({id:'test',name:'test',windows,plan:null,account:null,error:null,configured:true,fetchedAt:''})
describe('headline selection', () => {
  it('honors a valid pin, falling back when that window disappears', () => {
    const p = provider([window(20,'5h'),window(80,'7d')])
    expect(headlineWindow(p,'5h')?.label).toBe('5h')
    expect(headlineWindow(p,'missing')?.label).toBe('7d')
  })
  it('selects the fullest reported window and retains a pinned restriction', () => {
    const p=provider([{...window(20,'locked'),isExhausted:true,id:'pool-locked'},window(90,'other')])
    expect(headlineWindow(p)?.label).toBe('other')
    expect(headlineWindow(p,'pool-locked')?.isExhausted).toBe(true)
  })
  it('pairs the second ring with the same model pool instead of an unrelated fuller pool', () => {
    const p=provider([{...window(90,'5h'),id:'g5',scope:'Gemini'},{...window(40,'7d'),id:'g7',scope:'Gemini'},{...window(80,'5h'),id:'c5',scope:'Claude'}])
    expect(secondWindow(p)?.id).toBe('g7')
    expect(secondWindow(p,'c5')?.id).toBe('g5')
  })
})
describe('reset clock', () => {
  it('does not invent duration from the 5h label', () => {
    expect(clockFraction({...window(20,'5h'),resetsAt:'2026-10-02T13:00:00Z'},Date.parse('2026-10-02T12:00:00Z'),false)).toBeNull()
  })
  it('uses only reported duration and clamps before and after the window', () => {
    const w = {...window(20,'5h'),resetsAt:'2026-10-02T13:00:00Z',windowSeconds:7200}
    expect(clockFraction(w,Date.parse('2026-10-02T12:00:00Z'),false)).toBe(.5)
    expect(clockFraction(w,Date.parse('2026-10-02T12:00:00Z'),true)).toBe(.5)
    expect(clockFraction(w,Date.parse('2026-10-02T14:00:00Z'),true)).toBe(0)
    expect(clockFraction(w,Date.parse('2026-10-02T10:00:00Z'),true)).toBe(1)
  })
})
