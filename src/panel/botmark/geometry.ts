/**
 * The forward geometry helpers the mark is drawn with.
 *
 * Port of the original's `BotMarkGeometry`, same formulas and same constants.
 * Only the pieces the character itself needs are here: the ring maths, the two
 * paths (straight-sided for the eyes, smoothed for the head), and the span
 * scan. Yawed-solid silhouettes live in `solid.ts`.
 */

export interface Point {
  x: number
  y: number
}

/** The canvas every ring in `bot-data.json` is expressed in. */
export const HEAD_CENTRE = 114.2705
/** Half the distance between the eyes, in the same units. */
export const EYE_HALF = 21

export function centroid(ring: Point[]): Point {
  let x = 0
  let y = 0
  for (const point of ring) {
    x += point.x
    y += point.y
  }
  return { x: x / ring.length, y: y / ring.length }
}

/**
 * Point-by-point interpolation. Every ring in the data has the same count and
 * the same angular order, which is what makes this work.
 */
export function lerpRing(from: Point[], to: Point[], amount: number): Point[] {
  if (from.length !== to.length) return to
  return from.map((point, index) => ({
    x: point.x + (to[index].x - point.x) * amount,
    y: point.y + (to[index].y - point.y) * amount,
  }))
}

/** The straight-sided path, used for the eyes. */
export function ringPath(ring: Point[]): string {
  if (ring.length === 0) return ''
  return `M ${ring.map((p) => `${round(p.x)} ${round(p.y)}`).join(' L ')} Z`
}

/**
 * The smoothed path, used for a head that is blending or turning:
 * Catmull-Rom tangents over the ring, emitted as cubic Béziers.
 */
export function ringOutline(ring: Point[]): string {
  const count = ring.length
  if (count <= 2) return ringPath(ring)

  const parts: string[] = [`M ${round(ring[0].x)} ${round(ring[0].y)}`]
  for (let index = 0; index < count; index++) {
    const previous = ring[(index - 1 + count) % count]
    const point = ring[index]
    const next = ring[(index + 1) % count]
    const after = ring[(index + 2) % count]
    const c1x = point.x + (next.x - previous.x) / 6
    const c1y = point.y + (next.y - previous.y) / 6
    const c2x = next.x - (after.x - point.x) / 6
    const c2y = next.y - (after.y - point.y) / 6
    parts.push(
      `C ${round(c1x)} ${round(c1y)}, ${round(c2x)} ${round(c2y)}, ${round(next.x)} ${round(next.y)}`,
    )
  }
  parts.push('Z')
  return parts.join(' ')
}

/**
 * How wide the silhouette is at height `y`, by scanning the ring's crossings.
 * `headCentre` decides which side a crossing belongs to.
 */
export function spanAt(ring: Point[], y: number, headCentre: number): [number, number] {
  let left = -Infinity
  let right = Infinity
  for (let index = 0; index < ring.length; index++) {
    const start = ring[index]
    const end = ring[(index + 1) % ring.length]
    if (start.y <= y === end.y <= y) continue
    const x = start.x + ((end.x - start.x) * (y - start.y)) / (end.y - start.y)
    if (x <= headCentre) left = Math.max(left, x)
    else right = Math.min(right, x)
  }
  return [Number.isFinite(left) ? left : headCentre, Number.isFinite(right) ? right : headCentre]
}

/** Original 160 pre-sampled spans avoid rescanning a resting silhouette. */
export function shapeSpanAt(shape: {ring:Point[];top:number;bottom:number;spanSamples:[number,number][]},y:number):[number,number] {
  const samples=shape.spanSamples
  if(!samples.length)return spanAt(shape.ring,y,HEAD_CENTRE)
  const position=Math.max(0,Math.min(samples.length-1,(y-shape.top)/(shape.bottom-shape.top)*samples.length-.5)),start=Math.floor(position),end=Math.min(start+1,samples.length-1),amount=position-start
  return [samples[start][0]+(samples[end][0]-samples[start][0])*amount,samples[start][1]+(samples[end][1]-samples[start][1])*amount]
}

/** The shape's outline as an SVG `d`, from the segment list upstream stores. */
export function pathFromSegments(segments: { op: string; values: number[] }[]): string {
  const parts: string[] = []
  for (const segment of segments) {
    const values = segment.values.map(round).join(' ')
    parts.push(segment.op === 'Z' ? 'Z' : `${segment.op} ${values}`)
  }
  return parts.join(' ')
}

function round(value: number): number {
  return Math.round(value * 1000) / 1000
}
