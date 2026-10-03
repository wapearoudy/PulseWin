import type { HitRegion } from '../api'
import { berthPath, type DockLayout, type PanelEdge } from './berthShape'
import { bubblePath } from './bubbleShape'
import type { DetailCardLayout } from './cardLayout'

/** Absolute CSS coordinates, matching the native cursor's scale conversion. */
export type HitPoint = [number, number]
export interface ShapeHitRegion extends HitRegion {
  /** Closed SVG subpaths. The native receiver uses the SVG nonzero fill rule. */
  outline?: HitPoint[][]
  /** A slot can retain its rectangle while being clipped to the painted rail. */
  clipTo?: string
  /** Tracking forgiveness retains hover without intercepting desktop clicks. */
  hoverOnly?: boolean
}
const TOLERANCE = .04
const midpoint = (a:HitPoint,b:HitPoint):HitPoint => [(a[0]+b[0])/2,(a[1]+b[1])/2]
function distanceToLine(point:HitPoint,a:HitPoint,b:HitPoint):number {
  const dx=b[0]-a[0],dy=b[1]-a[1]
  return Math.hypot(dx,dy)<1e-12?Math.hypot(point[0]-a[0],point[1]-a[1]):Math.abs(dx*(a[1]-point[1])-(a[0]-point[0])*dy)/Math.hypot(dx,dy)
}
function cubic(out:HitPoint[],a:HitPoint,b:HitPoint,c:HitPoint,d:HitPoint,depth=0) {
  if(depth===14||Math.max(distanceToLine(b,a,d),distanceToLine(c,a,d))<=TOLERANCE){out.push(d);return}
  const ab=midpoint(a,b),bc=midpoint(b,c),cd=midpoint(c,d),abc=midpoint(ab,bc),bcd=midpoint(bc,cd),half=midpoint(abc,bcd)
  cubic(out,a,ab,abc,half,depth+1);cubic(out,half,bcd,cd,d,depth+1)
}
function arc(out:HitPoint[],start:HitPoint,rx:number,ry:number,rotation:number,large:number,sweep:number,end:HitPoint) {
  if(rx===0||ry===0){out.push(end);return}
  if(start[0]===end[0]&&start[1]===end[1])return
  rx=Math.abs(rx);ry=Math.abs(ry)
  const phi=rotation*Math.PI/180,cos=Math.cos(phi),sin=Math.sin(phi),dx=(start[0]-end[0])/2,dy=(start[1]-end[1])/2,x=cos*dx+sin*dy,y=-sin*dx+cos*dy
  const ratio=x*x/(rx*rx)+y*y/(ry*ry)
  if(ratio>1){rx*=Math.sqrt(ratio);ry*=Math.sqrt(ratio)}
  const factor=(large===sweep?-1:1)*Math.sqrt(Math.max(0,(rx*rx*ry*ry-rx*rx*y*y-ry*ry*x*x)/(rx*rx*y*y+ry*ry*x*x)))
  const cx=factor*rx*y/ry,cy=-factor*ry*x/rx,centre:HitPoint=[cos*cx-sin*cy+(start[0]+end[0])/2,sin*cx+cos*cy+(start[1]+end[1])/2]
  const theta=Math.atan2((y-cy)/ry,(x-cx)/rx)
  let delta=Math.atan2((-y-cy)/ry,(-x-cx)/rx)-theta
  if(!sweep&&delta>0)delta-=Math.PI*2
  if(sweep&&delta<0)delta+=Math.PI*2
  const count=Math.max(1,Math.ceil(Math.abs(delta)/(2*Math.acos(Math.max(-1,1-TOLERANCE/Math.max(rx,ry))))))
  for(let step=1;step<count;step++){
    const t=theta+delta*step/count,u=rx*Math.cos(t),v=ry*Math.sin(t)
    out.push([centre[0]+cos*u-sin*v,centre[1]+sin*u+cos*v])
  }
  out.push(end)
}

/** Flatten the commands produced by the existing SVG shape builders, instead
 * of maintaining a second approximation of their squircle, flare and tail. */
export function pathOutline(path:string):HitPoint[][] {
  const tokens=path.match(/[A-Za-z]|[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:e[-+]?\d+)?/g)??[]
  const contours:HitPoint[][]=[]
  let i=0,current:HitPoint=[0,0],out:HitPoint[]=[]
  const number=()=>{const value=Number(tokens[i++]);if(!Number.isFinite(value))throw new Error('Invalid panel shape coordinate');return value}
  const point=():HitPoint=>[number(),number()]
  while(i<tokens.length){
    const command=tokens[i++]
    if(command==='M'){if(out.length)contours.push(out);current=point();out=[current]}
    else if(command==='L'){current=point();out.push(current)}
    else if(command==='Q'){
      const control=point(),end=point(),a:HitPoint=[current[0]+(control[0]-current[0])*2/3,current[1]+(control[1]-current[1])*2/3],b:HitPoint=[end[0]+(control[0]-end[0])*2/3,end[1]+(control[1]-end[1])*2/3]
      cubic(out,current,a,b,end);current=end
    }else if(command==='C'){const a=point(),b=point(),end=point();cubic(out,current,a,b,end);current=end}
    else if(command==='A'){const rx=number(),ry=number(),rotation=number(),large=number(),sweep=number(),end=point();arc(out,current,rx,ry,rotation,large,sweep,end);current=end}
    else if(command==='Z'){if(out.length){contours.push(out);current=out[0];out=[]}}
    else throw new Error(`Unsupported panel shape command: ${command}`)
  }
  if(out.length)contours.push(out)
  return contours
}

export interface RailHitGeometry {
  edge:PanelEdge;left:number;top:number;width:number;length:number
  layout:DockLayout;openness:number;isDocked:boolean
}
export function railSurfaceRegion(o:RailHitGeometry,id='rail'):ShapeHitRegion {
  const width=o.layout.collapsedWidth+(o.width-o.layout.collapsedWidth)*o.openness,height=o.layout.collapsedHeight+(o.length-o.layout.collapsedHeight)*o.openness
  const canvasWidth=o.edge==='top'?height:width,canvasHeight=o.edge==='top'?width:height
  const x=o.left+(o.edge==='top'?(o.length-height)/2:o.edge==='right'?o.width-width:0),y=o.top+(o.edge==='top'?0:(o.length-height)/2)
  const outline=pathOutline(berthPath({width,height,openness:o.openness,isDocked:o.isDocked,layout:o.layout})).map(contour=>contour.map(([a,b]):HitPoint=>o.edge==='top'?[x+b,y+width-a]:o.edge==='left'?[x+width-a,y+b]:[x+a,y+b]))
  return {id,x,y,w:canvasWidth,h:canvasHeight,outline}
}
export function cardSurfaceRegion(o:{edge:PanelEdge;left:number;top:number;width:number;height:number;pointerCentre:number;layout:DetailCardLayout;usesRoundEnds:boolean}):ShapeHitRegion {
  const outline=pathOutline(bubblePath({width:o.width,height:o.height,edge:o.edge,pointerCenter:o.pointerCentre,cornerRadius:o.layout.cornerRadius,pointerWidth:o.layout.pointerWidth,pointerHeight:o.layout.pointerHeight,usesRoundEnds:o.usesRoundEnds})).map(contour=>contour.map(([x,y]):HitPoint=>o.edge==='top'?[o.left+y,o.top+o.height-x]:[o.left+x,o.top+y]))
  return {id:'card',x:o.left,y:o.top,w:o.width,h:o.height,outline}
}

/** Original PanelHitArea.slack. These rectangles report hover only. */
export const POINTER_SLACK=8
export function sliverTrackingRegion(o:RailHitGeometry):ShapeHitRegion {
  const width=o.layout.collapsedHitWidth,height=o.layout.collapsedHeight+POINTER_SLACK*2
  return o.edge==='top'?{id:'sliver',hoverOnly:true,x:o.left+(o.length-height)/2,y:o.top,w:height,h:width}
    :{id:'sliver',hoverOnly:true,x:o.left+(o.edge==='right'?o.width-width:0),y:o.top+(o.length-height)/2,w:width,h:height}
}
export function cardTrackingRegion(edge:PanelEdge,frameWidth:number,frameHeight:number,card:HitRegion):ShapeHitRegion {
  return edge==='top'?{id:'card',hoverOnly:true,x:card.x-POINTER_SLACK,y:0,w:card.w+POINTER_SLACK*2,h:frameHeight}
    :{id:'card',hoverOnly:true,x:0,y:card.y-POINTER_SLACK,w:frameWidth,h:card.h+POINTER_SLACK*2}
}

/** The geometry contract mirrored by the native receiver; used in probes and
 * tests only. Native input continues to come from window.cursor_position. */
export function outlineContains(outline:HitPoint[][],x:number,y:number):boolean {
  if(!Number.isFinite(x)||!Number.isFinite(y)||outline.some(contour=>contour.some(point=>point.some(value=>!Number.isFinite(value)))))return false
  let winding=0
  for(const contour of outline){
    if(contour.length<3)continue
    for(let index=0;index<contour.length;index++){
      const a=contour[index],b=contour[(index+1)%contour.length],cross=(b[0]-a[0])*(y-a[1])-(x-a[0])*(b[1]-a[1])
      if(Math.abs(cross)<=1e-8&&x>=Math.min(a[0],b[0])-1e-8&&x<=Math.max(a[0],b[0])+1e-8&&y>=Math.min(a[1],b[1])-1e-8&&y<=Math.max(a[1],b[1])+1e-8)return true
      if(a[1]<=y&&b[1]>y&&cross>0)winding++
      if(a[1]>y&&b[1]<=y&&cross<0)winding--
    }
  }
  return winding!==0
}
function surfaceContains(region:ShapeHitRegion,x:number,y:number):boolean {
  return [x,y,region.x,region.y,region.w,region.h].every(Number.isFinite)&&region.w>0&&region.h>0&&x>=region.x&&x<region.x+region.w&&y>=region.y&&y<region.y+region.h&&(!region.outline||outlineContains(region.outline,x,y))
}
export function regionContains(region:ShapeHitRegion,regions:ShapeHitRegion[],x:number,y:number):boolean {
  if(!surfaceContains(region,x,y))return false
  if(region.clipTo===undefined)return true
  const clip=regions.find(other=>other.id===region.clipTo&&!other.hoverOnly)
  return !!clip&&surfaceContains(clip,x,y)
}
export function pointerHits(regions:ShapeHitRegion[],x:number,y:number):{hovered:string|null;claimsClick:boolean} {
  const hovered=[...regions].reverse().find(region=>regionContains(region,regions,x,y))?.id??null
  const claimsClick=regions.some(region=>!region.hoverOnly&&regionContains(region,regions,x,y))
  return {hovered,claimsClick}
}
