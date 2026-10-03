import { memo, useMemo, useRef, type CSSProperties } from 'react'
import {
  berthCanvas,
  berthPath,
  berthTransform,
  dockLayout,
  type DockLayout,
  type PanelEdge,
} from './berthShape'
import { percentFigure } from '../usagePresentation'
import { UsageRing } from './UsageRing'
import './panel.css'

/**
 * The dock rail: a column of rings inside `DockBerthShape`, fused to a screen
 * edge.
 *
 * Geometry all comes from `dockLayout()`, which is a port of the original's
 * `DockLayout`. Nothing here invents a length: "anything new on the panel takes
 * size from them".
 */

export interface RailEntry {
  /** Stable key — the original keys by `account@group` so split rings stay apart. */
  id: string
  title: string
  usedFraction: number | null
  /** Reported credit when the account deliberately has no quota window. */
  figure?: string | null
  isSpent?: boolean
  isBusy?: boolean
  isRefreshing?: boolean
  secondFraction?: number | null
  secondIsSpent?: boolean
  windowClockFraction?: number | null
  icon?: React.ReactNode
  showsBotMark?: boolean
  hasReading?: boolean
  chosenTint?: string | null
}

export interface RailProps {
  entries: RailEntry[]
  edge: PanelEdge
  /** 0 = the collapsed sliver, 1 = the full rail. Interpolated, not swapped. */
  openness?: number
  /** Off the edge there is nothing to fuse with, so the flare gives way to a capsule. */
  isDocked?: boolean
  size?: number
  spacing?: number
  usesRoundEnds?: boolean
  /** Which ring is being pointed at. */
  highlightId?: string | null
  warningAt: number
  /** A top rail hides its labels by default: a second line of type under the menu bar turns a compact pill into a banner. */
  showPercentages?: boolean
  /** Whether the label sits above its ring rather than below. */
  labelAboveRing?: boolean
  showsRemaining?: boolean
  animateActivity?: boolean
  dockAlertColour?: boolean
  onContextMenu?: () => void
  onEnter?: (id: string) => void
  onLeave?: () => void
  onClick?: (id: string) => void
  /** The rail has been pressed: hand the drag to the window's owner. */
  onDragStart?: () => void
}

/** Total rail length for `count` slots. */
export function railHeight(layout: DockLayout, count: number, isDocked = true): number {
  const padding = layout.verticalPadding - (isDocked ? 0 : layout.flareHeight)
  if (count <= 0) return padding * 2
  return padding * 2 + count * layout.itemHeight + (count - 1) * layout.itemSpacing
}

function RailImpl({
  entries,
  edge,
  openness = 1,
  isDocked = true,
  size = 1,
  spacing = 1,
  usesRoundEnds = false,
  highlightId = null,
  warningAt,
  showPercentages,
  labelAboveRing = false,
  showsRemaining = false,
  animateActivity=true,
  dockAlertColour=true,
  onContextMenu,
  onEnter,
  onLeave,
  onClick,
  onDragStart,
}: RailProps) {
  const layout = useMemo(
    () => dockLayout({ scale: size, spacing, usesRoundEnds }),
    [size, spacing, usesRoundEnds],
  )

  // Side rails show labels by default; a top rail does not.
  const labels = showPercentages ?? edge !== 'top'

  const height = railHeight(layout, entries.length, isDocked)
  const padding = layout.verticalPadding - (isDocked ? 0 : layout.flareHeight)
  const press = useRef<{x: number; y: number} | null>(null)
  const moved = useRef(false)

  // A top rail is the same rail given a quarter turn: the slots run along the
  // screen instead of down it, so the container's two sides swap and so does
  // the direction the items are laid out in. The *canonical* dims below stay
  // the ones the shape was authored in — thickness by length — because the
  // berth and the bubble are both drawn facing right and turned into place.
  const isTop = edge === 'top'
  const length = height

  // The rail at `openness` is its own silhouette wound down: the frame narrows
  // towards the sliver and the corners follow it down to `collapsedWidth`, so
  // the collapsed state is a capsule 6pt across rather than a second shape.
  const openWidth = layout.collapsedWidth + (layout.width - layout.collapsedWidth) * openness
  const openHeight = layout.collapsedHeight + (length - layout.collapsedHeight) * openness

  const canvas = berthCanvas(edge, openWidth, openHeight)
  // The outline is authored facing right in `openWidth × openHeight` and then
  // turned into place by `transform`; the canvas is only the box it lands in.
  // Handing `berthPath` the canvas instead is what once left a top rail drawn
  // as a 16pt column: the rotation carried the shape clean out of its viewBox.
  const d = berthPath({
    width: openWidth,
    height: openHeight,
    openness,
    isDocked,
    layout,
  })
  const transform = berthTransform(edge, canvas.width, canvas.height)

  // Anchored to the docked edge, so the rail grows away from the edge rather
  // than from its own centre.
  const berthInset: CSSProperties = isTop
    ? { top: 0, left: (length - canvas.width) / 2 }
    : edge === 'right'
      ? { right: 0, top: (length - canvas.height) / 2 }
      : { left: 0, top: (length - canvas.height) / 2 }

  return (
    <div
      className="rail"
      style={{
        width: isTop ? length : layout.width,
        height: isTop ? layout.width : length,
        position: 'relative',
      }}
      onMouseLeave={onLeave}
      onContextMenu={e=>{e.preventDefault();onContextMenu?.()}}
      onPointerDown={e => {
        if(e.button !== 0) return
        if(e.ctrlKey) { e.preventDefault(); onContextMenu?.(); return }
        press.current = {x: e.screenX, y: e.screenY}; moved.current = false
        onDragStart?.()
      }}
      onPointerMove={e => {
        if(press.current && Math.hypot(e.screenX-press.current.x, e.screenY-press.current.y) >= 5) moved.current = true
      }}
      onPointerUp={e => {
        if(press.current && Math.hypot(e.screenX-press.current.x, e.screenY-press.current.y) >= 5) moved.current = true
        press.current = null
      }}
    >
      {/* The rail's own silhouette. `openness` interpolates the flare and the
          corners, so the collapsed sliver is this shape wound down rather than
          a second view faded in. */}
      <svg
        className="rail__berth"
        width={canvas.width}
        height={canvas.height}
        viewBox={`0 0 ${canvas.width} ${canvas.height}`}
        preserveAspectRatio="none"
        aria-hidden="true"
        style={{ position: 'absolute', ...berthInset }}
      >
        <path d={d} transform={transform} className="rail__surface" style={{fill: dockAlertColour && openness < .5 && entries.some(e=>e.isSpent || (e.usedFraction??0)>=warningAt) ? '#FF4F42' : undefined}} />
      </svg>

      <div
        className="rail__items"
        style={{
          position: 'absolute',
          inset: 0,
          display: 'flex',
          flexDirection: isTop ? 'row' : 'column',
          alignItems: 'center',
          justifyContent: 'center',
          gap: layout.itemSpacing,
          // Layout stays at full rail size even when collapsed, so the card's
          // geometry does not change; only the surface is wound down.
          paddingTop: isTop ? 0 : padding,
          paddingBottom: isTop ? 0 : padding,
          paddingLeft: isTop ? padding : 0,
          paddingRight: isTop ? padding : 0,
          opacity: openness,
          pointerEvents: openness < 0.5 ? 'none' : undefined,
        }}
      >
        {entries.map((entry) => {
          const ring = (
            <UsageRing
              key="ring"
              usedFraction={entry.usedFraction}
              showsRemaining={showsRemaining}
              isSpent={entry.isSpent}
              diameter={layout.ringDiameter}
              lineWidth={layout.ringLineWidth}
              animatesActivity={animateActivity}
              isBusy={entry.isBusy}
              isRefreshing={entry.isRefreshing}
              highlight={highlightId === entry.id}
              secondFraction={entry.secondFraction ?? null}
              secondIsSpent={entry.secondIsSpent}
              secondRingDiameter={layout.secondRingDiameter}
              secondRingLineWidth={layout.secondRingLineWidth}
              windowClockFraction={entry.windowClockFraction ?? null}
              warningAt={warningAt}
              scale={size}
              icon={entry.icon}
              showsBotMark={entry.showsBotMark}
              hasReading={entry.hasReading}
              chosenTint={entry.chosenTint}
            />
          )

          const label = labels ? (
            <span key="label" className="rail__percent" style={{ fontSize: layout.percentFontSize, height: layout.percentTextHeight, flexShrink: 0 }}>
              {entry.figure ?? (entry.usedFraction === null || entry.usedFraction === undefined
                ? '—'
                : `${percentFigure(entry.usedFraction, showsRemaining)}%`)}
            </span>
          ) : null

          return (
            <button
              key={entry.id}
              type="button"
              className="rail__item"
              title={entry.title}
              onMouseEnter={() => onEnter?.(entry.id)}
              onClick={e => {
                if(moved.current) return
                const rect = e.currentTarget.getBoundingClientRect()
                const cy = rect.top + (labelAboveRing && labels ? layout.percentTextHeight + layout.ringToTextSpacing : 0) + layout.ringDiameter/2
                if(e.detail === 0 || Math.hypot(e.clientX-(rect.left+rect.width/2), e.clientY-cy) <= layout.ringDiameter/2 + layout.ringLineWidth/2) onClick?.(entry.id)
              }}
              style={{
                width: layout.percentTextWidth + layout.ringLineWidth,
                // Ring + label block, both named budgets.
                height: layout.itemHeight,
                background: 'none',
                border: 'none',
                padding: 0,
                color: 'inherit',
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                gap: layout.ringToTextSpacing,
                cursor: 'pointer',
              }}
            >
              {labelAboveRing ? (
                <>
                  {label}
                  {ring}
                </>
              ) : (
                <>
                  {ring}
                  {label}
                </>
              )}
            </button>
          )
        })}
      </div>
    </div>
  )
}

export const Rail = memo(RailImpl)
