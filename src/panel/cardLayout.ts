import { PANEL_SCALE, type PanelEdge, type PanelSizeName } from './berthShape'

/**
 * Port of the original's `DetailCardLayout`.
 *
 * These are budgets, not measurements: the card's frame is worked out from them
 * before anything is laid out inside it, which is why the bar heights and the
 * line heights are named here rather than left to the browser's defaults. The
 * card's *width* scales with the panel, and so does every font in it — the
 * original shipped a version where the width followed `PanelMetrics` and the
 * type did not, and at Small a 205pt card truncated its own title.
 */
export interface DetailCardLayout {
  detailed: boolean
  activityHeight: number
  activitySpacing: number
  footnoteHeight: number
  headerLineSpacing: number
  showForecast: boolean
  width: number
  padding: number
  /** The rings' own curve: the outer edge of a ring's stroke. */
  cornerRadius: number
  pointerWidth: number
  pointerHeight: number
  /** Gap between the pointer's tip and the dock rail. */
  horizontalGap: number
  contentSpacing: number
  rowInternalSpacing: number
  progressBarHeight: number
  headerHeight: number
  titleFontSize: number
  rowFontSize: number
  messageFontSize: number
  footnoteFontSize: number
  headerIconSize: number
  rowTextLineHeight: number
}

/** The ring's outer edge: one circle sets every curve on the panel. */
export const RING_OUTER_RADIUS = (36 + 4) / 2

export function detailCardLayout(size: PanelSizeName = 'standard', showForecast=false, detailed=false): DetailCardLayout {
  const s = PANEL_SCALE[size]
  return {
    detailed, activityHeight: 231 * s, activitySpacing: 10 * s, footnoteHeight: 13 * s, headerLineSpacing: 4 * s,
    showForecast,
    width: 250 * s,
    padding: 18 * s,
    cornerRadius: RING_OUTER_RADIUS * s,
    pointerWidth: 20 * s,
    pointerHeight: 40 * s,
    horizontalGap: 8 * s,
    contentSpacing: 14 * s,
    rowInternalSpacing: 7 * s,
    progressBarHeight: 6 * s,
    headerHeight: 19 * s,
    titleFontSize: 14 * s,
    rowFontSize: 11.5 * s,
    messageFontSize: 12 * s,
    footnoteFontSize: 11 * s,
    headerIconSize: 16 * s,
    rowTextLineHeight: 14 * s,
  }
}

/** The card's height for `rows` limits, before the browser measures anything. */
export function detailCardHeight(layout: DetailCardLayout, rows: number): number {
  const rowHeight =
    layout.rowTextLineHeight * 2 + layout.rowInternalSpacing * 2 + layout.progressBarHeight + (layout.showForecast ? layout.rowTextLineHeight + layout.rowInternalSpacing : 0) + (layout.detailed ? layout.rowTextLineHeight + layout.rowInternalSpacing : 0)
  const body = rows > 0 ? rows * rowHeight + Math.max(rows - 1, 0) * layout.contentSpacing : 0
  return layout.padding * 2 + layout.headerHeight + (layout.detailed ? layout.headerLineSpacing + layout.footnoteHeight : 0) + (body > 0 ? layout.contentSpacing + body : 0) + (layout.detailed ? layout.contentSpacing + layout.activityHeight : 0)
}

/**
 * The card's box **as it lands on screen**, tail included.
 *
 * The outline is authored facing right — body, then the tail out to the right —
 * and a top-docked rail's card is that same outline given a quarter turn, so on
 * screen one of the two sides carries the tail and the other does not. Both the
 * panel's frame budget and the card's own hit region are read from here, which
 * is what stops the frame and the thing drawn in it from drifting apart.
 */
export function cardBox(
  edge: PanelEdge,
  layout: DetailCardLayout,
  rows: number,
  hasBalance = false,
  footnote = false,
): { width: number; height: number } {
  // A refusal still needs a message and the connection-settings action.
  const content = rows>0 ? detailCardHeight(layout, rows) : hasBalance
    ? detailCardHeight(layout,0)+layout.contentSpacing+layout.rowTextLineHeight
    : detailCardHeight(layout,0)+layout.contentSpacing+layout.rowTextLineHeight*3+layout.rowInternalSpacing+24*layout.width/250
  const body = content + (footnote ? layout.contentSpacing + layout.footnoteHeight : 0)
  return edge === 'top'
    ? { width: layout.width, height: body + layout.pointerWidth }
    : { width: layout.width + layout.pointerWidth, height: body }
}
