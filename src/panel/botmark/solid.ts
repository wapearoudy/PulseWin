// Pulse BotMarkGeometry. Yaw changes the silhouette of solids, not a CSS flip.
import type { BotMarkShape } from './data'
import type { Point } from './geometry'
const c=114.2705, clamp=(v:number,l:number,h:number)=>Math.max(l,Math.min(h,v))
const smooth=(a:number[])=>a.map((_,i)=>(a[(i+94)%96]+4*a[(i+95)%96]+6*a[i]+4*a[(i+1)%96]+a[(i+2)%96])/16)
const baselines=new Map<string,number[]>()
function profile(solid:number[][],angle:number) {
  return smooth(Array.from({length:96},(_,i)=>{
    const dx=Math.cos(i/96*Math.PI*2),dy=Math.sin(i/96*Math.PI*2)
    return solid.reduce((radius,[x,y,z,r])=>{const rx=x*Math.cos(angle)+z*Math.sin(angle),p=dx*rx+dy*y,d=p*p-rx*rx-y*y+r*r;return d>0?Math.max(radius,p+Math.sqrt(d)):radius},0)
  }))
}
export function turnedRing(shape:BotMarkShape, angle:number):Point[] {
  if(shape.solid) {
    if(!baselines.has(shape.id))baselines.set(shape.id,profile(shape.solid,0))
    const baseline=baselines.get(shape.id)!
    let ratios=profile(shape.solid,angle).map((r,i)=>clamp((r+12)/(baseline[i]+12),.32,1.5))
    for(let i=0;i<3;i++)ratios=smooth(ratios)
    return shape.ring.map((p,i)=>({x:c+(p.x-c)*ratios[i],y:c+(p.y-c)*ratios[i]}))
  }
  const segment=2*Math.PI/shape.sides, phase=((angle%segment)+segment)%segment
  const factor=shape.sides>0?1+(Math.cos(phase-segment/2)/Math.cos(segment/2)-1)*.45:1
  return shape.ring.map(p=>({x:c+(p.x-c)*factor,y:p.y}))
}
export function spanAt(ring:Point[],y:number):[number,number] {
  let left=-Infinity,right=Infinity
  ring.forEach((a,i)=>{const b=ring[(i+1)%ring.length];if((a.y<=y)===(b.y<=y))return;const x=a.x+(b.x-a.x)*(y-a.y)/(b.y-a.y);if(x<=c)left=Math.max(left,x);else right=Math.min(right,x)})
  return [Number.isFinite(left)?left:c,Number.isFinite(right)?right:c]
}
