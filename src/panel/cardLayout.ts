import { PANEL_SCALE, type PanelEdge, type PanelSizeName } from './berthShape'
import { historyParts, type CardPart } from './cardParts'

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
  parts: ReadonlySet<CardPart>
  activityHeight: number
  activitySpacing: number
  footnoteHeight: number
  headerLineSpacing: number
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
  /** How the limit bar is drawn; segmented bars are twice as tall. */
  usageBarStyle: 'continuous' | 'segmented'
  headerHeight: number
  titleFontSize: number
  rowFontSize: number
  messageFontSize: number
  footnoteFontSize: number
  headerIconSize: number
  rowTextLineHeight: number
  /** A metric's series in the card, the original's `DetailCardLayout.chartHeight`. */
  metricChartHeight: number
}

/** The ring's outer edge: one circle sets every curve on the panel. */
export const RING_OUTER_RADIUS = (36 + 4) / 2

export function detailCardLayout(size: PanelSizeName = 'standard', parts: ReadonlySet<CardPart> = new Set(), usageBarStyle: 'continuous' | 'segmented' = 'continuous'): DetailCardLayout {
  const s = PANEL_SCALE[size]
  const segmented = usageBarStyle === 'segmented'
  // Budget the selected history blocks at their largest rendered height.
  const historyRows = historyParts.filter(part => parts.has(part)).map(part => part === 'figures' ? 47 : part === 'chart' ? 30 : 14)
  const activityHeight = historyRows.length ? (47 + historyRows.reduce((sum, height) => sum + height + 10, 0)) * s : 0
  return {
    parts, activityHeight, activitySpacing: 10 * s, footnoteHeight: 13 * s, headerLineSpacing: 4 * s,
    width: 250 * s,
    padding: 18 * s,
    cornerRadius: RING_OUTER_RADIUS * s,
    pointerWidth: 20 * s,
    pointerHeight: 40 * s,
    horizontalGap: 8 * s,
    contentSpacing: 14 * s,
    rowInternalSpacing: 7 * s,
    progressBarHeight: (segmented ? 12 : 6) * s,
    usageBarStyle,
    headerHeight: 19 * s,
    titleFontSize: 14 * s,
    rowFontSize: 11.5 * s,
    messageFontSize: 12 * s,
    footnoteFontSize: 11 * s,
    headerIconSize: 16 * s,
    rowTextLineHeight: 14 * s,
    metricChartHeight: 30 * s,
  }
}

/**
 * The card's height for `rows` limits, before the browser measures anything.
 *
 * `metrics` are an extension's figures, which are not limits and have no bar:
 * each is one line of text, and `trends` of them carry the series behind the
 * figure as well. Nothing else changes size — the frame is still worked out
 * before a card opens, so the numbers here are the whole story.
 */
export function detailCardHeight(layout: DetailCardLayout, rows: number, metrics = 0, trends = 0): number {
  const rowHeight =
    layout.rowTextLineHeight * 2 + layout.rowInternalSpacing * 2 + layout.progressBarHeight
    + ['forecast', 'value', 'amounts'].filter(part => layout.parts.has(part as CardPart)).length * (layout.rowTextLineHeight + layout.rowInternalSpacing)
  const children = rows + metrics
  const body = children > 0
    ? rows * rowHeight + metrics * layout.rowTextLineHeight + Math.max(children - 1, 0) * layout.contentSpacing
      + trends * (layout.metricChartHeight + layout.rowInternalSpacing)
    : 0
  const update = layout.parts.has('updated') ? layout.headerLineSpacing + layout.footnoteHeight : 0
  const history = layout.activityHeight > 0 ? layout.contentSpacing + layout.activityHeight : 0
  const extraRows = (Number(layout.parts.has('promptCache')) + Number(layout.parts.has('resetCredits'))) * (layout.contentSpacing + layout.rowTextLineHeight)
  return layout.padding * 2 + layout.headerHeight + update + (body > 0 ? layout.contentSpacing + body : 0) + history + extraRows
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
  refreshError = false,
  metrics = 0,
  trends = 0,
): { width: number; height: number } {
  // A balance is one more line of text in the same column as an extension's
  // figures, so it is budgeted the same way; a refusal instead still needs a
  // message and the connection-settings action.
  const balanceLine = hasBalance && rows === 0 ? 1 : 0
  const content = rows>0 || metrics>0 || balanceLine>0
    ? detailCardHeight(layout, rows, metrics + balanceLine, trends)
    : detailCardHeight(layout,0)+layout.contentSpacing+layout.rowTextLineHeight*3+layout.rowInternalSpacing+24*layout.width/250
  const body = content + (footnote ? layout.contentSpacing + layout.footnoteHeight : 0)
    + (refreshError && (rows>0 || hasBalance) ? layout.contentSpacing + layout.rowTextLineHeight*3 + layout.rowInternalSpacing + 24*layout.width/250 : 0)
  return edge === 'top' || edge === 'bottom'
    ? { width: layout.width, height: body + layout.pointerWidth }
    : { width: layout.width + layout.pointerWidth, height: body }
}
