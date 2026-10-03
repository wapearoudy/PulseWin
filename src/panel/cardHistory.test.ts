import { describe,it,expect } from 'vitest'
import { budgetEstimate,cardActivity } from './cardHistory'
import type { SpendRecord, SpendSnapshot } from '../token-spend/types'
const now=new Date(2026,9,3,12).getTime()
const row=(sourceTimestamp:number,day='2026-10-03'):SpendRecord=>({agent:'opencode',model:'raw-model',modelName:'Model',day,hour:10,session:'s',project:null,
  tally:{input:100,output:25,cacheWrite:5,cacheRead:50},unclassifiedTokens:0,cost:.5,costBreakdown:[.3,.1,.02,.08],sourceTimestamp})
const snapshot=(records:SpendRecord[]):SpendSnapshot=>({scannedAt:new Date(now).toISOString(),records,sources:[],notes:[],pricesAt:null,pricingStatus:'unavailable'})
describe('detailed card measured history',()=>{
  it('builds 31 calendar days and counts models/cache with exclusive kinds',()=>{
    const history=snapshot([row(now/1000),row(now/1000-86400,'2026-10-02'),row(now/1000-35*86400,'2026-08-29')])
    const result=cardActivity(history,new Date(now))
    expect(result.chart).toHaveLength(31)
    expect(result.figures.map(value=>value.tokens)).toEqual([180,360,360])
    expect(result.top).toEqual({name:'Model',share:1})
    expect(result.cacheHitRate).toBeCloseTo(50/155)
    history.notes=['partial read'];expect(cardActivity(history,new Date(now)).cacheHitRate).toBeNull()
  })
  it('retains unknown cost and unclassified tokens without invented cache rate',()=>{
    const record={...row(now/1000),cost:null,costBreakdown:null,unclassifiedTokens:25}
    const result=cardActivity(snapshot([record]),new Date(now))
    expect(result.figures[0].cost).toBeNull();expect(result.figures[0].tokens).toBe(205)
    expect(result.cacheHitRate).toBeNull()
  })
  it('estimates only a supported account-wide window with sufficient history',()=>{
    const reset=now+3600*1000,opened=now/1000-4*3600
    const history=snapshot([row(opened-900),row(now/1000-60)])
    const window={label:'5h',percentUsed:25,resetsAt:new Date(reset).toISOString(),windowSeconds:18000,detail:null}
    expect(budgetEstimate(window,history,now)).toEqual({spent:.5,full:2})
    expect(budgetEstimate({...window,scope:'model-pool'},history,now)).toBeNull()
    expect(budgetEstimate({...window,percentUsed:1},history,now)).toBeNull()
    expect(budgetEstimate(window,snapshot([row(now/1000-60)]),now)).toBeNull()
    expect(budgetEstimate(window,snapshot([{...row(opened-900),aggregateTiming:true},row(now/1000-60)]),now)).toBeNull()
    expect(budgetEstimate(window,snapshot([row(opened-900),{...row(now/1000-60),cost:null}]),now)).toBeNull()
  })
})
