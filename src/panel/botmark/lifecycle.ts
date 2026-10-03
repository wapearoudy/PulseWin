import { Spring, completionState, type Persona } from './programme'
import { clamp, range, wrapped, type Random } from './math'
import type { BotMarkMood } from './mood'

/** Timestamps are evidence supplied by the usage/activity stores. Mounting a
 * mark, a cached reading or a repeated render is never itself an event. */
export class EventPlayback {
  private finish:number
  private reset:number
  private until=0
  private kind:'finish'|'reset'|null=null
  constructor(finishedAt=0,resetAt=0){this.finish=finishedAt;this.reset=resetAt}
  update(now:number,wallClock:number,persona:Persona,mood:BotMarkMood,finishedAt=0,resetAt=0,allowReset=true) {
    const recent=(stamp:number)=>stamp>0&&wallClock-stamp>=0&&wallClock-stamp<=20000
    if(finishedAt!==this.finish){this.finish=finishedAt;if(recent(finishedAt)&&mood!=='working'&&this.kind!=='reset'){this.kind='finish';this.until=now+2600}}
    if(resetAt!==this.reset){this.reset=resetAt;if(allowReset&&recent(resetAt)){this.kind='reset';this.until=now+6400}}
    if(!allowReset&&this.kind==='reset')this.until=0
    if(mood==='working'&&this.kind==='finish')this.until=0
    if(now>=this.until)this.kind=null
    return this.kind?{state:this.kind==='reset'?'celebrate':completionState(persona),key:`${this.kind}:${this.kind==='reset'?this.reset:this.finish}`,duration:this.kind==='reset'?6400:2600}:null
  }
}

/** Outgoing effect geometry survives until the exit spring reaches rest.
 * Progress and gather play a shot, rest 1500ms, then play another. */
export class MorphLifecycle {
  amount=new Spring(0)
  blend=new Spring(1)
  turn=new Spring(0)
  active:string|null=null
  previous:string|null=null
  started=0
  shotStarted=0
  private requested:string|null=null
  private visible=false
  private resting=false
  private restStarted=0
  private direction=1
  constructor(private random:Random=Math.random){}
  update(effect:string|null,state:string,now:number,enabled=true) {
    const requested=enabled?effect:null
    if(requested!==this.requested){this.requested=requested;this.shotStarted=now;this.resting=false}
    let visible=requested!==null
    if(requested&&(state==='progress'||state==='spawning')){
      const shot=state==='progress'?2500:2000
      if(!this.resting&&now-this.shotStarted>shot){this.resting=true;this.restStarted=now}
      else if(this.resting&&now-this.restStarted>1500){this.resting=false;this.shotStarted=now}
      visible=!this.resting
    }
    this.amount.target=visible?1:0
    if(requested&&requested!==this.active){
      this.previous=this.active&&this.amount.value>.02?this.active:null
      this.blend.value=this.previous?0:1;this.blend.velocity=0;this.blend.target=1
      this.active=requested;this.started=now
    }
    if(!requested&&this.amount.value<.004){this.active=null;this.previous=null;this.blend.value=1;this.blend.velocity=0;this.blend.target=1}
    if(this.previous&&this.blend.value>.996)this.previous=null
    if(visible!==this.visible){if(visible)this.direction=this.random()<.5?1:-1;this.turn.target+=Math.PI*this.direction;this.visible=visible}
  }
  step(delta:number){this.amount.step(14,1,delta);this.blend.step(11,1,delta);this.turn.step(14,1,delta)}
}

export type DrawnPose={x:number;y:number;degrees:number;turn:number}
/** Carries direct poses across a state handoff; springs head toward zero,
 * angles unwind by the shortest route rather than undoing completed turns. */
export class PoseContinuity {
  private drawn:DrawnPose|null=null
  private state=''
  private carry={x:new Spring(0),y:new Spring(0),degrees:new Spring(0),turn:new Spring(0)}
  step(delta:number){for(const s of Object.values(this.carry))s.step(12,1,delta)}
  apply(state:string,raw:DrawnPose):DrawnPose {
    if(this.drawn&&state!==this.state){for(const k of ['x','y','degrees','turn'] as const){const difference=this.drawn[k]-raw[k],s=this.carry[k];s.value=k==='degrees'?wrapped(difference,360):k==='turn'?wrapped(difference,2*Math.PI):difference;s.velocity=0;s.target=0}}
    this.drawn={x:raw.x+this.carry.x.value,y:raw.y+this.carry.y.value,degrees:raw.degrees+this.carry.degrees.value,turn:raw.turn+this.carry.turn.value};this.state=state
    return this.drawn
  }
}

export class BlinkQueue {
  private queue:{at:number;value:number}[]=[]
  private target:number|null=null
  constructor(private random:Random=Math.random){}
  reset(){this.queue=[];this.target=null}
  schedule(now:number){this.queue.push({at:now,value:.05},{at:now+70,value:.05},{at:now+150,value:1.08},{at:now+300,value:1});if(this.random()<.14)this.queue.push({at:now+370,value:.05},{at:now+480,value:1})}
  update(now:number){while(this.queue.length&&now>=this.queue[0].at)this.target=this.queue.shift()!.value;const value=this.target;if(!this.queue.length)this.target=null;return value}
  get blinking(){return this.queue.length>0||this.target!==null}
}

/** Delayed notice and sprung attention keep a rail of marks from snapping
 * to a newly arrived pointer together. */
export class PointerAttention {
  focus=new Spring(0)
  x=0;y=0
  private notice:number|null=null
  constructor(private random:Random=Math.random){}
  update(now:number,delta:number,point:{x:number;y:number}|null){
    if(point){if(this.notice===null)this.notice=now+range(this.random,50,250)}else this.notice=null
    const noticed=this.notice!==null&&now>=this.notice
    this.focus.target=noticed?1:0;this.focus.step(11,1,delta)
    const smoothing=1-Math.exp(60*Math.log(.91)*delta),tx=noticed&&point?22*clamp(point.x,-.6,.6):0,ty=noticed&&point?14*clamp(point.y,-.6,.6):0
    // Upstream advances this once per eye: twice per rendered frame.
    for(let i=0;i<2;i++){this.x+=(tx-this.x)*smoothing;this.y+=(ty-this.y)*smoothing}
  }
}
