import { describe, expect, it } from 'vitest'
import { BotMarkEngine, settledFrame, type EngineInput } from './engine'
import { EventPlayback, MorphLifecycle, PoseContinuity, PointerAttention, BlinkQueue } from './lifecycle'
import { celebratePose } from './gestures'
import { Particles } from './particles'
import { C, wrapped } from './math'
import { PERSONAS } from './programme'
import { SHAPES } from './data'
import { EffectMemory, effects, MORPH_SIZES } from './effects'
import { shapeSpanAt } from './geometry'
import { quietSince, windowIsSpent } from './mood'
function seeded(seed=0x5eed){return ()=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed/4294967296}}

describe('original rail facts',()=>{
  it('requires a witnessed last write and more than twenty minutes for quiet',()=>{
    expect(quietSince(null,10000000)).toBe(false);expect(quietSince(undefined,10000000)).toBe(false)
    expect(quietSince(1000,301000)).toBe(false);expect(quietSince(1000,1201000)).toBe(false);expect(quietSince(1000,1201001)).toBe(true)
  })
  it('bases exhaustion on the chosen budget, including a provider-reported spent flag without percentage',()=>{
    const headline={percentUsed:20},other={percentUsed:100}
    expect(windowIsSpent(headline)).toBe(false);expect(windowIsSpent(other)).toBe(true)
    expect(windowIsSpent({isExhausted:true,percentUsed:null})).toBe(true);expect(windowIsSpent(undefined)).toBe(false)
  })
})

describe('witnessed BotMark events',()=>{
  it('does not celebrate timestamps already present on mount or replay them',()=>{
    const event=new EventPlayback(100000,100001)
    expect(event.update(0,100002,'calm','idle',100000,100001)).toBeNull()
    expect(event.update(10,100020,'calm','idle',100020,100001)?.state).toBe('happy')
    expect(event.update(2611,102621,'calm','idle',100020,100001)).toBeNull()
    expect(event.update(3000,103010,'calm','idle',100020,100001)).toBeNull()
  })
  it('holds a reset for the complete 6400ms even if the mood changes',()=>{
    const event=new EventPlayback()
    expect(event.update(100,100000,'stoic','idle',0,100000)?.state).toBe('celebrate')
    expect(event.update(6400,106300,'stoic','working',106300,100000)?.state).toBe('celebrate')
    expect(event.update(6500,106400,'stoic','working',106300,100000)).toBeNull()
  })
  it('acknowledges disabled, stale and future evidence without later replay',()=>{
    const event=new EventPlayback()
    expect(event.update(0,100000,'calm','idle',0,100000,false)).toBeNull()
    expect(event.update(1,100001,'calm','idle',0,100000,true)).toBeNull()
    expect(event.update(2,100002,'calm','idle',0,70000)).toBeNull()
    expect(event.update(3,100003,'calm','idle',0,100004)).toBeNull()
    expect(event.update(4,100004,'calm','idle',0,100004)).toBeNull()
  })
  it('never treats working as a witnessed completion',()=>{
    const event=new EventPlayback()
    expect(event.update(0,100000,'proud','working',100000)).toBeNull()
    expect(event.update(1,100001,'proud','idle',100000)).toBeNull()
  })
})

describe('original morph lifecycle',()=>{
  it('retains outgoing geometry throughout the exit and blends incoming effects',()=>{
    const morph=new MorphLifecycle(seeded())
    morph.update('dots','thinking',0)
    for(let i=0;i<60;i++)morph.step(1/60)
    morph.update(null,'idle',1000)
    expect(morph.active).toBe('dots');expect(morph.amount.target).toBe(0)
    morph.update('pencil','writing',1033)
    expect(morph.previous).toBe('dots');expect(morph.blend.value).toBe(0)
    for(let i=0;i<120;i++){morph.update(null,'idle',2000+i*1000/60);morph.step(1/60)}
    expect(morph.active).toBeNull();expect(morph.previous).toBeNull()
  })
  it.each([['progress','progress',2500],['spawning','gather',2000]] as const)('rests %s shots for 1500ms', (state,effect,shot)=>{
    const morph=new MorphLifecycle(seeded());morph.update(effect,state,0);morph.update(effect,state,shot+1)
    expect(morph.amount.target).toBe(0)
    morph.update(effect,state,shot+1501);expect(morph.amount.target).toBe(0)
    morph.update(effect,state,shot+1502);expect(morph.amount.target).toBe(1);expect(morph.shotStarted).toBe(shot+1502)
  })
  it('draws finite geometry for every effect and every enter/exit amount',()=>{
    const memory=new EffectMemory(seeded())
    for(const effect of Object.keys(MORPH_SIZES))for(const amount of [0,.01,.3,.62,1])for(const elapsed of [0,500,1500,3500]){
      const fx=effects(effect,elapsed,elapsed,amount,memory)
      expect(fx.svg).not.toMatch(/NaN|Infinity/)
      expect([fx.x,fx.y,fx.r,fx.scale,fx.opacity].every(Number.isFinite)).toBe(true)
    }
  })
  it('writes an actual curved nib trail, then consumes it when the nib lifts',()=>{
    const memory=new EffectMemory(seeded())
    for(let t=0;t<1600;t+=33)effects('pencil',t,t,1,memory)
    const count=memory.writingTrail.length;expect(count).toBeGreaterThan(10)
    const fx=effects('pencil',1750,1750,1,memory)
    expect(fx.svg).toContain('stroke-width="6"');expect(memory.writingTrail.length).toBe(count-2)
  })
})

describe('original particles and celebration',()=>{
  it('completes nine yaw turns and three body turns before the final shake',()=>{
    const pose=celebratePose(3930,1)
    expect(pose.turn).toBeCloseTo(18*Math.PI,6)
    expect(pose.rotation).toBeGreaterThan(1060);expect(pose.rotation).toBeLessThan(1100)
    expect(celebratePose(6300).wild).toBe(false)
    expect(celebratePose(3930,-1).turn).toBeCloseTo(-18*Math.PI,6)
  })
  it('paints tapered ribbons both behind and ahead, and returns them when a spin stops',()=>{
    const particles=new Particles(seeded())
    let both=false,count=0
    for(let i=0;i<90;i++){const frame=particles.update(i*1000/30,1/30,i/30*10,2.6,true,true,C,'test');count=Math.max(count,frame.count);both||=!!frame.back&&!!frame.front;expect(frame.back+frame.front).not.toMatch(/NaN|Infinity/)}
    expect(count).toBe(9);expect(both).toBe(true)
    let frame=particles.update(3000,1/30,89/30*10,2.6,true,true,C,'test')
    for(let i=0;i<30;i++)frame=particles.update(3033+i*1000/30,1/30,89/30*10,2.6,true,true,C,'test')
    expect(frame.count).toBe(0)
  })
  it('clears all particle history when the mood gate closes',()=>{
    const particles=new Particles(seeded());particles.burst(16)
    expect(particles.update(0,1/30,0,2.6,false,true,C,'test').count).toBe(16)
    expect(particles.update(33,1/30,0,2.6,false,false,C,'test')).toEqual({back:'',front:'',count:0})
    for(const body of Object.values(SHAPES)){expect(Number.isFinite(body.beltRadius)).toBe(true);expect(body.spanSamples).toHaveLength(160);expect(shapeSpanAt(body,(body.top+body.bottom)/2).every(Number.isFinite)).toBe(true)}
  })
})

describe('persistent frame engine',()=>{
  it('busy moods emit ribbons at 25pt while the same idle gestures do not',()=>{
    for(const mood of ['working','idle'] as const){const input:EngineInput={mood,persona:'eager'},engine=new BotMarkEngine(input,seeded());let particles=0;for(let i=0;i<420;i++)particles+=engine.advance(1/30,input,100000+i*1000/30,25).particleCount;expect(particles>0).toBe(mood==='working')}
  })
  it.each(PERSONAS)('keeps %s continuous through live state and event changes',persona=>{
    const random=seeded(PERSONAS.indexOf(persona)+17),input:EngineInput={mood:'idle',persona},engine=new BotMarkEngine(input,random)
    let previous:ReturnType<BotMarkEngine['advance']>|null=null,worstMove=0,worstTurn=0,next=2
    for(let i=0;i<60*30;i++){
      const time=i/30,wall=100000+time*1000
      if(time>=next){const roll=random();if(roll<.5)input.mood=(['idle','working','fetching','spent','unavailable'] as const)[Math.floor(random()*5)];else if(roll<.7)input.isPointedAt=!input.isPointedAt;else if(roll<.85)input.finishedAt=wall;else input.resetAt=wall;next=time+1+random()*4}
      const frame=engine.advance(1/30,input,wall)
      expect(frame.head+frame.transform+frame.eyes.map(e=>e.transform).join('')).not.toMatch(/NaN|Infinity/)
      if(previous){worstMove=Math.max(worstMove,Math.hypot(frame.position.x-previous.position.x,frame.position.y-previous.position.y));worstTurn=Math.max(worstTurn,Math.abs(wrapped(frame.position.degrees-previous.position.degrees,360)))}previous=frame
    }
    expect(worstMove).toBeLessThan(30);expect(worstTurn).toBeLessThan(30)
  })
  it('caches a legible reduced-motion still and ignores pointer/news in it',()=>{
    const input:EngineInput={mood:'fetching',persona:'curious',body:'gem',gaze:'right'}
    const first=settledFrame(input),next=settledFrame({...input,pointer:{x:1,y:1},finishedAt:100000,resetAt:100001})
    expect(next).toBe(first);expect(first.particleCount).toBe(0);expect(first.state).not.toBe('celebrate');expect(first.morphAmount).toBeGreaterThan(.9)
  })
  it('hands off direct motion exactly, then decays the carry instead of accumulating it',()=>{
    const continuity=new PoseContinuity(),before=continuity.apply('celebrate',{x:90,y:45,degrees:1080,turn:18*Math.PI})
    const after=continuity.apply('idle',{x:0,y:0,degrees:0,turn:0})
    expect(after.x).toBe(before.x);expect(after.y).toBe(before.y);expect(wrapped(after.degrees-before.degrees,360)).toBe(0)
    for(let i=0;i<90;i++)continuity.step(1/30)
    const rest=continuity.apply('idle',{x:0,y:0,degrees:0,turn:0});expect(rest.x).toBeCloseTo(0,5);expect(rest.y).toBeCloseTo(0,5)
  })
  it('notices a pointer gradually and runs double-blink queues to completion',()=>{
    const attention=new PointerAttention(()=>.5);attention.update(0,1/30,{x:1,y:1});expect(attention.focus.value).toBe(0)
    for(let t=33;t<1000;t+=33)attention.update(t,1/30,{x:1,y:1})
    expect(attention.focus.value).toBeGreaterThan(.99);expect(Math.abs(attention.x-13.2)).toBeLessThan(.001)
    const blink=new BlinkQueue(()=>0);blink.schedule(0);expect(blink.update(0)).toBe(.05);expect(blink.update(150)).toBe(1.08);expect(blink.update(370)).toBe(.05);expect(blink.update(480)).toBe(1);expect(blink.blinking).toBe(false)
  })
})
