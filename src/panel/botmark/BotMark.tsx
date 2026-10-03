import { memo, useEffect, useId, useRef } from 'react'
import { listen } from '@tauri-apps/api/event'
import { EXPRESSIONS, shape } from './data'
import { ringPath, type Point } from './geometry'
import { resolveMood, type BotMarkMood } from './mood'
import { eyesOn } from './tint'
import type { Persona } from './programme'
import { BotMarkEngine, settledFrame, type EngineInput } from './engine'

export interface BotMarkProps extends EngineInput {
  mood:BotMarkMood;colour:string;persona?:Persona
  /** Unix milliseconds from an unambiguous live window-reset observation.
   * A timestamp present on mount is remembered, never replayed. */
  resetAt?:number
  resetCelebrationAllowed?:boolean
}
function BotMarkImpl(props:BotMarkProps){
  const latest=useRef(props);latest.current=props
  const svg=useRef<SVGSVGElement>(null),head=useRef<SVGPathElement>(null),clip=useRef<SVGPathElement>(null),group=useRef<SVGGElement>(null)
  const decor=useRef<SVGGElement>(null),badge=useRef<SVGGElement>(null),back=useRef<SVGGElement>(null),front=useRef<SVGGElement>(null)
  const eyes=useRef<(SVGPathElement|null)[]>([]),pointer=useRef<Point|null>(null),uid=useId().replace(/[^a-zA-Z0-9_-]/g,'')
  useEffect(()=>{
    let dead=false,stop:(()=>void)|undefined
    listen<Point>('pointer-position',e=>{
      const b=svg.current?.getBoundingClientRect();if(!b)return
      const x=(e.payload.x-b.left-b.width/2)/(b.width*2),y=(e.payload.y-b.top-b.height/2)/(b.height*2)
      pointer.current={x,y}
    }).then(fn=>{if(dead)fn();else stop=fn}).catch(()=>undefined)
    return()=>{dead=true;stop?.()}
  },[])
  useEffect(()=>{
    const engine=new BotMarkEngine(latest.current),reduce=matchMedia('(prefers-reduced-motion: reduce)')
    let raf=0,last=0,stillKey=''
    const frame=(time:number)=>{
      raf=requestAnimationFrame(frame)
      // The original renders at 30fps; seven persistent marks do not need
      // the display's refresh rate to play these slow authored scenes.
      if(last&&time-last<1000/30-1)return
      const delta=last?Math.min((time-last)/1000,.1):1/30;last=time
      const p=latest.current,moving=p.animated!==false&&!reduce.matches
      const width=svg.current?.getBoundingClientRect().width??28
      const key=JSON.stringify([p.persona,p.mood,p.body,p.gaze,!!p.isPointedAt,!!p.isQuiet,p.colour,width])
      if(!moving&&stillKey===key)return
      stillKey=moving?'':key
      const f=moving?engine.advance(delta,{...p,animated:true,pointer:p.pointer===undefined?pointer.current:p.pointer},Date.now(),width,uid):settledFrame(p,width)
      head.current?.setAttribute('d',f.head);clip.current?.setAttribute('d',f.head)
      group.current?.setAttribute('transform',f.transform);group.current?.setAttribute('opacity',String(f.opacity))
      if(decor.current){decor.current.innerHTML=f.shapes;decor.current.style.color=p.colour}
      if(badge.current){badge.current.innerHTML=f.badge;badge.current.setAttribute('transform',f.transform);badge.current.setAttribute('opacity',String(f.opacity));badge.current.style.color=eyesOn(p.colour)}
      if(back.current)back.current.innerHTML=f.backParticles
      if(front.current)front.current.innerHTML=f.frontParticles
      f.eyes.forEach((eye,i)=>{const node=eyes.current[i];if(!node)return;node.setAttribute('d',eye.path);node.setAttribute('transform',eye.transform);node.setAttribute('visibility',eye.visible?'visible':'hidden')})
      svg.current?.setAttribute('data-state',f.state);svg.current?.setAttribute('data-body',f.body);svg.current?.setAttribute('viewBox',f.viewBox)
    }
    raf=requestAnimationFrame(frame);return()=>cancelAnimationFrame(raf)
  },[])
  const initial=shape(props.body??'blob')
  return <svg ref={svg} className="botmark" data-persona={props.persona??'calm'} data-body={initial.id} viewBox="-15 -15 259 259" width="100%" height="100%" aria-hidden="true">
    <defs><clipPath id={`${uid}-body`}><path ref={clip} d={initial.outline}/></clipPath></defs>
    <g ref={back}/><g ref={decor}/><g ref={group}><path ref={head} fill={props.colour} d={initial.outline}/><g clipPath={`url(#${uid}-body)`}>{[0,1].map(i=><path key={i} ref={node=>{eyes.current[i]=node}} fill={eyesOn(props.colour)} d={ringPath(EXPRESSIONS[0][i])}/>)}</g></g><g ref={front}/><g ref={badge}/>
  </svg>
}
export const BotMark=memo(BotMarkImpl)
export {resolveMood}
export type {BotMarkMood}
