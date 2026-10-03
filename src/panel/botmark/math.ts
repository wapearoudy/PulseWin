/** BotMath's arithmetic; clocks are milliseconds, physics deltas seconds. */
export const C = 114.2705
export const clamp = (v: number, lo = 0, hi = 1) => Math.max(lo, Math.min(hi, v))
export const cubicInOut = (v: number) => v < .5 ? 4*v*v*v : 1-(-2*v+2)**3/2
export const cubicOut = (v: number) => 1-(1-v)**3
export const backOut = (v: number) => 1+2.70158*(v-1)**3+1.70158*(v-1)**2
export const smoothstep = (v: number) => v*v*(3-2*v)
export const unit = (v: number) => ((v%1)+1)%1
export type Random = () => number
export const range = (random: Random, lo: number, hi: number) => lo+(hi-lo)*random()
export const wrapped = (v: number, period: number) => v-period*Math.round(v/period)
