// Port of Pulse BotMarkChoreography / Persona / Programme. Authored beats
// are extracted verbatim into choreography.json; never choose a random subset.
import choreography from './choreography.json'
import type { BotMarkMood } from './mood'
export const PERSONAS = ['calm','eager','steady','curious','sleepy','playful','stoic','proud'] as const
export type Persona = typeof PERSONAS[number]
export const PERSONA_LABELS = ['沉稳','热切','踏实','好奇','迟缓','顽皮','冷面','得意']
export type Beat = [state:string, minMs:number, maxMs:number]
export function personaAt(value:string, index=0): Persona {
  return PERSONAS.includes(value as Persona) ? value as Persona : PERSONAS[((index%8)+8)%8]
}
export function routine(persona:Persona, mood:BotMarkMood, pointed=false, quiet=false, date=new Date()): Beat[] {
  const scene=choreography[persona]
  const beats=(scene[mood==='idle' && pointed ? 'attention' : mood] as Beat[]).map(b=>[...b] as Beat)
  if(mood==='working' && (date.getDay()===0 || date.getDay()===6 || date.getHours()<9 || date.getHours()>=21)) beats.push(['angry',1800,2500])
  if(mood==='idle' && !pointed && quiet && persona==='sleepy' && (date.getHours()>=23 || date.getHours()<8)) beats.push(['drowsy',2000,3000])
  return beats
}
export function personality(persona:Persona) {
  return {tempo:persona==='playful'||persona==='eager'?.85:persona==='sleepy'?1.5:persona==='stoic'?1.25:1,
    motion:persona==='playful'?1.15:persona==='eager'?1.1:persona==='sleepy'?.8:persona==='stoic'?.6:1,
    gaze:persona==='curious'||persona==='eager'?1.2:persona==='stoic'?.5:persona==='sleepy'?.7:1,
    eyes:persona==='eager'||persona==='curious'?1.06:persona==='stoic'?.94:1}
}
export function completionState(persona:Persona) { return choreography[persona].completion }
export class Spring {
  velocity=0; target:number
  constructor(public value:number) { this.target=value }
  step(frequency:number,damping:number,delta:number) {
    const steps=Math.max(1,Math.ceil(delta*120)), dt=delta/steps
    for(let i=0;i<steps;i++) { this.velocity+=(-2*damping*frequency*this.velocity-frequency*frequency*(this.value-this.target))*dt; this.value+=this.velocity*dt }
    if(!Number.isFinite(this.value)) { this.value=this.target; this.velocity=0 }
    return this.value
  }
}
