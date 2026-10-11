import {describe,it,expect} from 'vitest'
import {buildRecap,recapPeriods,recapPrice,workdays,recapFilename} from './recap'
import type {SpendRecord} from './types'
const record=(day:string,tokens=100,cost:number|null=1,extra:Partial<SpendRecord>={}):SpendRecord=>({agent:'claude',model:'m',modelName:null,day,hour:12,session:day,project:'secret-project',tally:{input:tokens,output:0,cacheWrite:0,cacheRead:0},cost,costBreakdown:cost===null?null:[cost,0,0,0],...extra})
describe('calendar recap rules',()=>{
  it('running periods compare daily averages over observed matching days; future days stay out',()=>{
    const r=buildRecap([record('2026-09-01'),record('2026-09-02'),record('2026-10-01',200),record('2026-10-02',200),record('2026-10-03',99999)],'2026-10',20,new Date(2026,9,2,23))
    expect(r.summary.tokens).toBe(400);expect(r.summary.days).toHaveLength(2);expect(r.change).toBe(1);expect(r.paid).toBeCloseTo(20*2/31);expect(r.payback).toBeCloseTo(2/(20*2/31))
  })
  it('unknown earlier history limits subscription days and never invents a previous period',()=>{
    const r=buildRecap([record('2026-10-10')],'2026-10',31,new Date(2026,9,11));expect(r.observedDays).toBe(2);expect(r.paid).toBe(2);expect(r.change).toBeNull();expect(r.recordsBegin?.getDate()).toBe(10)
  })
  it('whole past months compare per day, empty future periods produce no cards',()=>{
    const records=[record('2026-01-01',3100),record('2026-02-01',2800)];const r=buildRecap(records,'2026-02',20,new Date(2026,3,11));expect(r.change).toBeCloseTo(0);expect(r.paid).toBe(20);expect(r.isInProgress).toBe(false);expect(buildRecap(records,'2027',20,new Date(2026,3,11)).cards).toEqual([])
  })
  it('unpriced and untimed work do not remove the priced subtotal or reliable timing',()=>{
    const r=buildRecap([record('2026-10-01',100,5),record('2026-10-02',10,null,{aggregateTiming:true})],'2026-10',20,new Date(2026,9,2));expect(r.summary.cost).toBe(5);expect(r.untimedTokens).toBe(10);expect(r.cards).toContain('payback');expect(r.cards).toContain('timetable');expect(r.summary.unpricedTokens).toBe(10)
    const all=buildRecap([record('2026-10-01',100,null,{aggregateTiming:true})],'2026-10',20,new Date(2026,9,2));expect(all.cards).not.toContain('timetable');expect(all.cards).not.toContain('payback')
  })
  it('prices accept deliberate comma formats and refuse coercions',()=>{
    for(const [text,value] of [['$ 20',20],['12,5',12.5],['1,200.50',1200.5],['',null],['0',null]] as const)expect(recapPrice(text)).toBe(value)
    for(const text of ['1e3','-5','1,2,3','20 USD','2.123','10000.01'])expect(recapPrice(text)).toBe('invalid')
  })
  it('pickers use earliest records with a 2020 floor and seven-day default',()=>{
    const records=[record('2026-09-01')];expect(recapPeriods(records,false,new Date(2026,9,3)).defaultKey).toBe('2026-09');expect(recapPeriods(records,false,new Date(2026,9,8)).defaultKey).toBe('2026-10');expect(recapPeriods([record('1970-01-01')],true,new Date(2026,9,8)).keys.slice(-1)[0]).toBe('2020')
  })
  it('streaks stay within the period, ties choose the earliest busy day',()=>{
    const r=buildRecap([record('2026-10-01'),record('2026-10-02')],'2026-10',null,new Date(2026,9,3));expect(r.currentStreak).toBe(2);expect(r.longestStreak).toBe(2);expect(r.busiest?.day).toBe('2026-10-01');expect(recapFilename(r,'poster')).toBe('pulse-recap-2026-10-01-poster.png')
  })
  it('work before 5am belongs to previous night and prolonged unattended work is excluded',()=>{
    const epoch=(hour:number)=>new Date(2026,9,2,hour).getTime()/1000;expect(workdays([epoch(1),epoch(2)])[0]).toEqual({endMinute:26*60,crossesMidnight:true});expect(workdays(Array.from({length:23},(_,h)=>epoch(h)))).toEqual([])
  })
})
