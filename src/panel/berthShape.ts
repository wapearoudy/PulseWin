/**
 * Port of Pulse's `DockLayout` (Sources/Pulse/Panel/UsageDockView.swift) and
 * `DockBerthShape`.
 *
 * Every number here is a budget, not a measurement: the AppKit window frame is
 * derived from these before SwiftUI lays anything out. The original's own docs
 * are emphatic that a budget which is *short* is not an approximation, it is a
 * squeeze — `percentTextHeight` was 15 against a rendered 15.6–16.0 and that
 * pushed ring hit targets up to 6pt off down a full rail.
 */

export type PanelEdge = 'right' | 'left' | 'top'

/** PanelSize → PanelMetrics.scale. "Deliberately modest steps." */
export const PANEL_SCALE = {
  small: 0.82,
  standard: 1,
  large: 1.22,
} as const

/** RailSpacing → PanelMetrics.spacing. Applied to `itemSpacing` only. */
export const RAIL_SPACING = {
  compact: 0.6,
  standard: 1,
  roomy: 1.4,
} as const

export type PanelSizeName = keyof typeof PANEL_SCALE
export type RailSpacingName = keyof typeof RAIL_SPACING

/** How many rings the panel has to leave room for (`PanelMetrics.railCapacity`). */
export const DEFAULT_RAIL_CAPACITY = 25

export interface DockLayout {
  /** Rail width. Flush against the screen edge. */
  width: number
  /**
   * Measured from the panel's top edge, NOT from the rail body's flat top —
   * the concave flare occupies the first `flareHeight` of it. Visible breathing
   * room above the first ring is therefore `verticalPadding - flareHeight`.
   */
  verticalPadding: number
  /**
   * Room added at each end beyond sitting the first ring exactly on the centre
   * of the rounded end. Zero with softened ends, whose 46pt padding is written
   * rather than derived — there is no end circle to be concentric with.
   */
  endRingOffset: number
  horizontalPadding: number
  ringDiameter: number
  ringLineWidth: number
  ringToTextSpacing: number
  percentFontSize: number
  /** Budget height of the percent label; the original measured 15.6–16.0 and rounded up. */
  percentTextHeight: number
  /** Budget width of the label at "100%". */
  percentTextWidth: number
  /** Gap between ring+label items along the rail. */
  itemSpacing: number
  secondRingDiameter: number
  secondRingLineWidth: number
  /** Rail body's inner-side corner reach. */
  cornerRadius: number
  /** How far the concave flare rises above the body's flat top edge. */
  flareHeight: number
  /** How far in from the screen edge the flare starts sweeping. */
  flareWidth: number
  /** Ring + percent label. */
  itemHeight: number
  collapsedWidth: number
  collapsedHeight: number
  collapsedHitWidth: number
}

export interface DockLayoutOptions {
  scale?: number
  /** `AppSettings.usesRoundEnds`, default off. */
  usesRoundEnds?: boolean
  /** `AppSettings.railSpacing` multiplier, default 1. */
  spacing?: number
}

/**
 * Derive the rail's layout constants. Mirrors `DockLayout`'s computed `var`s —
 * note `cornerRadius + flareWidth` equals `width` exactly in both end styles,
 * because the corner and the flare share the rail's top and bottom edges.
 */
export function dockLayout(options: DockLayoutOptions = {}): DockLayout {
  const scale = options.scale ?? PANEL_SCALE.standard
  const usesRoundEnds = options.usesRoundEnds ?? false
  const spacing = options.spacing ?? 1

  const width = 64 * scale
  const ringDiameter = 36 * scale
  const flareHeight = usesRoundEnds ? width / 2 : 24 * scale
  const cornerRadius = usesRoundEnds ? width / 2 : 26 * scale
  const flareWidth = usesRoundEnds ? width - cornerRadius : 38 * scale
  const endRingOffset = usesRoundEnds ? 8 * scale : 0
  const ringToTextSpacing = 6 * scale
  const percentTextHeight = 16 * scale

  const verticalPadding = usesRoundEnds
    ? flareHeight + cornerRadius - ringDiameter / 2 + endRingOffset
    : 46 * scale

  return {
    width,
    verticalPadding,
    endRingOffset,
    horizontalPadding: 10 * scale,
    ringDiameter,
    ringLineWidth: 4 * scale,
    ringToTextSpacing,
    percentFontSize: 13 * scale,
    percentTextHeight,
    percentTextWidth: 38 * scale,
    itemSpacing: 30 * scale * spacing,
    secondRingDiameter: 26 * scale,
    secondRingLineWidth: 2.5 * scale,
    cornerRadius,
    flareHeight,
    flareWidth,
    itemHeight: ringDiameter + ringToTextSpacing + percentTextHeight,
    collapsedWidth: 6 * scale,
    collapsedHeight: 96 * scale,
    collapsedHitWidth: 20 * scale,
  }
}

/** Superellipse exponent. 2 is a plain circle; 4 lands close to Apple's squircle. */
const SQUIRCLE_EXPONENT = 4
/** Enough segments that the sampled curve stays sub-pixel smooth at rail sizes. */
const CORNER_SAMPLE_COUNT = 48

interface Vec {
  dx: number
  dy: number
}

/**
 * One quarter of a superellipse, appended as line segments.
 *
 * A circular arc jumps from zero curvature along the straight edge to
 * `1/radius` the instant the corner starts, and that discontinuity is what
 * reads as the edge having been sliced off. A superellipse eases the curvature
 * in, so the straight edge and the corner belong to the same stroke. SwiftUI
 * exposes this as `.continuous` for rounded rectangles; the berth outline is
 * drawn by hand, so it is sampled.
 *
 * `from` and `to` are unit directions from `center` to the corner's start and
 * end points; they must be perpendicular and axis-aligned.
 */
function superellipseCorner(
  center: { x: number; y: number },
  radius: number,
  from: Vec,
  to: Vec,
): Array<{ x: number; y: number }> {
  if (radius <= 0) return []

  const points: Array<{ x: number; y: number }> = []
  for (let step = 1; step <= CORNER_SAMPLE_COUNT; step++) {
    const t = (step / CORNER_SAMPLE_COUNT) * (Math.PI / 2)
    // |x/r|^n + |y/r|^n = 1 in parametric form.
    const along = Math.pow(Math.cos(t), 2 / SQUIRCLE_EXPONENT)
    const across = Math.pow(Math.sin(t), 2 / SQUIRCLE_EXPONENT)
    points.push({
      x: center.x + radius * (from.dx * along + to.dx * across),
      y: center.y + radius * (from.dy * along + to.dy * across),
    })
  }
  return points
}

const n = (v: number) => (Math.round(v * 1000) / 1000).toString()

export interface BerthPathOptions {
  width: number
  height: number
  /**
   * How far open the berth is: 0 is the collapsed sliver, 1 the full rail.
   *
   * The sliver is not a different shape — it is this one with its flare and
   * corners wound all the way down, which is what lets the two animate between
   * each other as one object changing size rather than being swapped. At 0 the
   * flare vanishes and the corner radius equals the whole width.
   */
  openness?: number
  /** Off the edge there is nothing to fuse with, so the flare gives way to a capsule. */
  isDocked?: boolean
  layout?: DockLayout
}

/**
 * The rail's outline, always generated facing RIGHT; callers mirror or rotate
 * it into place. The original draws it once this way because the outline
 * carries no text and no asymmetric detail, so transforming the finished path
 * is exact — and a second copy of the geometry could drift from the first.
 *
 * The right edge runs flush against the screen for the shape's whole height —
 * nothing is rounded there, so no wallpaper ever shows between the rail and the
 * edge. The body is inset from the top and bottom by `flareHeight`, and each
 * end sweeps out to the screen edge through a concave fillet that leaves the
 * body's flat edge horizontally and meets the screen edge vertically, so both
 * junctions are tangent-continuous.
 */
export function berthPath(options: BerthPathOptions): string {
  const { width: w, height: h } = options
  const openness = options.openness ?? 1
  const layout = options.layout ?? dockLayout()

  if (options.isDocked === false) {
    // A true capsule: half circles, not rounded-off corners. A squircle eases
    // its curvature into the straight edge either side of it, and at this width
    // the two corners of an end meet with no straight edge between them at all
    // — so there is nothing to ease into and it reads as a flattened lozenge.
    const radius = Math.min(w, h) / 2
    return capsulePath(w, h, radius)
  }

  const f = Math.min(layout.flareHeight * openness, h / 2)
  // The convex corners live on the body, which spans y in [f, h - f].
  const r = Math.max(Math.min(cornerRadiusAt(layout, openness), Math.min(w, (h - f * 2) / 2)), 0)
  // The flare must leave room for that corner: if `flareWidth + r` exceeded the
  // width, the body's flat top edge would run backwards and the path would fold
  // in on itself.
  const fw = Math.max(Math.min(layout.flareWidth * openness, w - r), 0)

  // Pulls each fillet's control points off its endpoints. 0.55 is the usual
  // circular-arc approximation; it keeps the sweep full instead of flattening
  // it into a sliver.
  const k = 0.55

  const parts: string[] = []

  // Body's flat top edge, left to right.
  parts.push(`M ${n(r)} ${n(f)}`)
  parts.push(`L ${n(w - fw)} ${n(f)}`)

  // Concave fillet sweeping up into the screen edge.
  parts.push(
    `C ${n(w - fw * (1 - k))} ${n(f)}, ${n(w)} ${n(f * k)}, ${n(w)} 0`,
  )

  // Flush against the screen for the full height.
  parts.push(`L ${n(w)} ${n(h)}`)

  // Mirrored fillet back down into the body's bottom edge.
  parts.push(
    `C ${n(w)} ${n(h - f * k)}, ${n(w - fw * (1 - k))} ${n(h - f)}, ${n(w - fw)} ${n(h - f)}`,
  )

  // Body's flat bottom edge, right to left.
  parts.push(`L ${n(r)} ${n(h - f)}`)

  // The two convex corners. Both styles each start at the tangent point the
  // path is already on: bottom edge round to the left edge, then left edge
  // round to the top one.
  const quad = (p: { x: number; y: number }) => `Q ${n(p.x)} ${n(p.y)} ${n(p.x)} ${n(p.y)}`

  const bottomCorner = superellipseCorner(
    { x: r, y: h - f - r },
    r,
    { dx: 0, dy: 1 },
    { dx: -1, dy: 0 },
  )
  for (const p of bottomCorner) parts.push(quad(p))

  // Left edge.
  parts.push(`L 0 ${n(f + r)}`)

  const topCorner = superellipseCorner(
    { x: r, y: f + r },
    r,
    { dx: -1, dy: 0 },
    { dx: 0, dy: -1 },
  )
  for (const p of topCorner) parts.push(quad(p))

  parts.push('Z')
  return parts.join(' ')
}

function capsulePath(w: number, h: number, radius: number): string {
  const r = Math.min(radius, Math.min(w, h) / 2)
  return [
    `M ${n(r)} 0`,
    `L ${n(w - r)} 0`,
    `A ${n(r)} ${n(r)} 0 0 1 ${n(w)} ${n(r)}`,
    `L ${n(w)} ${n(h-r)}`,
    `A ${n(r)} ${n(r)} 0 0 1 ${n(w-r)} ${n(h)}`,
    `L ${n(r)} ${n(h)}`,
    `A ${n(r)} ${n(r)} 0 0 1 0 ${n(h-r)}`,
    `L 0 ${n(r)}`,
    `A ${n(r)} ${n(r)} 0 0 1 ${n(r)} 0`,
    'Z',
  ].join(' ')
}

/**
 * Placement transform for the three edges, as an SVG `transform` string.
 *
 * Mirrors the original exactly: the canonical path is drawn facing right (in a
 * `w × h` box) and then moved. Left is a mirror about the vertical axis; top is
 * a quarter turn anticlockwise, which carries the flare from the right-hand
 * edge to the top one. Top is a **rotation rather than a reflection**, because
 * rotation preserves the path's winding.
 */
export function berthTransform(
  edge: PanelEdge,
  width: number,
  height: number,
): string | undefined {
  switch (edge) {
    case 'right':
      return undefined
    case 'left':
      return `translate(${n(width)} 0) scale(-1 1)`
    case 'top':
      // The canonical rect is this one laid on its side.
      return `rotate(-90) translate(${n(-height)} 0)`
  }
}

/** The `w × h` box the canonical (facing-right) path is drawn into, per edge. */
export function berthCanvas(
  edge: PanelEdge,
  width: number,
  height: number,
): { width: number; height: number } {
  return edge === 'top'
    ? { width: height, height: width }
    : { width, height }
}

/**
 * `openness` interpolates the corner radius from the collapsed sliver's width
 * up to the full rail's corner, so the sliver is the same shape wound down
 * rather than a second view.
 */
export function cornerRadiusAt(layout: DockLayout, openness: number): number {
  return layout.collapsedWidth + (layout.cornerRadius - layout.collapsedWidth) * openness
}
