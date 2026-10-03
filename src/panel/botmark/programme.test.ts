import { describe,it,expect } from 'vitest'
import { PERSONAS, routine, personaAt, Spring } from './programme'
import { SHAPES, STATES } from './data'
import { turnedRing } from './solid'
import { percentFigure } from '../../usagePresentation'
const date=new Date(2026,8,30,12)
describe('original BotMark programme semantics',()=>{
  it('never plays work effects as idle personality',()=>{
    const everyday=new Set(PERSONAS.flatMap(p=>[...routine(p,'idle',false,false,date),...routine(p,'idle',true,false,date)].map(b=>b[0])))
    for(const p of PERSONAS)for(const [id,lo,hi] of routine(p,'working',false,false,date)){expect(everyday.has(id)).toBe(false);expect(hi).toBeGreaterThanOrEqual(lo)}
  })
  it('all eight complete authored scenes use available artwork',()=>{
    for(const p of PERSONAS)for(const mood of ['idle','working','fetching','spent','unavailable'] as const){const beats=routine(p,mood,false,false,date);expect(beats.length).toBeGreaterThan(1);for(const [id] of beats)expect(STATES[id]).toBeDefined()}
    expect(new Set(PERSONAS.map(p=>routine(p,'idle',false,false,date).map(b=>b[0]).join(','))).size).toBe(8)
    expect(personaAt('automatic',8)).toBe('calm')
  })
  it('limits sleepy night behavior to idle, quiet nights',()=>{
    const night=new Date(2026,8,30,23)
    expect(routine('sleepy','idle',false,true,night).slice(-1)[0]?.[0]).toBe('drowsy')
    expect(routine('calm','idle',false,true,night).some(b=>b[0]==='drowsy')).toBe(false)
    expect(routine('sleepy','idle',true,true,night).some(b=>b[0]==='drowsy')).toBe(false)
  })
  it('keeps interrupted spring motion finite at different frame rates',()=>{
    for(const dt of [1/30,1/60,1/120]){const s=new Spring(0);s.target=1;for(let i=0;i<30;i++)s.step(14,1,dt);s.target=-1;for(let i=0;i<120;i++)s.step(14,1,dt);expect(s.value).toBeCloseTo(-1,3)}
  })
  it('yaw leaves every body a closed finite 96-point silhouette',()=>{
    for(const body of Object.values(SHAPES))for(const a of [0,Math.PI/2,Math.PI]){const ring=turnedRing(body,a);expect(ring).toHaveLength(96);expect(ring.every(p=>Number.isFinite(p.x)&&Number.isFinite(p.y))).toBe(true)}
  })
})
it('keeps tiny used and remaining readings away from misleading endpoints',()=>{
  expect(percentFigure(.0003)).toBe(1);expect(percentFigure(.996)).toBe(99)
  expect(percentFigure(.996,true)).toBe(1);expect(percentFigure(.004,true)).toBe(99)
  expect(percentFigure(0)).toBe(0);expect(percentFigure(1)).toBe(100)
})
