/**
 * Derived geometry for one ring, ported from `UsageRingView`'s computed
 * properties.
 *
 * The original's rule is that layout constants are **budgets**, never
 * measurements: the panel frame is worked out from them before anything is laid
 * out, and "a budget that is short is not an approximation, it is a squeeze".
 * Kept pure and separate from the component for exactly that reason — so the
 * numbers can be asserted against the original's own stated values.
 */

/** Gap between the progress ring and the dark disc it encircles. */
export const CENTRE_GAP = 4

/** The icon's share of that dark disc. */
export const ICON_SCALE = 0.8
export const BOT_SCALE = 1.4

/** How much the icon's disc shrinks to make room for the second ring, per side. */
export const SECOND_RING_SQUEEZE = 2

export const HALO_RADIUS = 10

export const CLOCK_GAP = 3
export const CLOCK_LINE_WIDTH = 2

export interface RingGeometryInput {
  /** The usage ring's nominal diameter. Its ink extends `lineWidth / 2` beyond this. */
  diameter: number
  lineWidth: number
  /** `PanelMetrics.scale`. */
  scale: number
  /**
   * Whether a second ring is drawn.
   *
   * The disc gives up two points a side only when it actually is: "a provider
   * with one limit keeps the icon it had."
   */
  hasSecondRing: boolean
  secondRingDiameter: number
}

export interface RingGeometry {
  /** Diameter of the dark disc behind the provider's glyph. */
  centreDiameter: number
  /** The circle the white travelling activity mark is stroked along. */
  busyDiameter: number
  /** The circle the window-clock hairline is stroked along. */
  clockDiameter: number
  /** Radius the usage arc and its track are stroked along. Strokes are centred, so ink spans ±lineWidth/2. */
  ringRadius: number
  /** Radius the second ring is stroked along. */
  secondRingRadius: number
  /**
   * Half the widest ink the whole ring produces, including the clock arc.
   *
   * The ring's own ink reaches `diameter/2 + lineWidth/2` past the nominal
   * frame, and the clock arc sits outside that again — which is why the
   * component's viewBox is this and not `diameter`.
   */
  inkHalf: number
}

export function ringGeometry({
  diameter,
  lineWidth,
  scale,
  hasSecondRing,
  secondRingDiameter,
}: RingGeometryInput): RingGeometry {
  const squeeze = hasSecondRing ? SECOND_RING_SQUEEZE * scale : 0

  const centreDiameter = Math.max(
    diameter - (lineWidth + CENTRE_GAP) * 2 - squeeze * 2,
    0,
  )

  // The busy arc rides the empty ring between the disc and the usage ring —
  // halfway between the two, so it touches neither. **The second ring takes
  // this band**, and the providers that report two limits are exactly the ones
  // whose CLIs make this spin, so with it drawn the mark rides just outside the
  // disc instead.
  const busyDiameter = hasSecondRing
    ? Math.max(centreDiameter + SECOND_RING_SQUEEZE * scale, 0)
    : Math.max(diameter - lineWidth * 1.5 - CENTRE_GAP, 0)

  const clockDiameter =
    diameter + lineWidth + (CLOCK_GAP + CLOCK_LINE_WIDTH / 2) * 2 * scale

  const ringRadius = diameter / 2
  const secondRingRadius = secondRingDiameter / 2

  // The clock arc is an overlay drawn outside the ring's frame, and it is the
  // widest thing here, so it sets the viewBox.
  const inkHalf = Math.max(ringRadius + lineWidth / 2, clockDiameter / 2 + CLOCK_LINE_WIDTH / 2)

  return {
    centreDiameter,
    busyDiameter,
    clockDiameter,
    ringRadius,
    secondRingRadius,
    inkHalf,
  }
}

/** Length of a `trim(from: 0, to: fraction)` dash, and the gap that follows it. */
export function dashArray(fraction: number, radius: number): string {
  const circumference = 2 * Math.PI * radius
  const drawn = Math.max(0, Math.min(1, fraction)) * circumference
  return `${drawn} ${circumference}`
}
