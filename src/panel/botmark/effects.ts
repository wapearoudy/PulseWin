import { C, clamp, cubicInOut, cubicOut, backOut, smoothstep, unit, range, type Random } from './math'
import type { Point } from './geometry'
export const MORPH_SIZES:Record<string,number>={dots:22,orbit:19,radar:19,progress:19,gather:19,wave:16,send:20,receive:20,dock:20,ball:18,whirl:15,pencil:17,bang:13,standby:13}
export const MORPH_VIEWBOXES:Record<string,number>={dots:1.5,orbit:1.14,radar:1.14,progress:1.32,gather:1.15,wave:1.42,send:1.12,receive:1.12,dock:1.3,ball:1.22,whirl:1.45,pencil:1.18,bang:1.28,standby:1.75}
export class EffectMemory {receiveCycle=-1;receiveAngle=-.7;writingTrail:Point[]=[];constructor(public random:Random=Math.random){}}
export interface EffectFrame {svg:string;x:number;y:number;r:number;scale:number;opacity:number}
const circle=(x:number,y:number,r:number,opacity=1,stroke=0)=>`<circle cx="${x}" cy="${y}" r="${Math.max(.01,r)}" opacity="${clamp(opacity)}" ${stroke?`fill="none" stroke="currentColor" stroke-width="${stroke}"`:'fill="currentColor"'}/>`
/** BotMarkMorphs drawing formulas, with persistent receive direction and nib
 * trail. Amount is the effect's contribution, not a global opacity switch. */
export function effects(effect:string|null,now:number,elapsed:number,amount:number,memory=new EffectMemory(),morphAmount=amount,morphElapsed=elapsed,shotElapsed=elapsed,baseRadius=MORPH_SIZES[effect??'']??19):EffectFrame {
  let svg='',x=0,y=0,r=0,scale=1,opacity=1
  const dot=(dx:number,dy:number,size:number,alpha=1,stroke=0)=>{svg+=circle(C+dx,C+dy,size,alpha,stroke)}
  const grow=cubicOut(amount)
  if(!effect||amount<=.004)return {svg,x,y,r,scale,opacity}
  switch(effect){
    case 'dots':{
      const raw=unit(morphElapsed/1400+.119)
      for(let i=0;i<2;i++){const phase=clamp((amount-.12*i)/(1-.12*i));if(phase<=.004)continue;const d=Math.abs(raw-i*2/3),pulse=Math.exp(-(Math.min(d,1-d)**2)/.045),g=cubicOut(phase);dot((i===0?-62:62)*backOut(phase),-9*pulse*amount,22*g*(.84+.22*pulse)*1.02,g*(1-.5*(1-pulse)))}
      const d=Math.abs(raw-1/3),pulse=Math.exp(-(Math.min(d,1-d)**2)/.045)
      scale*=1+(.84+.22*pulse-1)*amount/Math.max(morphAmount,.001);y-=9*pulse*amount*morphAmount;opacity*=1-.5*(1-pulse)*amount;break
    }
    case 'orbit':for(let i=0;i<5;i++){const phase=.0017*now+i*2*Math.PI/5,cosine=Math.cos(phase),depth=.5+.5*clamp(cosine),radius=52*backOut(amount);dot(radius*Math.sin(phase),-.42*radius*cosine,Math.max(12*depth*grow,.3),clamp((cosine+.4)/.6,.18,1)*grow)}break
    case 'radar':for(let i=0;i<3;i++){const phase=unit(now/1300+i/3);dot(0,0,baseRadius+(104-baseRadius)*phase,grow*(1-phase)*.9,3.4*(1-.55*phase))}break
    case 'progress':{const radius=62*backOut(amount),p=clamp(shotElapsed/2500/.85);dot(0,0,radius,.16*grow,5);if(p>0)svg+=`<circle cx="${C}" cy="${C}" r="${Math.max(.01,radius)}" fill="none" stroke="currentColor" stroke-width="5" opacity="${grow}" stroke-dasharray="${p*2*Math.PI*radius} ${2*Math.PI*radius}" transform="rotate(-90 ${C} ${C})"/>`;break}
    case 'gather':for(let i=0;i<5;i++){const p=clamp((shotElapsed/2000-.09*i)/.62);if(p>=1)continue;const settle=cubicOut(p),angle=2.4*i+2.2*p,radius=96*(1-settle);dot(radius*Math.cos(angle),radius*Math.sin(angle)*.8,9*(.5+.5*settle)*grow,grow*clamp(5*p)*(1-.25*settle))}break
    case 'wave':for(const offset of [-2,-1,1,2]){const phase=clamp((amount-.1*Math.abs(offset))/(1-.1*Math.abs(offset)));if(phase<=.004)continue;const energy=(.42+.29*Math.sin(.0021*now)*Math.sin(.0034*now)+.29*Math.sin(.0013*now+1.7))*(.55+.45*Math.sin(.012*now-1.05*Math.abs(offset)));dot(44*offset*backOut(phase),-6*clamp(energy)*phase,(7+9*clamp(energy,.08,1))*cubicOut(phase),phase)}break
    case 'receive':{
      const cycle=Math.floor(elapsed/1700);if(cycle!==memory.receiveCycle){memory.receiveCycle=cycle;memory.receiveAngle=range(memory.random,-1.25*Math.PI,.25*Math.PI)}
      const p=unit(elapsed/1700),travel=clamp(p/.6),e=cubicOut(travel),radius=108*(1-e),a=memory.receiveAngle,orbit=18*Math.sin(travel*Math.PI)*(1-.7*e)
      if(travel<1)dot(Math.cos(a)*radius-Math.sin(a)*orbit,Math.sin(a)*radius+Math.cos(a)*orbit,3.5+6.5*e,grow*clamp(3.5*travel)*(.3+.7*e))
      const ring=clamp((p-.58)/.32);if(ring>0&&ring<1)dot(0,0,20+26*cubicOut(ring),grow*(1-ring)*.8,2.8*(1-ring))
      scale*=1+.11*Math.sin(clamp((p-.58)/.34)*Math.PI)*amount;break
    }
    case 'send':{
      const p=unit(elapsed/1500),t=clamp((p-.18)/.55),e=t*t*(.4+.6*t)
      if(t>0&&t<1)dot(.74*108*e,-.62*108*e,10*(1-.55*e)*grow,grow*(1-e*e))
      const second=clamp((p-.26)/.55),se=second*second*(.4+.6*second)
      if(t>0&&second>0&&second<1)dot(.74*108*se,-.62*108*se,5*(1-.6*se)*grow,.3*grow*(1-se))
      const ring=clamp((p-.18)/.3);if(ring>0&&ring<1)dot(0,0,20+34*cubicOut(ring),grow*(1-ring)*.8,2.8*(1-ring))
      const bump=p<.18?-.06*Math.sin(p/.18*Math.PI):p<.42?.05*Math.sin((p-.18)/.24*Math.PI):0;scale*=1+bump*amount;break
    }
    case 'dock':for(let i=0;i<2;i++){const p=clamp((elapsed/1000-(.2+1.3*i))/.9);if(p<=0)continue;const e=cubicOut(p),a=.0011*now+i*Math.PI;dot((-120+30*i)*(1-e)+42*Math.sin(a)*e,95*(1-e)+(21*Math.cos(a)+2*Math.sin(.003*now+i))*e,(7+3*e)*grow,grow*clamp(4*p))}break
    case 'ball':{const g=416/.3844,fall=Math.sqrt(80/g),seconds=elapsed/1000,p=unit((seconds-fall)/.62),height=seconds<fall?40-.5*g*seconds*seconds:208*p*(1-p);y+=(40-height)*amount*morphAmount;break}
    case 'whirl':x+=(2*Math.sin(.0009*now)+.8*Math.sin(.0017*now))*amount*morphAmount;y+=(2.4*Math.sin(.0013*now)+1.2*Math.sin(.0006*now))*amount*morphAmount;break
    case 'pencil':{
      const cycle=unit(elapsed/2500),lift=cycle>=.68;let px=0,py=0,wiggle=0,rotation=0
      if(!lift){const p=cycle/.68,envelope=clamp(p/.08)*clamp((1-p)/.08);px=-54+smoothstep(p)*118;py=26;wiggle=3.2*Math.sin(24*p)*envelope;rotation=17+Math.sin(.0006*elapsed)}
      else{const p=cubicInOut((cycle-.68)/.32);px=64-118*p;py=26-20*Math.sin(p*Math.PI);rotation=17-2*Math.sin(p*Math.PI)+Math.sin(.0006*elapsed)}
      const angle=(rotation-90)*Math.PI/180,ox=68*Math.cos(angle),oy=68*Math.sin(angle)
      svg+=`<rect x="${C-15}" y="${C-44}" width="30" height="88" rx="15" fill="currentColor" opacity="${clamp(1.6*amount-.3)}" transform="translate(${C+(px+ox)*amount} ${C+(py+.15*wiggle+oy)*amount}) rotate(${rotation*amount}) scale(${grow}) translate(${-C} ${-C})"/>`
      if(amount>.6&&!lift){const point={x:C+px,y:C+py+wiggle+19},last=memory.writingTrail[memory.writingTrail.length-1];if(last&&Math.hypot(point.x-last.x,point.y-last.y)<=2.4)memory.writingTrail[memory.writingTrail.length-1]=point;else{memory.writingTrail.push(point);if(memory.writingTrail.length>64)memory.writingTrail.shift()}}
      else memory.writingTrail.splice(0,Math.min(2,memory.writingTrail.length))
      const points=memory.writingTrail
      if(points.length>=2){let d=`M ${points[0].x} ${points[0].y}`;for(let i=0;i<points.length-1;i++){const prev=points[Math.max(i-1,0)],a=points[i],b=points[i+1],after=points[Math.min(i+2,points.length-1)];d+=` C ${a.x+(b.x-prev.x)/6} ${a.y+(b.y-prev.y)/6} ${b.x-(after.x-a.x)/6} ${b.y-(after.y-a.y)/6} ${b.x} ${b.y}`};svg+=`<path d="${d}" fill="none" stroke="currentColor" stroke-width="6" opacity="${clamp(1.2*amount)}"/>`}
      x+=px*amount*morphAmount;y+=(py+.5*wiggle)*amount*morphAmount;r+=rotation*amount*morphAmount;break
    }
    case 'bang':{const enter=cubicOut(clamp(1.1*amount)),seconds=elapsed/1000,shake=2.2*Math.sin(42*seconds)*Math.exp(-(seconds%2.2)*5.5);svg+=`<path d="M ${C-15} ${C-33} A 15 15 0 0 1 ${C+15} ${C-33} L ${C+8.5} ${C+39.5} A 8.5 8.5 0 0 1 ${C-8.5} ${C+39.5} Z" fill="currentColor" opacity="${clamp(1.5*amount-.2)}" transform="translate(0 ${-26-(1-enter)*70}) rotate(${shake} ${C} ${C-74}) translate(${C} ${C}) scale(${clamp(1.2*amount)}) translate(${-C} ${-C})"/>`;y+=58*amount*morphAmount;scale*=1+.04*Math.exp(-(seconds%2.2)*5.5)*amount;break}
    case 'standby':{const pulse=.5+.5*Math.sin(.0016*now);dot(0,0,26+7*pulse,grow*(.06+.1*pulse));if(amount<.995)dot(0,0,104-88*grow,(1-grow)*.5,2.4);opacity*=1-(.28+.2*Math.sin(.0016*now))*amount;break}
  }
  return {svg,x,y,r,scale,opacity}
}
