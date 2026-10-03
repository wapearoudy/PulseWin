// Per-state target poses from Pulse's BotMarkEngine.updateStateTargets.
// Springs in the renderer keep changes of mood continuous.
const sin=Math.sin, abs=Math.abs, pi=Math.PI
export function pose(id:string, now:number, elapsed:number) {
  const t=now/1000,e=elapsed/1000
  let r=0,x=0,y=0,sy=1,eyes=1,open=1
  switch(id) {
    case 'idle':r=1.5*sin(.5*t)+.6*sin(.17*t);x=sin(.27*t);y=1.2*sin(.85*t);sy=1+.007*sin(.85*t);break
    case 'listening':r=8+1.5*sin(.5*t);x=2;y=-2+.8*sin(.8*t);sy=1.015;break
    case 'thinking':r=-9+5*sin(.35*t);x=5*sin(.3*t);y=2.5*sin(.6*t);break
    case 'searching':r=13*sin(1.3*t);x=7*sin(1.3*t);y=3*sin(1.7*t);break
    case 'working':r=4+2.5*sin(t*pi*3.2);x=3;y=1.5+3*Math.max(0,sin(t*pi*3.2));sy=1-.02*Math.max(0,sin(t*pi*3.2));break
    case 'excited':{const p=(2.2*t)%1;y=-10*sin(p*pi)+2;sy=p<.1?.92:p<.3?1.05:1;x=4*sin(1.1*t);eyes=1.06;r=7*sin(t*pi*2.2);break}
    case 'surprised':{const settle=Math.min(e/1.2,1);x=-4*(1-settle);y=-8*(1-settle);sy=e<.2?1.08:1;eyes=1.15-.08*settle;r=1.5*sin(11*t)*(1-settle);break}
    case 'suspicious':r=-6+3*sin(.3*t);x=-4*sin(.25*t);y=1+1.2*sin(.45*t);open=.85;break
    case 'angry':r=(e%2.5)<.42?4.5*sin(.05*now):0;y=3.5;sy=.975;break
    case 'drowsy':r=2.5*sin(.32*t);x=1.5*sin(.2*t);y=6+2.2*sin(.36*t);sy=1+.022*sin(.36*t);open=.34+.07*sin(.8*t);break
    case 'happy':r=3*sin(1.2*t);x=2.5*sin(1.1*t);y=-3*abs(sin(2.4*t));sy=1+.02*sin(2.4*t);eyes=1.05;break
    case 'curious':r=10+6*sin(.7*t);x=5*sin(.6*t);y=-2+1.5*sin(.9*t);sy=1.01;eyes=1.08;break
    case 'confused':r=12*sin(.8*t);x=3*sin(.8*t);y=2*sin(.5*t);open=.9;break
    case 'bored':r=-3+4*sin(.25*t);x=4*sin(.2*t);y=5+1.5*sin(.35*t);sy=.99;open=.6;eyes=.98;break
    case 'proud':r=2.5*sin(.4*t);x=2*sin(.35*t);y=-4+sin(.6*t);sy=1.03;eyes=1.02;open=.9;break
    case 'shy':r=-8+3*sin(.5*t);x=-3+2*sin(.4*t);y=3;sy=.98;eyes=.95;open=.85;break
    case 'sad':r=3+2*sin(.3*t);x=1.5*sin(.25*t);y=7+sin(.4*t);sy=.97;eyes=.97;open=.7;break
    case 'laughing':r=4*sin(t*pi*6.4);x=2*sin(2*t);y=-5*abs(sin(t*pi*6.4));sy=1+.03*sin(t*pi*6.4);open=.7;break
    case 'scared':r=2*sin(.04*now);x=-2+1.5*sin(.05*now);y=2+sin(1.5*t);sy=.97;eyes=1.12;open=1.05;break
    case 'playful':r=8*sin(1.4*t);x=4*sin(1.1*t);y=-3*abs(sin(2.2*t));sy=1+.015*sin(2.2*t);eyes=1.06;break
    case 'humming':r=2*sin(.4*t);x=1.5*sin(.3*t);y=1.5*sin(.7*t);break
    case 'celebrate':y=-2.5*abs(sin(1.6*t));eyes=1.1;open=1.1;break
    case 'notifying':eyes=1+.05*Math.exp(-3*e);r=3;x=2;y=-1;break
  }
  return {r,x,y,sy,eyes,open}
}

import { cubicInOut, clamp, range, type Random } from './math'
/** Timed nods, sleep/wake, impulses and drag beats from updateStateTargets.
 * These timers belong to the mark, not to the component's render count. */
export class StateMotion {
  private nodUntil=0
  private nodNext=0
  private impulseNext=0
  private impulseUntil=0
  private drowsyStarted=0
  private wakeBurst=false
  private dragCycle=-1
  private notified=false
  constructor(private random:Random=Math.random){}
  enter(now:number){this.nodNext=now+range(this.random,1200,2200);this.impulseNext=now+range(this.random,500,1200);this.drowsyStarted=0;this.wakeBurst=false;this.dragCycle=-1;this.notified=false}
  update(id:string,now:number,elapsed:number,expression:number,expressionAmount:number,eyelid:number,expressionPool:number[]){
    const target=pose(id,now,elapsed),e=elapsed/1000,t=now/1000
    let rotationImpulse=0,yImpulse=0,burst=0,expressionTarget:number|undefined,blink=false
    switch(id){
      case 'sleeping':{
        if(expressionPool.includes(expression))target.open=expressionAmount>.85?1:.08
        else if(e<1.2)target.open=Math.max(.08,1-Math.min(1,e)*(1+.15*sin(6.5*e)))
        else{target.open=.08;if(eyelid<.18)expressionTarget=13}
        const settle=Math.min(e/2,1),dip=sin(clamp(e/.5)*pi);target.r=4*settle+2*sin(.25*t);target.x=-2*settle;target.y=8*settle+3*sin(.55*t)-5*dip;target.sy=1+.016*sin(.55*t)+.05*dip;break
      }
      case 'waking':
        if(e<.5){target.open=.07;expressionTarget=3;target.y=6}
        else if(e<1.2){target.open=1;target.eyes=1.12;target.y=-5;target.sy=1.04;if(!this.wakeBurst){burst=Math.round(range(this.random,9,13));this.wakeBurst=true}}
        else if(e<2.2){blink=e<1.4;expressionTarget=0}
        else{const settle=Math.min((e-2.2)/.8,1);target.r=6*sin(settle*pi*3)*(1-settle);target.y=2*sin(.9*t)}break
      case 'listening':
        if(now>=this.nodNext){this.nodUntil=now+380;this.nodNext=now+range(this.random,1800,3200)}
        if(now<this.nodUntil){const p=1-(this.nodUntil-now)/380;target.y+=4.5*sin(p*pi);target.r+=2*sin(p*pi)}break
      case 'curious':
        if(now>=this.nodNext){this.nodUntil=now+440;this.nodNext=now+range(this.random,1600,2800)}
        if(now<this.nodUntil){const p=1-(this.nodUntil-now)/440;target.x+=8*sin(p*pi);target.r+=5*sin(p*pi)}break
      case 'suspicious':if(now>=this.impulseNext){rotationImpulse=30;this.impulseNext=now+range(this.random,4000,7000)}break
      case 'confused':if(now>=this.impulseNext){rotationImpulse=22;this.impulseNext=now+range(this.random,2600,4200)}break
      case 'angry':
        if(now>=this.impulseNext){this.impulseUntil=now+420;yImpulse=70;this.impulseNext=now+range(this.random,1800,3200)}
        target.r=now<this.impulseUntil?4.5*sin(.05*now):0;break
      case 'bored':
        if(now>=this.impulseNext){this.impulseUntil=now+600;this.impulseNext=now+range(this.random,4000,7000)}
        if(now<this.impulseUntil){const p=1-(this.impulseUntil-now)/600;target.sy=1+.05*sin(p*pi);target.y+=3*sin(p*pi)}break
      case 'drowsy':
        if(now>=this.nodNext&&!this.drowsyStarted)this.drowsyStarted=now
        if(this.drowsyStarted){const p=(now-this.drowsyStarted)/1000
          if(p<1.7){const progress=p/1.7,squared=progress*progress;target.y=6+19*squared+2.2*sin(progress*pi*2.5)*(1-progress);target.r=10*squared;target.open=.34-squared*.3;target.sy=1-.045*squared}
          else if(p<2){const rebound=sin((p-1.7)/.3*pi);target.y=25-7*rebound;target.r=10-4*rebound;target.open=.04+.42*rebound}
          else if(p<3.5){const progress=(p-2)/1.5,recovery=1-(1-progress)**2.2;target.y=25-19*recovery;target.r=10*(1-recovery);target.open=.46-.12*recovery;if(progress>.32&&progress<.46)target.open=.05}
          else{this.drowsyStarted=0;this.nodNext=now+range(this.random,1500,3500)}
        }break
      case 'dragging':{
        const p=(e%3.4)/3.4,cycle=Math.floor(e/3.4)
        if(p<.12){target.x=-16;target.y=-22;target.r=-5}
        else if(p<.62){target.x=-16+32*cubicInOut((p-.12)/.5);target.y=-22+2*sin(1.4*t);target.r=6*sin(2.6*t);target.eyes=1.06}
        else{if(cycle!==this.dragCycle){this.dragCycle=cycle;yImpulse=90}target.x=16}break
      }
      case 'notifying':if(!this.notified&&e>.12){this.notified=true;yImpulse=-26;blink=true}break
    }
    return {target,rotationImpulse,yImpulse,burst,expressionTarget,blink}
  }
}
