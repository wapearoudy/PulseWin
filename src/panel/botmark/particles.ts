import { STAR_GOLD, STAR_PATH } from './data'
import { C, clamp, range, smoothstep, type Random } from './math'
type TrailPoint = { x: number; y: number; angle: number; z: number }
type Orbit = { angle:number; angularVelocity:number; tilt:number; roll:number; radius:number; radiusVelocity:number; follow:number; carry:number; arc:number }
type Particle = { x:number; y:number; vx:number; vy:number; returnAmount:number; life:number; maximum:number; radius:number; rotation:number; rotationSpeed:number; color:string; round:boolean; isStar:boolean; hue:number; hueSpan:number; hueVelocity:number; orbit?:Orbit; history:TrailPoint[] }
const colors=['#f9705c','#5b95f0','#3fbe86','#f5b13f','#9a72ee','#35c3bd']
export interface ParticleFrame { back:string; front:string; count:number }

/** A persistent port of BotMarkParticles: depth-split, tapered HSL ribbons,
 * emission queues, orbital return and the original confetti physics. */
export class Particles {
  private particles:Particle[]=[]
  private lastAngle:number
  private angularVelocity=0
  private trailActive=false
  private queue:{at:number;index:number}[]=[]
  private layouts:{tilt:number;roll:number}[]=[]
  private hue=0
  private orbitCount=4
  private beltRadius=C
  constructor(private random:Random=Math.random) {this.lastAngle=this.rand(0,Math.PI*2)}
  private rand(lo:number,hi:number) {return range(this.random,lo,hi)}
  private newParticle():Particle {return {x:0,y:0,vx:0,vy:0,returnAmount:0,life:0,maximum:1,radius:4,rotation:0,rotationSpeed:0,color:'#fff',round:true,isStar:false,hue:0,hueSpan:0,hueVelocity:0,history:[]}}
  private resetStyle(count=1) {
    const roll=this.rand(-.85,.85)
    this.layouts=Array.from({length:count},(_,i)=>({tilt:this.rand(.16,.5),roll:roll+i*Math.PI/count+this.rand(-.12,.12)}))
    this.orbitCount=count>1?3*count:Math.round(this.rand(3,5));this.hue=this.rand(0,360)
  }
  private spawn(angle:number,direction:number,index:number) {
    if(this.particles.length>110)return
    if(!this.layouts.length)this.resetStyle()
    const layout=this.layouts[index%this.layouts.length],perLayout=Math.max(Math.ceil(this.orbitCount/this.layouts.length)-1,1),p=this.newParticle()
    p.x=p.y=C;p.maximum=9
    p.radius=this.orbitCount<=3?this.rand(8,10.5):this.orbitCount===4?this.rand(6.6,8.6):this.rand(5.6,7.4)
    p.rotation=this.rand(0,360);p.rotationSpeed=this.rand(-240,240);p.color=colors[Math.floor(this.random()*colors.length)]
    p.hue=this.hue+360*index/Math.max(this.orbitCount,1)+this.rand(-14,14)
    p.hueSpan=this.rand(45,95)*(this.random()<.5?1:-1);p.hueVelocity=this.rand(18,42)*(this.random()<.5?1:-1)
    p.orbit={angle,angularVelocity:direction*this.rand(.5,1.1),tilt:layout.tilt+this.rand(-.04,.04),roll:layout.roll+this.rand(-.05,.05),radius:116*this.beltRadius/C+Math.floor(index/this.layouts.length)*38/perLayout+this.rand(-1.5,1.5),radiusVelocity:this.rand(0,2.5),follow:this.rand(.74,.94),carry:0,arc:this.rand(2.2,3.4)}
    this.particles.push(p)
  }
  burst(count=20,force=1,curl=0) {
    if(this.particles.length>120)return
    for(let i=0;i<count;i++) {
      const angle=i/count*2*Math.PI+this.rand(-.35,.35),distance=this.rand(96,116)*this.beltRadius/C,speed=this.rand(170,360)*force,curlVelocity=curl*speed*.2,p=this.newParticle()
      p.x=C+Math.cos(angle)*distance;p.y=C+Math.sin(angle)*distance
      p.vx=Math.cos(angle)*speed-Math.sin(angle)*curlVelocity;p.vy=Math.sin(angle)*speed+Math.cos(angle)*curlVelocity-this.rand(20,75)
      p.isStar=this.random()<.18;p.maximum=this.rand(.45,.85);p.radius=p.isStar?this.rand(4,7):this.rand(3.5,8)
      p.rotation=this.rand(0,360);p.rotationSpeed=this.rand(-260,260);p.round=!p.isStar&&this.random()<.3
      p.color=p.isStar?STAR_GOLD:colors[Math.floor(this.random()*colors.length)];this.particles.push(p)
    }
  }
  private project(o:Orbit,angle:number):TrailPoint {
    const h=o.radius*Math.sin(angle),v=-o.radius*Math.cos(angle)*Math.sin(o.tilt)
    return {x:C+h*Math.cos(o.roll)-v*Math.sin(o.roll),y:C+h*Math.sin(o.roll)+v*Math.cos(o.roll),angle,z:Math.cos(angle)*Math.cos(o.tilt)}
  }
  private paths(points:TrailPoint[],width:number) {
    if(points.length<2)return {front:'',back:''}
    let length=0
    for(let i=1;i<points.length;i++)length+=Math.hypot(points[i].x-points[i-1].x,points[i].y-points[i-1].y)
    if(length<2)return {front:'',back:''}
    const actualWidth=Math.min(width,.34*length),normals=points.map((_,i)=>{
      const a=points[Math.max(0,i-1)],b=points[Math.min(points.length-1,i+1)],dx=b.x-a.x,dy=b.y-a.y,magnitude=Math.hypot(dx,dy)||1,half=actualWidth*(.5+i/(points.length-1)*.5)/2
      return {x:-dy/magnitude*half,y:dx/magnitude*half}
    })
    const segment=(start:number,end:number)=>{
      let d=''
      for(let i=start;i<=end;i++)d+=`${i===start?'M':'L'} ${points[i].x+normals[i].x} ${points[i].y+normals[i].y} `
      if(end===points.length-1){const radius=Math.max(Math.hypot(normals[end].x,normals[end].y),.2);d+=`A ${radius} ${radius} 0 0 0 ${points[end].x-normals[end].x} ${points[end].y-normals[end].y} `}
      for(let i=end;i>=start;i--)d+=`L ${points[i].x-normals[i].x} ${points[i].y-normals[i].y} `
      if(start===0){const radius=Math.max(Math.hypot(normals[start].x,normals[start].y),.2);d+=`A ${radius} ${radius} 0 0 0 ${points[start].x+normals[start].x} ${points[start].y+normals[start].y} `}
      return d+'Z '
    }
    let front='',back='',cursor=0
    while(cursor<points.length){const isFront=points[cursor].z>=0;let end=cursor;while(end+1<points.length&&(points[end+1].z>=0)===isFront)end++;const start=Math.max(cursor-1,0),last=Math.min(end+1,points.length-1);if(last>start){const piece=segment(start,last);if(isFront)front+=piece;else back+=piece}cursor=end+1}
    return {front,back}
  }
  update(now:number,delta:number,spinAngle:number,sizeScale:number,wide:boolean,enabled:boolean,beltRadius:number,id:string):ParticleFrame {
    this.beltRadius=beltRadius
    if(!enabled){this.particles=[];this.queue=[];this.trailActive=false;this.angularVelocity=0;this.lastAngle=spinAngle;return {back:'',front:'',count:0}}
    let difference=spinAngle-this.lastAngle;if(!Number.isFinite(difference)||Math.abs(difference)>1.2)difference=0
    this.lastAngle=spinAngle
    const wasSpinning=Math.abs(this.angularVelocity)>=.9
    this.angularVelocity=delta>0?difference/delta:0
    const spinning=Math.abs(this.angularVelocity)>=.9
    if(!wasSpinning&&spinning){this.resetStyle(wide?3:1);this.trailActive=false}
    if(wasSpinning&&!spinning)this.queue=[]
    if(!this.trailActive&&Math.abs(this.angularVelocity)>=5){this.trailActive=true;this.queue=Array.from({length:this.orbitCount},(_,i)=>({at:now+i*this.rand(55,105),index:i}))}
    while(this.queue.length&&now>=this.queue[0].at){const shot=this.queue.shift()!;this.spawn(spinAngle-this.rand(0,.18),this.angularVelocity<0?-1:1,shot.index)}
    let back='',front='';const alive:Particle[]=[]
    for(const p of this.particles){
      p.life+=delta;const progress=clamp(p.life/p.maximum)
      if(p.orbit){p.returnAmount=clamp(p.returnAmount+(!spinning||progress>.55?delta/.5:-delta/.35));if(p.returnAmount>=1)continue}else if(p.life>=p.maximum)continue
      const opacity=p.orbit?Math.min(1,p.life/.26):progress<.1?progress/.1:(1-(progress-.1)/.9)**1.7
      if(p.orbit){
        const o=p.orbit
        if(spinning){o.carry=this.angularVelocity*o.follow;o.angle+=this.angularVelocity*delta*o.follow+o.angularVelocity*delta}else{o.angle+=(o.carry+o.angularVelocity)*delta;o.carry*=Math.exp(-2.6*delta);o.angularVelocity*=Math.exp(-2.6*delta)}
        o.radius+=o.radiusVelocity*delta;const point=this.project(o,o.angle);p.x=point.x;p.y=point.y
        const depthScale=.72+.28*clamp(point.z),enter=Math.min(p.life/.34,1),width=Math.max(p.radius*depthScale*1.7*sizeScale*smoothstep(enter)*(1-.72*p.returnAmount*p.returnAmount),.5)
        const previous=p.history[p.history.length-1]?.angle??o.angle,change=o.angle-previous,subdivisions=Math.min(Math.ceil(Math.abs(change)/.09),24)
        for(let i=1;i<=subdivisions;i++)p.history.push(this.project(o,previous+change*i/subdivisions))
        if(!p.history.length)p.history.push(point)
        const arc=o.arc*(1-smoothstep(p.returnAmount))
        while(p.history.length>2&&Math.abs(o.angle-p.history[0].angle)>arc)p.history.shift()
        const excess=Math.abs(o.angle-p.history[0].angle)-arc
        if(p.history.length>=2&&excess>0)p.history[0]=this.project(o,p.history[0].angle+(o.angle-p.history[0].angle<0?-1:1)*excess)
        if(p.history.length>48)p.history.splice(0,p.history.length-48)
        if(p.history.length>=2){
          const paths=this.paths(p.history,width),from=p.history[0],to=p.history[p.history.length-1],name=`${id}-ribbon-${alive.length}`
          const stops=Array.from({length:5},(_,i)=>`<stop offset="${i/4}" stop-color="hsl(${p.hue+p.hueVelocity*p.life+i/4*p.hueSpan} 56% ${56+11*i/4}%)"/>`).join('')
          const paint=`<defs><linearGradient id="${name}" gradientUnits="userSpaceOnUse" x1="${from.x}" y1="${from.y}" x2="${to.x}" y2="${to.y}">${stops}</linearGradient></defs>`
          if(paths.back)back+=paint+`<path d="${paths.back}" fill="url(#${name})" opacity="${opacity}"/>`
          if(paths.front)front+=(!paths.back?paint:'')+`<path d="${paths.front}" fill="url(#${name})" opacity="${opacity}"/>`
        }
      } else {
        p.x+=p.vx*delta;p.y+=p.vy*delta;const drag=.94**(60*delta);p.vx*=drag;p.vy=p.vy*drag+40*delta
        const size=Math.max(p.radius*(1-.4*progress),.5)
        if(p.isStar){p.rotation+=p.rotationSpeed*delta;back+=`<path d="${STAR_PATH}" transform="translate(${p.x} ${p.y}) rotate(${p.rotation}) scale(${size})" fill="${p.color}" opacity="${opacity}"/>`}
        else if(p.round)back+=`<circle cx="${p.x}" cy="${p.y}" r="${size}" fill="${p.color}" opacity="${opacity}"/>`
        else{const width=Math.max(2*size,Math.min(.05*Math.hypot(p.vx,p.vy),30)),height=1.5*size;back+=`<rect x="${p.x-width/2}" y="${p.y-height/2}" width="${width}" height="${height}" rx="${height/2}" transform="rotate(${Math.atan2(p.vy,p.vx)*180/Math.PI} ${p.x} ${p.y})" fill="${p.color}" opacity="${opacity}"/>`}
      }
      alive.push(p)
    }
    this.particles=alive;return {back,front,count:alive.length}
  }
}
