import { Spring } from './programme'
import { cubicInOut, range, type Random } from './math'
export interface GesturePose {turn:number;rotation:number;x:number;y:number;bounceY:number;gazeX:number;gazeY:number;eyeOpen?:number;eyeScale?:number;wild:boolean}
export const blankGesture=():GesturePose=>({turn:0,rotation:0,x:0,y:0,bounceY:0,gazeX:0,gazeY:0,wild:false})
/** The full reset celebration: wind-up, nine yaw turns / three body turns,
 * deceleration and a dizzy shake, within the original 6.2-second cycle. */
export function celebratePose(elapsed:number,direction=1):GesturePose {
  const o=blankGesture(),active=elapsed/1000-.14
  if(active<0)return o
  const cycle=active%6.2;if(cycle>5.49)return o
  o.wild=true
  const turns=9,speed=(turns*2*Math.PI+.5)/(.15+2+.3125)
  let angle=turns*2*Math.PI
  if(cycle<.24)angle=-.25*(1-Math.cos(cycle/.24*Math.PI))
  else if(cycle<.54){const t=cycle-.24;angle=-.5+speed*t*t/.6}
  else if(cycle<2.54)angle=-.5+speed*(.15+cycle-.54)
  else if(cycle<3.79)angle=-.5+speed*2.15+1.25*speed*(1-(1-(cycle-2.54)/1.25)**4)/4
  o.turn=angle*direction
  let envelope=0
  if(cycle>2.54){const progress=Math.min((cycle-2.54)/1.25,1);envelope=progress<.4?0:((progress-.4)/.6)**2;if(cycle>=3.79)envelope=Math.max(0,(1-(cycle-3.79)/1.7)**1.6)}
  const shake=Math.max(cycle-2.54,0)
  o.rotation=angle/(turns*2*Math.PI)*1080*direction+11*Math.sin(9.2*shake)*direction*envelope
  o.x=(Math.cos(9.2*shake)-1)*6*direction*envelope;o.y=2.6*Math.sin(18.4*shake)*envelope
  o.gazeX=13*Math.sin(11.5*shake)*direction*envelope;o.gazeY=(Math.cos(9*shake)-1)*3.5*envelope
  o.eyeOpen=1.14-.44*envelope+.1*Math.sin(16*shake)*envelope;o.eyeScale=1.12-.09*envelope
  return o
}

export class Gestures {
  private gesture:{kind:string;started:number;direction:number;turns:number}|null=null
  private spin:Spring|null=null
  private behaviorNext=0
  private ambientNext:number
  private winkNext=0
  winkAt=-Infinity;winkEye=0
  private bounceStarted=-1
  private direction=1
  private shapeCycle:number
  wide=false
  constructor(private random:Random=Math.random){this.ambientNext=this.rand(2500,5000);this.shapeCycle=Math.floor(this.rand(0,5))}
  private rand(lo:number,hi:number){return range(this.random,lo,hi)}
  enter(state:string,now:number){
    this.behaviorNext=now+(state==='excited'?this.rand(400,1100):state==='searching'?this.rand(800,1600):state==='working'||state==='playful'?this.rand(1200,2400):this.rand(6000,10000))
    this.winkNext=now+this.rand(3000,8000)
    if(state==='celebrate')this.direction=this.random()<.5?1:-1
  }
  private startSpin(turns=1,direction=this.random()<.5?1:-1){if(this.spin)return false;this.spin=new Spring(0);this.spin.target=turns*2*Math.PI*direction;return true}
  private startGesture(kind:string,now:number){if(this.gesture||this.spin)return;this.gesture={kind,started:now,direction:this.random()<.5?1:-1,turns:kind==='spinDizzy'?Math.round(this.rand(3,4)):1}}
  private bounce(now:number){if(this.bounceStarted<0)this.bounceStarted=now}
  shapeChanged(now:number){this.shapeCycle=(this.shapeCycle+1)%5;switch(this.shapeCycle){case 0:this.startSpin();break;case 1:this.wide=this.startSpin(2);break;case 2:this.startGesture('spinBounce',now);break;case 3:this.startGesture('spinDizzy',now);break;default:this.startSpin();return true}return false}
  update(state:string,now:number,elapsed:number,delta:number,tempo:number):GesturePose {
    let out=blankGesture()
    if(['idle','happy','excited','curious','playful'].includes(state)&&now>=this.winkNext){this.winkAt=now;this.winkEye=this.random()<.5?0:1;this.winkNext=now+this.rand(4500,10000)*tempo}
    if(now>=this.behaviorNext&&!this.gesture&&!this.spin){switch(state){case 'searching':this.startSpin();this.behaviorNext=now+this.rand(4000,7000)*tempo;break;case 'working':this.startSpin(1,1);this.behaviorNext=now+this.rand(6000,9000)*tempo;break;case 'excited':this.startSpin();this.behaviorNext=now+this.rand(2800,5000)*tempo;break;case 'playful':this.startSpin();this.behaviorNext=now+this.rand(3500,6000)*tempo;break}}
    if(now>=this.ambientNext){if(!this.gesture&&!this.spin){const roll=this.random();if(['happy','excited','proud'].includes(state)){if(roll<.55)this.startSpin();else this.startGesture('spinBounce',now)}else if(state==='playful'){if(roll<.34)this.startGesture('spinBounce',now);else if(roll<.62)this.bounce(now);else if(roll<.86)this.startGesture('spinDizzy',now);else this.startSpin()}}this.ambientNext=now+this.rand(9000,18000)}
    if(this.gesture){const g=this.gesture,e=(now-g.started)/1000
      if(g.kind==='spinBounce'){if(e<.7)out.turn=g.turns*2*Math.PI*g.direction*cubicInOut(e/.7);else{this.bounce(now);this.gesture=null}}
      else {const spinDuration=.55+.16*g.turns;if(e<spinDuration)out.turn=g.turns*2*Math.PI*g.direction*(e/spinDuration)**2;else if(e<spinDuration+1.5){const shake=e-spinDuration,envelope=(1-shake/1.5)**1.3;out.rotation=17*Math.sin(10*shake)*g.direction*envelope;out.x=10*Math.cos(10*shake)*g.direction*envelope;out.y=3*Math.sin(20*shake)*envelope;out.eyeOpen=.46+.14*Math.sin(21*shake);out.eyeScale=1.03}else this.gesture=null}
    }
    if(this.bounceStarted>=0){const sequence=[[48,.5],[28,.382],[14,.27],[6,.177]];let e=(now-this.bounceStarted)/1000,i=0;while(i<sequence.length&&e>=sequence[i][1]){e-=sequence[i][1];i++}if(i>=sequence.length)this.bounceStarted=-1;else{const p=e/sequence[i][1];out.bounceY=-4*sequence[i][0]*p*(1-p)}}
    if(state==='celebrate')out=celebratePose(elapsed,this.direction)
    if(this.spin){this.spin.step(6.2,1,delta);out.turn+=this.spin.value;if(Math.abs(this.spin.target-this.spin.value)<.004&&Math.abs(this.spin.velocity)<.015){this.spin=null;this.wide=false}}
    return out
  }
  wink(now:number,index:number){if(index!==this.winkEye||now>=this.winkAt+320)return 1;const p=(now-this.winkAt)/320;return Math.max(p<.42?1-p/.42:(p-.42)/.58,.04)}
}
