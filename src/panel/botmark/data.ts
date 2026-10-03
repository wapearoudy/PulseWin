/**
 * The mark's character sheet, ported verbatim.
 *
 * `bot-data.json` is the original's own resource, copied unchanged: eighteen
 * bodies sampled to the same 96-point ring, twenty-five eye expressions of two
 * 48-point rings each, and the thirty-nine-state table. Transcribing any of it
 * by hand would have been a paraphrase of a drawing.
 */
import raw from './bot-data.json'
import type { Point } from './geometry'
import { pathFromSegments } from './geometry'

export interface BotMarkFace {
  x: number
  y: number
  sx: number
  sy: number
  eye: number
}

export interface BotMarkShape {
  id: string
  ring: Point[]
  /** The body's outline, ready to drop into an SVG `d`. */
  outline: string
  face: BotMarkFace
  tiltScale: number
  radius: number
  beltRadius: number
  top: number
  bottom: number
  solid?: number[][]
  sides: number
  spanSamples: [number, number][]
}

export interface BotMarkState {
  id: string
  morph: string | null
  blinkCadence: [number, number] | null
  expressionCadence: [number, number]
  expressionPool: number[]
}

export interface BotMarkLabel {
  en: string
  zh: string
}

interface RawShape {
  ring: number[][]
  path: { op: string; values: number[] }[]
  face: BotMarkFace
  tiltScale: number
  radius: number
  beltRadius: number
  top: number
  bottom: number
  solid?: number[][]
  sides: number
  spanSamples: number[][]
}

interface RawState {
  id: string
  morph?: string | null
  blinkCadence?: number[] | null
  expressionCadence: number[]
  expressionPool: number[]
}

const file = raw as unknown as {
  headC: number
  eyeHalf: number
  shapes: Record<string, RawShape>
  shapeOrder: string[]
  shapeLabels: Record<string, BotMarkLabel>
  expressions: number[][][][]
  states: RawState[]
  starPath: { op: string; values: number[] }[]
  starGold: string
}

const toPoints = (pairs: number[][]): Point[] => pairs.map(([x, y]) => ({ x, y }))

function toShape(id: string, source: RawShape): BotMarkShape {
  return {
    id,
    ring: toPoints(source.ring),
    outline: pathFromSegments(source.path),
    face: source.face,
    tiltScale: source.tiltScale,
    radius: source.radius,
    beltRadius: source.beltRadius,
    top: source.top,
    bottom: source.bottom,
    sides: source.sides,
    solid: source.solid,
    spanSamples: source.spanSamples.map(([left, right]) => [left, right]),
  }
}

export const SHAPES: Record<string, BotMarkShape> = Object.fromEntries(
  Object.entries(file.shapes).map(([id, shape]) => [id, toShape(id, shape)]),
)

/** Declaration order, which is also the order the settings picker uses. */
export const SHAPE_ORDER: string[] = file.shapeOrder

export const SHAPE_LABELS: Record<string, BotMarkLabel> = file.shapeLabels

/** Every expression is two eye rings in the canvas's own coordinates. */
export const EXPRESSIONS: Point[][][] = file.expressions.map((rings) =>
  rings.map((ring) => toPoints(ring)),
)

export const STATES: Record<string, BotMarkState> = Object.fromEntries(
  file.states.map((state) => [
    state.id,
    {
      id: state.id,
      morph: state.morph ?? null,
      blinkCadence: state.blinkCadence ? [state.blinkCadence[0], state.blinkCadence[1]] : null,
      expressionCadence: [state.expressionCadence[0], state.expressionCadence[1]],
      expressionPool: state.expressionPool,
    } satisfies BotMarkState,
  ]),
)

export function state(id: string): BotMarkState {
  return STATES[id] ?? STATES.idle
}

export function shape(id: string): BotMarkShape {
  return SHAPES[id] ?? SHAPES.blob
}

/** The bodies the settings picker offers, in upstream order. */
export function shapeChoices(): { id: string; label: string }[] {
  return SHAPE_ORDER.map((id) => ({ id, label: SHAPE_LABELS[id]?.en ?? id }))
}

/** `eyeHalf`: half the distance between the eyes, in canvas units. */
export const EYE_HALF = file.eyeHalf
export const STAR_PATH = pathFromSegments(file.starPath)
export const STAR_GOLD = file.starGold
