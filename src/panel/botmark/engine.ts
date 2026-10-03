import { EXPRESSIONS, EYE_HALF, shape, state, type BotMarkFace } from './data'
import { centroid, lerpRing, ringOutline, ringPath, shapeSpanAt, type Point } from './geometry'
import { rotationEmphasis, squashEmphasis, tempoEmphasis, type BotMarkMood } from './mood'
import { personality, routine, Spring, type Persona } from './programme'
import { turnedRing, spanAt } from './solid'
import { pose, StateMotion } from './motion'
import { effects, EffectMemory, MORPH_SIZES, MORPH_VIEWBOXES } from './effects'
import { BlinkQueue, EventPlayback, MorphLifecycle, PointerAttention, PoseContinuity } from './lifecycle'
import { Gestures, blankGesture } from './gestures'
import { Particles } from './particles'
import { C, clamp, cubicInOut, range, smoothstep, type Random } from './math'

export interface EngineInput {
  mood:BotMarkMood; body?:string; pointer?:Point|null; animated?:boolean
  persona?:Persona; gaze?:'left'|'right'|'top'; isPointedAt?:boolean
  isQuiet?:boolean; finishedAt?:number; resetAt?:number; resetCelebrationAllowed?:boolean
}
export interface MarkFrame {
  state:string; body:string; head:string; transform:string; opacity:number
  eyes:{path:string;transform:string;visible:boolean}[]
  shapes:string; badge:string; backParticles:string; frontParticles:string; particleCount:number
  viewBox:string; position:{x:number;y:number;degrees:number;turn:number}; morphAmount:number
}
const faceMix=(a:BotMarkFace,b:BotMarkFace,m:number):BotMarkFace=>({x:a.x+(b.x-a.x)*m,y:a.y+(b.y-a.y)*m,sx:a.sx+(b.sx-a.sx)*m,sy:a.sy+(b.sy-a.sy)*m,eye:a.eye+(b.eye-a.eye)*m})
const circle=shape('blob').ring.map((_,i)=>({x:C+C*Math.cos(i/96*Math.PI*2),y:C+C*Math.sin(i/96*Math.PI*2)}))
const teardrop=shape('teardrop').ring
const pencil=teardrop.map((_,i)=>{const p=teardrop[(i+teardrop.length/2)%teardrop.length];return {x:2*C-p.x,y:2*C-p.y}})
const reflected=(ring:Point[])=>ring.map((_,j)=>{const p=ring[(ring.length-j)%ring.length];return {x:2*C-p.x,y:p.y}})

/** Clock, queues and springs outlive React updates. Every drawing comes from
 * this production seam, which also permits deterministic frame regressions. */
export class BotMarkEngine {
  private clock=0
  private active=''
  private key=''
  private cursor=0
  private next=0
  private started=0
  private expressionAt=0
  private expressionCursor=0
  private expressionEntries=new Map<string,number>()
  private from=EXPRESSIONS[0]
  private to=this.from
  private drawn=this.from
  private blend=new Spring(1)
  private expressionFrequency=7
  private expressionIndex=0
  private blinkAt=0
  private gazeAt=0
  private spinAngle=0
  private particleSpinAngle=0
  private rotation=new Spring(0)
  private x=new Spring(0)
  private y=new Spring(0)
  private sy=new Spring(1)
  private eyelid=new Spring(1)
  private eyeSize=new Spring(1)
  private gazeX=new Spring(0)
  private gazeY=new Spring(0)
  private facing:Spring
  private humming=new Spring(0)
  private notification=new Spring(0)
  private bodyId:string
  private bodyFrom:Point[]
  private faceFrom:BotMarkFace
  private tiltFrom:number
  private beltFrom:number
  private shapeBlend=new Spring(1)
  private morph:MorphLifecycle
  private events:EventPlayback
  private gestures:Gestures
  private particles:Particles
  private blinks:BlinkQueue
  private attention:PointerAttention
  private continuity=new PoseContinuity()
  private memory:EffectMemory
  private motion:StateMotion
  constructor(initial:EngineInput,private random:Random=Math.random,private settled=false){
    this.facing=new Spring(initial.gaze==='right'?-1:1)
    const body=shape(initial.body??'blob');this.bodyId=body.id;this.bodyFrom=body.ring;this.faceFrom=body.face;this.tiltFrom=body.tiltScale;this.beltFrom=body.beltRadius
    this.morph=new MorphLifecycle(random);this.events=new EventPlayback(initial.finishedAt,initial.resetAt);this.gestures=new Gestures(random);this.particles=new Particles(random);this.blinks=new BlinkQueue(random);this.attention=new PointerAttention(random);this.memory=new EffectMemory(random);this.motion=new StateMotion(random)
  }
  private rand(lo:number,hi:number){return range(this.random,lo,hi)}
  private expression(index:number,frequency=7){
    if(this.to===EXPRESSIONS[index]&&this.blend.target===1)return
    this.from=this.drawn;this.to=EXPRESSIONS[index]??EXPRESSIONS[0];this.blend.value=0;this.blend.velocity=0;this.blend.target=1;this.expressionFrequency=frequency
    this.expressionIndex=index
  }
  advance(delta:number,input:EngineInput,wallClock=Date.now(),viewWidth=28,id='bot'):MarkFrame {
    delta=clamp(delta,0,.1);this.clock+=delta*1000
    const now=this.clock,moving=input.animated!==false,persona=input.persona??'calm',character=personality(persona),tempo=character.tempo*tempoEmphasis(input.mood)
    const event=this.settled?null:this.events.update(now,wallClock,persona,input.mood,input.finishedAt,input.resetAt,input.resetCelebrationAllowed!==false)
    const authored=routine(persona,input.mood,input.isPointedAt,input.isQuiet),scene=this.settled?authored.slice(0,1):authored,key=moving&&event?event.key:`${persona}:${input.mood}:${!!input.isPointedAt}:${scene.map(b=>b[0])}`
    if(this.key!==key||moving&&now>=this.next){
      this.cursor=this.key!==key?0:(this.cursor+1)%scene.length;this.key=key
      const [identifier,lo,hi]=moving&&event?[event.state,event.duration,event.duration]:scene[this.cursor]
      this.next=now+this.rand(lo,hi)
      if(identifier!==this.active){
        this.active=identifier;this.started=now
        const info=state(identifier),entry=this.expressionEntries.get(identifier)??0;this.expressionCursor=entry%info.expressionPool.length;this.expressionEntries.set(identifier,(this.expressionCursor+1)%info.expressionPool.length)
        if(identifier!=='sleeping'&&identifier!=='waking')this.expression(info.expressionPool[this.expressionCursor],identifier==='excited'?10:8)
        this.expressionAt=now+this.rand(...info.expressionCadence)*tempo;this.blinkAt=now+this.rand(1500,7000);this.gazeAt=now+this.rand(500,1400)
        this.blinks.reset();if(identifier!=='drowsy'&&identifier!=='waking'&&identifier!=='sleeping'&&moving)this.blinks.schedule(now)
        this.gestures.enter(identifier,now)
        this.motion.enter(now)
      }
    }
    const info=state(this.active),body=shape(input.body??'blob'),elapsed=now-this.started
    if(body.id!==this.bodyId){
      const previous=shape(this.bodyId),amount=cubicInOut(clamp(this.shapeBlend.value))
      this.bodyFrom=lerpRing(this.bodyFrom,previous.ring,amount);this.faceFrom=faceMix(this.faceFrom,previous.face,amount);this.tiltFrom+=(previous.tiltScale-this.tiltFrom)*amount;this.beltFrom+=(previous.beltRadius-this.beltFrom)*amount
      this.bodyId=body.id;this.shapeBlend.value=0;this.shapeBlend.velocity=0;this.shapeBlend.target=1
      if(moving&&this.gestures.shapeChanged(now))this.particles.burst(16,.95,.3)
    }
    this.morph.update(info.morph,this.active,now,moving)
    const stateMotion=moving?this.motion.update(this.active,now,elapsed,this.expressionIndex,this.blend.value,this.eyelid.value,info.expressionPool):null
    const target=stateMotion?.target??pose(this.active,3000,3000),motion=moving?character.motion:0,gesture=moving?this.gestures.update(this.active,now,elapsed,delta,tempo):blankGesture()
    if(stateMotion){this.rotation.velocity+=stateMotion.rotationImpulse;this.y.velocity+=stateMotion.yImpulse;if(stateMotion.burst)this.particles.burst(stateMotion.burst,.8);if(stateMotion.expressionTarget!==undefined)this.expression(stateMotion.expressionTarget,this.active==='waking'?12:11);if(stateMotion.blink&&!this.blinks.blinking)this.blinks.schedule(now)}
    this.rotation.target=target.r*motion*rotationEmphasis(input.mood);this.x.target=target.x*motion;this.y.target=target.y*motion
    this.sy.target=moving?clamp(1+(target.sy-1)*squashEmphasis(input.mood),.9,1.12):1
    this.eyeSize.target=(gesture.eyeScale??target.eyes)*character.eyes;this.facing.target=input.gaze==='right'?-1:1
    if(moving&&now>=this.expressionAt&&this.active!=='waking'&&this.active!=='sleeping'){
      this.expressionCursor=(this.expressionCursor+1+Math.floor(this.rand(0,Math.max(info.expressionPool.length-1,1))))%info.expressionPool.length
      this.expression(info.expressionPool[this.expressionCursor],this.active==='searching'||this.active==='excited'?10:6);this.expressionAt=now+this.rand(...info.expressionCadence)*tempo
    }
    if(moving&&info.blinkCadence&&now>=this.blinkAt){this.blinks.schedule(now);this.blinkAt=now+this.rand(...info.blinkCadence)*tempo}
    this.eyelid.target=(moving?this.blinks.update(now):null)??gesture.eyeOpen??target.open
    if(!moving){this.gazeX.target=this.gazeY.target=0}
    else if(now>=this.gazeAt){
      const aim=aimForState(this.active,this.random),bias=input.gaze==='top'?0:7
      this.gazeX.target=(bias&&this.random()<.8?Math.abs(aim.x):aim.x)*character.gaze;this.gazeY.target=aim.y*character.gaze;this.gazeAt=now+this.rand(aim.lo,aim.hi)*tempo
    }
    this.humming.target=moving&&this.active==='humming'?1:0;this.notification.target=this.active==='notifying'?1:0
    if(moving&&(this.active==='humming'||this.active==='loading')){const seconds=elapsed/1000,settled=this.active==='loading'?3:1.6,speed=seconds<.5?7*cubicInOut(seconds/.5):seconds<1.3?7+(settled-7)*cubicInOut((seconds-.5)/.8):settled+.3*Math.sin(.5*seconds);this.spinAngle+=speed*delta}
    else if(this.active!=='celebrate')this.spinAngle*=.94
    for(const [s,f,d] of [[this.rotation,5,.9],[this.x,3.5,1],[this.y,4,1],[this.sy,10,.8],[this.eyeSize,9,.85],[this.eyelid,26,1],[this.gazeX,13,1],[this.gazeY,13,1],[this.facing,9,.85],[this.blend,this.expressionFrequency,1],[this.shapeBlend,10,1],[this.humming,6,1],[this.notification,9,.55]] as const)s.step(f,d,delta)
    this.morph.step(delta);this.continuity.step(delta)
    this.attention.update(now,delta,moving&&!this.settled?input.pointer??null:null)
    const shapeAmount=cubicInOut(clamp(this.shapeBlend.value)),transitioning=shapeAmount<.999,base=transitioning?lerpRing(this.bodyFrom,body.ring,shapeAmount):body.ring,face=faceMix(this.faceFrom,body.face,shapeAmount),tilt=this.tiltFrom+(body.tiltScale-this.tiltFrom)*shapeAmount,belt=this.beltFrom+(body.beltRadius-this.beltFrom)*shapeAmount
    const m=clamp(this.morph.amount.value),blend=clamp(this.morph.blend.value),previous=blend<.999?this.morph.previous:null
    const morphSize=(MORPH_SIZES[this.morph.active??'']??19)*blend+(MORPH_SIZES[previous??'']??MORPH_SIZES[this.morph.active??'']??19)*(1-blend)
    const fx=effects(this.morph.active,now,elapsed,m*blend,this.memory,m,now-this.morph.started,now-this.morph.shotStarted,morphSize)
    if(previous){const old=effects(previous,now,elapsed,m*(1-blend),this.memory,m,now-this.morph.started,now-this.morph.shotStarted,morphSize);fx.svg+=old.svg;fx.x+=old.x;fx.y+=old.y;fx.r+=old.r;fx.scale*=old.scale;fx.opacity*=old.opacity}
    const morphIsTurning=this.morph.amount.value>.001||Math.abs(this.morph.turn.target-this.morph.turn.value)>.01
    const rawTurn=(morphIsTurning?this.morph.turn.value:0)+gesture.turn
    const position=this.continuity.apply(this.active,{x:(this.x.value+gesture.x)*(1-m)+fx.x,y:(this.y.value+gesture.y+gesture.bounceY)*(1-m)+fx.y,degrees:this.rotation.value*tilt*(1-m)+gesture.rotation*(1-m)+fx.r,turn:rawTurn})
    const bodyRing=Math.abs(position.turn)>.001&&!transitioning?turnedRing(body,position.turn):base
    const morphRing=previous?lerpRing(previous==='pencil'?pencil:circle,this.morph.active==='pencil'?pencil:circle,cubicInOut(blend)):this.morph.active==='pencil'?pencil:circle
    const outline=m>0?ringOutline(lerpRing(bodyRing,morphRing,cubicInOut(clamp(m/.62)))):transitioning||Math.abs(position.turn)>.001&&(body.solid||body.sides>0)?ringOutline(bodyRing):body.outline
    this.drawn=this.from.map((r,i)=>lerpRing(r,this.to[i],clamp(this.blend.value)))
    const turn=clamp((1-this.facing.value)/2),eyeRings=this.drawn.map((r,i)=>lerpRing(r,reflected(this.drawn[1-i]),turn)),centres=eyeRings.map(centroid)
    const turned=Math.abs(position.turn)>.001,top=transitioning||turned?Math.min(...bodyRing.map(p=>p.y)):body.top,bottom=transitioning||turned?Math.max(...bodyRing.map(p=>p.y)):body.bottom,pair=(centres[0].x+centres[1].x)/2-C,focus=clamp(this.attention.focus.value),weight=1-.8*focus
    const halves=eyeRings.map((r,i)=>Math.max(...r.map(p=>Math.abs(p.x-centres[i].x)))),distance=Math.abs(centres[1].x-centres[0].x)*face.sx,fit=halves[0]+halves[1]>.5?clamp((distance-5)/(halves[0]+halves[1]),.35,4):4,pulse=1+.07*Math.sin(clamp(this.blend.value)*Math.PI)
    const eyes=eyeRings.map((ring,i)=>{
      const centre=centres[i];let localCentre=C+face.x,offset=(centre.x-C)*face.sx,perspective=1,visible=true,fade=1
      if(Math.abs(position.turn)>.001){const scanY=clamp(C+face.y+(centre.y-C)*face.sy,top+2,bottom-2),[left,right]=spanAt(bodyRing,scanY),radius=Math.max((right-left)/2,12),initial=Math.asin(clamp(offset/radius,-1,1)),angle=initial+position.turn,cosine=Math.cos(angle);localCentre=(left+right)/2;visible=cosine>.02;perspective=Math.max(cosine,.02)/Math.max(Math.cos(initial),.02);offset=radius*Math.sin(angle);fade=smoothstep(clamp(cosine/.5))}
      const scaled=Math.min(clamp(this.eyeSize.value,.2,2)*face.eye,fit/pulse),sx=clamp(perspective*scaled*pulse,.02,2.4),sy=clamp(Math.max(this.eyelid.value*(moving?this.gestures.wink(now,i):1),.04)*scaled*pulse,.02,2.4)
      let drift=(1.4*Math.sin(.00042*now+i)+.5*Math.sin(.001*now+2*i)+this.gazeX.value*weight+gesture.gazeX+(input.gaze==='top'?0:7)*weight)*this.facing.value-pair*(1-weight)+this.attention.x*focus-10*clamp(this.notification.value)
      const dy=.9*Math.sin(.00058*now+i)+this.attention.y*focus+this.gazeY.value*weight+gesture.gazeY+7*clamp(this.notification.value),halfHeight=EYE_HALF*sy+2,yy=clamp(C+face.y+(centre.y+dy-C)*face.sy,top+halfHeight,bottom-halfHeight)
      let left=-Infinity,right=Infinity
      for(let j=0;j<ring.length;j+=2){const sampleY=yy+(ring[j].y-centre.y)*sy,at=transitioning||turned?spanAt(bodyRing,sampleY):shapeSpanAt(body,sampleY);left=Math.max(left,at[0]-(ring[j].x-centre.x)*sx);right=Math.min(right,at[1]-(ring[j].x-centre.x)*sx)}
      const wanted=localCentre+offset+drift*face.sx,bounded=left<=right?clamp(wanted,left,right):(left+right)/2;let xx=bounded+(wanted-bounded)*(1-fade),finalY=yy
      if(this.notification.value>.01){const anchor=bodyRing[Math.round(7*bodyRing.length/8)%bodyRing.length],dx=xx-anchor.x,dy=finalY-anchor.y,length=Math.max(Math.hypot(dx,dy),1),nx=dx/length,ny=dy/length,needed=20*clamp(this.notification.value,0,1.4)+Math.hypot(halves[i]*sx*nx,EYE_HALF*sy*ny)+5;if(length<needed){xx+=nx*(needed-length);finalY+=ny*(needed-length)}}
      return {path:ringPath(ring),transform:`translate(${xx} ${finalY}) scale(${sx} ${sy}) translate(${-centre.x} ${-centre.y})`,visible:visible&&m<.5}
    })
    let badge=''
    if(this.notification.value>.01){const anchor=bodyRing[Math.round(7*bodyRing.length/8)%bodyRing.length];badge=`<circle cx="${anchor.x}" cy="${anchor.y}" r="${20*clamp(this.notification.value,0,1.4)}" stroke="currentColor" stroke-width="10" fill="#1d9bf0"/>`}
    if(this.humming.value>.01)for(let i=0;i<2;i++){const a=.85*this.spinAngle+i*Math.PI,r=1.3*body.radius,depth=.55+.45*clamp((Math.cos(a)+1)/2),size=7.5*depth*clamp(this.humming.value);fx.svg+=`<circle cx="${C+r*Math.sin(a)}" cy="${C-.38*r*Math.cos(a)-8}" r="${size}" fill="currentColor" opacity="${(.3+.7*depth)*clamp(this.humming.value)}"/>`}
    if(Math.abs(gesture.turn)>.001)this.particleSpinAngle=gesture.turn;else if(this.active==='humming'||this.active==='loading')this.particleSpinAngle=this.spinAngle
    const particles=this.particles.update(now,delta,this.particleSpinAngle,clamp((340/Math.max(viewWidth,1))**.7,1,2.6),this.active==='humming'||gesture.wild||this.gestures.wide,moving&&!this.settled&&(input.mood==='working'||input.mood==='fetching'||!!event),belt+(this.active==='loading'?(52-belt)*m:0),id)
    const expansion=(MORPH_VIEWBOXES[this.morph.active??'']??1)*blend+(MORPH_VIEWBOXES[previous??'']??MORPH_VIEWBOXES[this.morph.active??'']??1)*(1-blend),responsive=1-smoothstep(clamp((viewWidth-44)/90)),radius=129.5/(1+(expansion-1)*m*responsive),scale=morphSize/C*fx.scale
    return {state:this.active,body:this.bodyId,head:outline,position,morphAmount:m,transform:`translate(${C+position.x} ${C+position.y}) rotate(${position.degrees}) scale(${1-m+scale*m} ${this.sy.value*(1-m)+scale*m}) translate(${-C} ${-C})`,opacity:fx.opacity,eyes,shapes:fx.svg,badge,backParticles:particles.back,frontParticles:particles.front,particleCount:particles.count,viewBox:`${114.5-radius} ${114.5-radius} ${radius*2} ${radius*2}`}
  }
  get isBlinking(){return this.blinks.blinking}
}

const stills=new Map<string,MarkFrame>()
/** A memoised settled still: one authored signature pose, three simulated
 * seconds, no pointer or news, then wait out an in-flight blink. */
export function settledFrame(input:EngineInput,viewWidth=28):MarkFrame {
  const key=JSON.stringify([input.persona,input.mood,input.body,input.gaze,!!input.isPointedAt,!!input.isQuiet,viewWidth])
  const cached=stills.get(key);if(cached)return cached
  const quiet={...input,animated:true,pointer:null,finishedAt:0,resetAt:0},engine=new BotMarkEngine(quiet,Math.random,true)
  let frame=engine.advance(0,quiet,0,viewWidth)
  for(let i=0;i<180;i++)frame=engine.advance(1/60,quiet,0,viewWidth)
  for(let i=0;engine.isBlinking&&i<120;i++)frame=engine.advance(1/60,quiet,0,viewWidth)
  stills.set(key,frame);return frame
}

/** Original per-state glance ranges and cadences. A fifth may look outward. */
function aimForState(state:string,random:Random){
  const rand=(l:number,h:number)=>range(random,l,h),direction=()=>random()<.5?-1:1
  let x=0,y=0,lo=2500,hi=5000
  switch(state){
    case 'idle':lo=2500;hi=5500;break
    case 'listening':x=15*rand(-.3,.3);y=9*rand(-.25,.25);lo=2200;hi=4200;break
    case 'thinking':x=direction()*rand(.5,1)*15;y=-9*rand(.4,1);lo=1500;hi=2800;break
    case 'searching':x=direction()*rand(.7,1)*15;y=9*rand(-1,1);lo=550;hi=1150;break
    case 'working':x=15*rand(-.4,.4);y=9*rand(.4,1);lo=1200;hi=2400;break
    case 'excited':x=15*rand(-1,1);y=9*rand(-1,.3);lo=700;hi=1400;break
    case 'surprised':lo=1600;hi=2600;break
    case 'suspicious':x=15*direction();y=2.7;lo=2200;hi=4200;break
    case 'angry':x=15*rand(-.2,.2);y=1.8;lo=1800;hi=3200;break
    case 'drowsy':x=15*rand(-.4,.4);y=9*rand(.4,1);lo=2500;hi=4500;break
    case 'happy':x=15*rand(-.7,.7);y=-9*rand(0,.6);lo=1800;hi=3400;break
    case 'curious':x=direction()*rand(.6,1)*15;y=9*rand(-1,1);lo=950;hi=1900;break
    case 'confused':x=direction()*rand(.5,1)*15;y=9*rand(-.6,1);lo=1100;hi=2300;break
    case 'bored':x=direction()*rand(.7,1)*15;y=9*rand(.4,.9);lo=3000;hi=6000;break
    case 'proud':x=15*rand(-.3,.3);y=-9*rand(.3,.7);lo=2600;hi=4600;break
    case 'shy':x=direction()*rand(.6,1)*15;y=9*rand(.5,1);lo=2000;hi=4000;break
    case 'sad':x=15*rand(-.3,.3);y=9*rand(.6,1);lo=2800;hi=5000;break
    case 'laughing':x=15*rand(-.5,.5);y=-9*rand(.2,.6);lo=800;hi=1700;break
    case 'scared':x=direction()*rand(.7,1)*15;y=9*rand(-.6,.6);lo=450;hi=1050;break
    case 'playful':x=direction()*rand(.5,1)*15;y=-9*rand(0,.6);lo=900;hi=1800;break
    case 'notifying':{const focused=random()<.72;x=(focused?.45:.1)*15;y=-9*(focused?.3:.05);lo=1200;hi=2400;break}
    default:x=15*rand(-.4,.4);y=9*rand(-.3,.3)
  }
  return {x,y,lo,hi}
}
