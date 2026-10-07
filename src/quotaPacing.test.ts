import { describe, expect, it } from 'vitest'
import { quotaTimeFraction, quotaWarning } from './quotaPacing'
import type { UsageWindow } from './types'

const now=Date.parse('2026-10-07T12:00:00Z')
const window=(used:number,elapsed:number,duration=18000):UsageWindow=>({label:'quota',percentUsed:used,windowSeconds:duration,resetsAt:new Date(now+duration*(1-elapsed)*1000).toISOString(),detail:null})
describe('quota pace',()=>{
  it('keeps equal and behind progress normal and preserves the minimum threshold',()=>{
    expect(quotaWarning(window(75,.75),.75,now)).toBe(false)
    expect(quotaWarning(window(80,.9),.75,now)).toBe(false)
    expect(quotaWarning(window(74.9,.1),.75,now)).toBe(false)
    expect(quotaWarning(window(75,.1),.75,now)).toBe(true)
    expect(quotaWarning(window(80,.75),.75,now)).toBe(true)
  })
  it('uses the actual duration equally for five-hour and weekly cycles',()=>{
    for(const duration of [18000,604800]) {
      expect(quotaTimeFraction(window(80,.75,duration),now)).toBe(.75)
      expect(quotaWarning(window(80,.75,duration),.75,now)).toBe(true)
    }
  })
  it('requires current valid cycle evidence and never infers a cycle from a label',()=>{
    const valid=window(95,.75)
    for(const invalid of [{...valid,windowSeconds:null},{...valid,resetsAt:null},{...valid,resetsAt:'invalid'},{...valid,windowSeconds:NaN},{...valid,windowSeconds:0},window(95,-.1),window(95,1),{...valid,kind:'balance' as const},{...valid,kind:'topUp' as const}]) {
      expect(quotaTimeFraction(invalid,now)).toBeNull()
      expect(quotaWarning(invalid,.75,now)).toBe(false)
    }
    expect(quotaWarning({...valid,percentUsed:NaN},.75,now)).toBe(false)
  })
  it('continues to distinguish actual exhaustion from pace, including explicit restrictions',()=>{
    expect(quotaWarning({...window(100,.99),windowSeconds:null},.75,now)).toBe(true)
    expect(quotaWarning({...window(20,.75),isExhausted:true},.75,now)).toBe(true)
    expect(quotaWarning({...window(100,.75),kind:'balance'},.75,now)).toBe(false)
  })
})
