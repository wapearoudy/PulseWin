import { describe,it,expect } from 'vitest'
import { activityStep,activityWindow,tokenActivity } from './activity'
import type { SpendRecord } from './types'
const row=(day:string,input=100,agent='claude'):SpendRecord=>({agent,model:'m',modelName:null,day,hour:9,session:'s',project:null,tally:{input,output:0,cacheRead:0,cacheWrite:0},cost:null,costBreakdown:null})
describe('twelve-month Token activity',()=>{
  it('draws at most 53 calendar weeks including leap days',()=>{
    for(const now of [new Date(2026,9,11),new Date(2024,1,29),new Date(2025,1,28),new Date(2026,0,1)]){
      const a=tokenActivity([row('2020-01-01'),row(`${now.getFullYear()}-${String(now.getMonth()+1).padStart(2,'0')}-${String(now.getDate()).padStart(2,'0')}`)],now)
      expect(a.weeks.length).toBeLessThanOrEqual(53);expect(a.weeks.length).toBeGreaterThanOrEqual(52)
    }
    const w=activityWindow(new Date(2024,1,29));expect(w.start.getMonth()).toBe(2);expect(w.start.getDate()).toBe(1)
  })
  it('distinguishes unrecorded history, observed quiet days and days outside the window',()=>{
    const a=tokenActivity([row('2026-10-10')],new Date(2026,9,11))
    expect(a.weeks.flatMap(w=>w.unrecorded).some(Boolean)).toBe(true)
    expect(a.weeks.flatMap(w=>w.days).filter(Boolean).map(d=>d?.tokens)).toEqual([100,0]);expect(a.total).toBe(100);expect(a.activeDays).toBe(1)
    expect(tokenActivity([row('2027-01-01')],new Date(2026,9,11)).weeks).toHaveLength(0)
  })
  it('counts exactly the displayed window, clips partial weeks and filters agents and models',()=>{
    const now=new Date(2026,9,11),a=tokenActivity([row('2025-01-01',999),row('2026-10-10',100),row('2026-10-11',200,'codex')],now)
    expect(a.total).toBe(300);expect(a.weeks.reduce((s,w)=>s+w.tokens,0)).toBe(a.total)
    expect(tokenActivity([row('2026-10-10',100),row('2026-10-11',200,'codex')],now,'codex').total).toBe(200)
    expect(tokenActivity([row('2026-10-10')],now,null,'missing').weeks).toHaveLength(0)
  })
  it('uses busy-day quartiles rather than the largest day and preserves exact totals',()=>{
    const huge={...row('2026-10-11',9007199254740992),exactTally:{input:'9007199254740993',output:'0',cacheRead:'0',cacheWrite:'0'}}
    const a=tokenActivity([row('2026-10-08',1),row('2026-10-09',2),row('2026-10-10',3),huge],new Date(2026,9,11));expect(a.cuts).toEqual([1,2,3]);expect(activityStep(2,a.cuts)).toBe(2);expect(activityStep(0,a.cuts)).toBe(0);expect(a.exactTotal).toBe('9007199254740999')
    expect(a.marks[a.marks.length-1].label).toBe('10月')
  })
})
