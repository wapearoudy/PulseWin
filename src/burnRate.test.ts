import { describe,expect,it } from 'vitest'
import { approximate,burnRate } from './burnRate'
const now=Date.parse('2026-10-02T12:00:00Z')
const reading=(percentUsed:number,remaining:number,duration=18000)=>({percentUsed,windowSeconds:duration,resetsAt:new Date(now+remaining*1000).toISOString(),label:'5h',detail:null})
describe('original single-reading forecast',()=>{
  it('withholds for missing evidence, early windows and past resets',()=>{
    for(const value of [{...reading(20,9000),windowSeconds:null},{...reading(20,9000),percentUsed:null},reading(20,17999),reading(20,0),reading(20,20000),reading(NaN,9000)])expect(burnRate(value,now)).toBeNull()
  })
  it('separates a verdict from the short actionable horizon',()=>{
    expect(burnRate(reading(20,9000),now)).toEqual({exhaustsBeforeReset:false,timeToExhaustion:null})
    const near=burnRate(reading(80,9000),now)!;expect(near.exhaustsBeforeReset).toBe(true);expect(near.timeToExhaustion).toBeCloseTo(2250)
    expect(burnRate(reading(30,15000),now)?.timeToExhaustion).toBeCloseTo(7000)
    expect(burnRate(reading(60,50000,100000),now)).toEqual({exhaustsBeforeReset:true,timeToExhaustion:null})
  })
  it('handles empty and exhausted amounts without division errors',()=>{
    expect(burnRate(reading(0,9000),now)).toEqual({exhaustsBeforeReset:false,timeToExhaustion:null})
    expect(burnRate(reading(100,9000),now)).toEqual({exhaustsBeforeReset:true,timeToExhaustion:0})
  })
  it('does not pretend to predict minute precision',()=>{expect(approximate(53*60)).toBe('约 1 小时');expect(approximate(75*60)).toBe('约 1.5 小时');expect(approximate(6*60)).toBe('不足 15 分钟')})
})
