import { memo, useId } from 'react'
import { usageTint, isSpent as computeSpent, USAGE_TINT, type UsageTintName } from './tint'
import {
  CLOCK_LINE_WIDTH,
  ICON_SCALE,
  BOT_SCALE,
  HALO_RADIUS,
  dashArray as dash,
  ringGeometry,
} from './ringGeometry'

export { HALO_RADIUS } from './ringGeometry'

/**
 * Port of Pulse's `UsageRingView` (Sources/Pulse/Panel/UsageRingView.swift).
 *
 * Layer order, bottom to top:
 *   1. track            — a full circle, `primary` at 18%
 *   2. usage arc        — the coloured reading, with the hover halo
 *   3. icon disc + icon — `centreDiameter`
 *   4. second ring      — the next-fullest limit, inside the first
 *   5. activity mark    — the white travelling arc, between disc and ring
 *   6. window clock     — an **overlay outside** the ring's frame
 *
 * **The halo belongs to the arc, not to the ring view.** Hung on the whole view
 * it is drawn behind everything, including behind the icon, whose antialiased
 * edges let it through — the original measured a green cast of +16 over neutral
 * on a 35% ring that way.
 *
 * **The clock arc is an overlay, after the frame, never a stack child.** A
 * fixed-size child sets the stack's size, and the clock circle is wider than
 * the ring by design: inside the stack it grew the usage ring from 36pt to
 * 52pt, moving every ring centre the hit testing is measured against.
 */

/** Short enough to read as a travelling mark rather than a second progress ring. */
const BUSY_SWEEP = 0.22
const BUSY_PERIOD = 1.0
const REFRESH_SWEEP = 0.16
const REFRESH_PERIOD = 0.85

/** The track is `Color.primary.opacity(0.18)` — white at 18% on the dark panel. */
const TRACK_OPACITY = 0.18
const CLOCK_TRACK_OPACITY = 0.16
const CLOCK_ARC_OPACITY = 0.7

export interface UsageRingProps {
  /**
   * 0..1, or `null` when the provider reported no figure.
   *
   * Drives the **colour** either way: what it means — how close this limit is —
   * does not change because the figure beside it was counted from the other
   * end. So a ring with a sliver left is a small red arc, not a large one.
   */
  usedFraction: number | null | undefined
  /** Whether the provider itself says this limit is spent. Not derived from the fraction. */
  isSpent?: boolean
  /** Draw the arc as what is **left** rather than what is gone. Only the arc. */
  showsRemaining?: boolean
  /** The usage ring's nominal diameter. Its ink extends `lineWidth / 2` beyond this. */
  diameter: number
  lineWidth: number
  /** Whether this provider's CLI is working right now. Drives the white travelling mark. */
  isBusy?: boolean
  /** Whether Pulse is fetching. Rotates a coloured segment; white stays reserved for CLI activity. */
  isRefreshing?: boolean
  /** Off: the two facts above are still tracked but draw nothing extra. */
  animatesActivity?: boolean
  /** Whether this ring is the one being pointed at. */
  highlight?: boolean
  /** The next-fullest limit, drawn as a smaller ring inside this one. `null` for a single-limit provider. */
  secondFraction?: number | null
  secondIsSpent?: boolean
  /** How much of the window clock to draw, 0..1, or `null` to leave it out. */
  windowClockFraction?: number | null
  /** Where red begins. One number the whole rail has to agree on. */
  warningAt: number
  /** `/`-independent scale from `PanelMetrics`. */
  scale?: number
  /** `DockLayout.secondRingDiameter`, threaded in so the two cannot drift. */
  secondRingDiameter?: number
  /** `DockLayout.secondRingLineWidth`. */
  secondRingLineWidth?: number
  /** The provider's glyph, drawn on the dark disc. */
  icon?: React.ReactNode
  showsBotMark?: boolean
  /** Dims the icon when there is no reading. */
  hasReading?: boolean
  /** A chosen colour for this account's ring. Spent still wins. */
  chosenTint?: string | null
}

function UsageRingImpl({
  usedFraction,
  isSpent = false,
  showsRemaining = false,
  diameter,
  lineWidth,
  isBusy = false,
  isRefreshing = false,
  animatesActivity = true,
  highlight = false,
  secondFraction = null,
  secondIsSpent = false,
  windowClockFraction = null,
  warningAt,
  scale = 1,
  secondRingDiameter = 26,
  secondRingLineWidth = 2.5,
  icon,
  showsBotMark = false,
  hasReading = true,
  chosenTint = null,
}: UsageRingProps) {
  const uid = useId()

  const spent = computeSpent(usedFraction, isSpent)
  const used = usedFraction === null || usedFraction === undefined ? 0 : clamp01(usedFraction)

  const secondDrawn = secondFraction !== null && secondFraction !== undefined

  const { centreDiameter, busyDiameter, clockDiameter, ringRadius, secondRingRadius, inkHalf } =
    ringGeometry({ diameter, lineWidth, scale, hasSecondRing: secondDrawn, secondRingDiameter })

  // Colour says how full the limit is, not which provider this is.
  const tintName: UsageTintName | null =
    chosenTint && !spent ? null : usageTint({ usedFraction, isExhausted: isSpent, warningAt })
  const arcColour = chosenTint && !spent ? chosenTint : USAGE_TINT[tintName ?? 'good']

  // Full when spent, whichever way it counts — the most urgent state must not
  // be the one with the least ink. And never let a full ring mean "over limit":
  // clamp the arc and let the number carry any overage.
  const arcFraction = usedFraction == null || !Number.isFinite(usedFraction) ? 0 : spent ? 1 : clamp01(showsRemaining ? 1 - used : used)

  const secondUsed = secondFraction === null || secondFraction === undefined ? 0 : clamp01(secondFraction)
  const secondSpent = secondIsSpent || secondUsed >= 1
  const secondShown = showsRemaining && !secondSpent ? 1 - secondUsed : secondUsed
  const secondArcFraction = secondSpent ? 1 : clamp01(secondShown)
  // Same colour language as the outer ring, deliberately: two arcs measuring
  // the same kind of thing must be read the same way.
  const secondColour = chosenTint && !secondSpent ? chosenTint : USAGE_TINT[
    usageTint({ usedFraction: secondUsed, isExhausted: secondSpent, warningAt })
  ]

  const half = inkHalf
  const size = half * 2

  const iconDiameter=centreDiameter*(showsBotMark?BOT_SCALE:ICON_SCALE)
  const showActivity = isBusy && !showsBotMark && animatesActivity
  const showRefresh = isRefreshing && animatesActivity

  return (
    <svg
      className="ring"
      width={diameter}
      height={diameter}
      viewBox={`${-diameter/2} ${-diameter/2} ${diameter} ${diameter}`}
      aria-hidden="true"
      style={{ display: 'block', overflow: 'visible', flexShrink: 0 }}
    >
      <defs>
        {/*
          Keeps the halo out of the ring's centre. The glow is a shadow cast by
          the arc, so it spreads inwards as well as outwards, and the inward
          half lands under the provider's mark. A mask has no colour to get
          wrong — the original's opaque-disc version works on the black panel
          (a black disc on a black rail is invisible) but on glass it becomes a
          solid white coin behind the mark.
        */}
        <mask id={`${uid}-halo`} maskUnits="userSpaceOnUse" x={-half} y={-half} width={size} height={size}>
          <circle cx={0} cy={0} r={half + HALO_RADIUS * 2} fill="white" />
          <circle cx={0} cy={0} r={centreDiameter / 2} fill="black" />
        </mask>
      </defs>

      {/* 1. Track */}
      <circle
        cx={0}
        cy={0}
        r={ringRadius}
        fill="none"
        stroke={`rgba(255,255,255,${TRACK_OPACITY})`}
        strokeWidth={lineWidth}
      />

      {/* 2. Usage arc (+ hover halo, masked to the outward half) */}
      {arcFraction > 0 && (
        <circle
          cx={0}
          cy={0}
          r={ringRadius}
          fill="none"
          stroke={arcColour}
          strokeWidth={lineWidth}
          strokeLinecap="round"
          strokeDasharray={dash(arcFraction, ringRadius)}
          transform="rotate(-90)"
          mask={highlight ? `url(#${uid}-halo)` : undefined}
          style={{
            opacity: isRefreshing && animatesActivity ? 0.3 : 1,
            filter: highlight ? `drop-shadow(0 0 ${HALO_RADIUS}px ${hexAlpha(arcColour, 0.42)})` : undefined,
            transition: 'stroke-dasharray 0.35s ease-out',
          }}
        />
      )}

      {/* 3. Icon disc + glyph */}
      {centreDiameter > 0 && (
        <>
          <circle cx={0} cy={0} r={centreDiameter / 2} className="ring__disc" />
          {icon && (
            <foreignObject
              x={-iconDiameter / 2}
              y={-iconDiameter / 2}
              width={iconDiameter}
              height={iconDiameter}
              style={{ opacity: hasReading || showsBotMark ? 1 : 0.35, overflow:'visible' }}
            >
              <div className="ring__icon">{icon}</div>
            </foreignObject>
          )}
        </>
      )}

      {/* 4. Second ring — same colour language, thinner, inside */}
      {secondDrawn && (
        <>
          <circle
            cx={0}
            cy={0}
            r={secondRingRadius}
            fill="none"
            stroke={`rgba(255,255,255,${TRACK_OPACITY})`}
            strokeWidth={secondRingLineWidth}
          />
          {secondArcFraction > 0 && (
            <circle
              cx={0}
              cy={0}
              r={secondRingRadius}
              fill="none"
              stroke={secondColour}
              strokeWidth={secondRingLineWidth}
              strokeLinecap="round"
              strokeDasharray={dash(secondArcFraction, secondRingRadius)}
              transform="rotate(-90)"
            />
          )}
        </>
      )}

      {/* 5. Activity mark — white, reserved for CLI work; starts at 3 o'clock */}
      {showActivity && busyDiameter > 0 && (
        <circle
          cx={0}
          cy={0}
          r={busyDiameter / 2}
          fill="none"
          stroke="#ffffff"
          strokeWidth={Math.max(lineWidth * 0.5, 1.5)}
          strokeLinecap="round"
          strokeDasharray={dash(BUSY_SWEEP, busyDiameter / 2)}
          className="ring__spin"
          style={{ animationDuration: `${BUSY_PERIOD}s`, transformOrigin: '0px 0px' }}
        />
      )}

      {/* Refresh mark — the usage colour, over the track and the arc alike.
          Rotation about `(0,0)` is a spin around the ring: the viewBox is
          centred on the ring's centre, so no transform-origin maths is needed. */}
      {showRefresh && (
        <g transform="rotate(-90)">
          <circle
            cx={0}
            cy={0}
            r={ringRadius}
            fill="none"
            stroke={arcColour}
            strokeWidth={lineWidth}
            strokeLinecap="round"
            strokeDasharray={dash(REFRESH_SWEEP, ringRadius)}
            className="ring__spin"
            style={{ animationDuration: `${REFRESH_PERIOD}s`, transformOrigin: '0px 0px' }}
          />
        </g>
      )}

      {/* 6. Window clock — outside the usage ring, neutral, never a second hue */}
      {windowClockFraction !== null && windowClockFraction !== undefined && (
        <>
          <circle
            cx={0}
            cy={0}
            r={clockDiameter / 2}
            fill="none"
            stroke={`rgba(255,255,255,${CLOCK_TRACK_OPACITY})`}
            strokeWidth={CLOCK_LINE_WIDTH * scale}
          />
          {clamp01(windowClockFraction) > 0 && (
            <circle
              cx={0}
              cy={0}
              r={clockDiameter / 2}
              fill="none"
              stroke={`rgba(255,255,255,${CLOCK_ARC_OPACITY})`}
              strokeWidth={CLOCK_LINE_WIDTH * scale}
              strokeLinecap="round"
              strokeDasharray={dash(clamp01(windowClockFraction), clockDiameter / 2)}
              transform="rotate(-90)"
            />
          )}
        </>
      )}
    </svg>
  )
}

function clamp01(v: number): number {
  return Math.max(0, Math.min(1, v))
}

/** `#RRGGBB` → `rgba(r,g,b,a)`, for the halo's drop shadow. */
function hexAlpha(hex: string, alpha: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex)
  if (!m) return hex
  const value = parseInt(m[1], 16)
  const r = (value >> 16) & 0xff
  const g = (value >> 8) & 0xff
  const b = value & 0xff
  return `rgba(${r},${g},${b},${alpha})`
}

export const UsageRing = memo(UsageRingImpl)
