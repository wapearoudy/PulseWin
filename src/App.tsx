import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  beginDrag,
  getActivity,
  getResets,
  onResetsChanged,
  onActivityChanged,
  type Activity,
  showPanelMenu,
  onMenuChanged,
  onRefreshChanged,
  panelPlacement,
  onPlacementChanged,
  onDragChanged,
  getSnapshot,
  onEdgeChanged,
  onHoverChanged,
  onUsageUpdated,
  panelEdge,
  refreshNow,
  refreshProvider,
  setHitRegions,
  setPanelMetrics,
} from './api'
import { dockLayout, type PanelEdge } from './panel/berthShape'
import { BotMark } from './panel/botmark/BotMark'
import { quietSince, resolveMood, windowIsSpent } from './panel/botmark/mood'
import { brandColour, deal } from './panel/botmark/tint'
import { railHeight, Rail, type RailEntry } from './panel/Rail'
import { cardBox, detailCardLayout } from './panel/cardLayout'
import { cardSurfaceRegion, cardTrackingRegion, railSurfaceRegion, sliverTrackingRegion, type ShapeHitRegion } from './panel/hitRegions'
import { ProviderIcon } from './panel/ProviderIcon'
import { personaAt } from './panel/botmark/programme'
import { defaultAccountAppearance, usePreferences } from './preferences'
import { clockFraction, headlineWindow, secondWindow } from './usagePresentation'
import { UsageCard } from './panel/UsageCard'
import type { Snapshot } from './types'
import { balanceOnly, creditRailText } from './creditAmount'

/** `PanelMetrics.scale` — `PanelSize.standard` for now. */
/** `RailSpacing.standard`. */

/**
 * How long the rail stays open after the pointer leaves it.
 *
 * A rail that collapses the instant the pointer steps off it cannot be walked
 * down: every gap between two rings would shut it. The original's answer is to
 * sample the pointer and keep the panel open while the pointer is anywhere on
 * it; the grace period here covers the gaps between samples and the trip from a
 * ring to its card.
 */
const COLLAPSE_GRACE_MS = 320

/** Length of the wind-down, in milliseconds. */
const OPENNESS_MS = 180

/**
 * The backend reports percentages as 0..100; the panel's whole colour language
 * is expressed in fractions. Converted once, here, so nothing downstream has to
 * remember which end it is on.
 */
const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v))

export default function App() {
  const [now, setNow] = useState(Date.now)
  useEffect(() => { const timer = window.setInterval(() => setNow(Date.now()), 1000); return () => window.clearInterval(timer) }, [])
  const { preferences } = usePreferences()
  const PANEL_SIZE = preferences.panelSize
  const RAIL_SPACING = preferences.railSpacing
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [refreshingIds, setRefreshingIds] = useState<Set<string>>(new Set())
  const [menuOpen, setMenuOpen] = useState(false)
  const [activity,setActivity]=useState<Activity>({running:[],lastWrite:null,finishedAt:{}})
  const [resets,setResets]=useState<Record<string,number>>({})
  useEffect(()=>{
    let disposed=false,revision=0,off:(()=>void)|undefined
    void (async()=>{
      try {
        off=await onResetsChanged(value=>{revision++;if(!disposed)setResets(value)})
        if(disposed){off();return}
        const version=revision,value=await getResets()
        if(!disposed&&revision===version&&value)setResets(value)
      } catch { /* Preview bridges may not provide reset events. */ }
    })()
    return()=>{disposed=true;off?.()}
  },[])
  useEffect(()=>{
    let disposed=false,revision=0,stop:(()=>void)|undefined
    void (async()=>{
      try {
        stop=await onActivityChanged(value=>{revision++;if(!disposed)setActivity(value)})
        if(disposed){stop();return}
        const version=revision,value=await getActivity();if(!disposed&&revision===version&&value)setActivity(value)
      } catch { /* Older preview harnesses have no activity bridge. */ }
    })()
    return()=>{disposed=true;stop?.()}
  },[])
  /**
   * The region the backend says the pointer is on — `entry:<id>`, `card` or
   * `sliver`. Kept as the raw region id because which *region* is hit and which
   * *ring* is lit are two different questions: the card and the sliver both
   * have to hold the rail open without lighting anything.
   */
  const [region, setRegion] = useState<string | null>(null)
  /** Which edge the rail is fused to. Rust owns this; the rail is drawn from it. */
  const [isDocked, setDocked] = useState(true)
  const [dragging, setDragging] = useState(false)
  const [edge, setEdge] = useState<PanelEdge>('right')
  /** Docked and unattended: the rail winds down to a sliver. */
  const [expanded, setExpanded] = useState(true)
  const openness = useOpenness(expanded)
  const mounted = useRef(true)

  /** The ring the pointer is on, or null when it is on the card or the sliver. */
  const hovered = region && region.startsWith('entry:') ? region.slice('entry:'.length) : null

  /**
   * Whose card is open.
   *
   * Not the same as `hovered`: once the pointer steps off the ring and onto the
   * card the ring is no longer hovered, and a card that closed at that moment
   * could never be read — or clicked.
   */
  const [cardId, setCardId] = useState<string | null>(null)
  useEffect(() => {
    if (hovered !== null) setCardId(hovered)
    else if (region === null || region === 'rail') {
      const timer = window.setTimeout(() => setCardId(null), 320)
      return () => window.clearTimeout(timer)
    }
  }, [hovered, region])

  useEffect(() => {
    mounted.current = true
    let unlisten: (() => void) | undefined
    let unlistenHover: (() => void) | undefined
    let unlistenEdge: (() => void) | undefined
    const extra: (() => void)[] = []
    const keep = (fn: () => void) => mounted.current ? extra.push(fn) : fn()
    panelPlacement().then(p => mounted.current && setDocked(p.dockEdge !== null || !p.railPosition)).catch(() => undefined)
    onPlacementChanged(p => { setDocked(p.dockEdge !== null || !p.railPosition); setCardId(null) }).then(keep)
    onRefreshChanged(event=>setRefreshingIds(old=>{const next=new Set(old);event.ids.forEach(id=>event.refreshing?next.add(id):next.delete(id));return next})).then(keep).catch(()=>undefined)
    onMenuChanged(setMenuOpen).then(keep).catch(() => undefined)
    onDragChanged(d => { setDragging(d); if(d) setCardId(null) }).then(keep)

    getSnapshot()
      .then((s) => mounted.current && setSnapshot(s))
      .catch(() => undefined)

    // The rail may already be docked somewhere other than where it ships: ask
    // before laying anything out, or the frame is measured for the wrong edge.
    panelEdge()
      .then((restored) => {
        if (mounted.current) setEdge(restored)
      })
      .catch(() => undefined)

    onUsageUpdated((s) => {
      if (mounted.current) {
        setSnapshot(s)

      }
    })
      .then((fn) => {
        unlisten = fn
      })
      .catch(() => undefined)

    onHoverChanged((id) => {
      if (!mounted.current) return
      setRegion(id)
    })
      .then((fn) => {
        unlistenHover = fn
      })
      .catch(() => undefined)

    onEdgeChanged((next) => {
      if (mounted.current) setEdge(next)
    })
      .then((fn) => {
        unlistenEdge = fn
      })
      .catch(() => undefined)

    return () => {
      mounted.current = false
      unlisten?.()
      unlistenHover?.()
      unlistenEdge?.()
      extra.forEach(fn => fn())
    }
  }, [])

  // Open on any contact, close only after the pointer has been away a while.
  useEffect(() => {
    if (region !== null || !preferences.autoCollapse || !isDocked || dragging || menuOpen) {
      setExpanded(true)
      return
    }
    const timer = window.setTimeout(() => setExpanded(false), COLLAPSE_GRACE_MS)
    return () => window.clearTimeout(timer)
  }, [region, preferences.autoCollapse, isDocked, dragging, menuOpen])

  const handleRefresh = useCallback(async (provider?: string) => {
    const key=provider??'*'
    setRefreshingIds(old=>new Set([...old,key]))
    try {
      const [next] = await Promise.all([provider ? refreshProvider(provider) : refreshNow(), new Promise(resolve=>setTimeout(resolve,650))])
      setSnapshot(next)
    } catch { /* Provider errors are displayed in the card. */ }
    finally { setRefreshingIds(old=>{const next=new Set(old);next.delete(key);return next}) }
  }, [])

  // A tool the user does not have is not a ring at zero. The original's rule is
  // that disabled providers are not fetched at all; here every provider is
  // asked and the verdict travels with the result, so the rail keeps the ones
  // that are *present* — including one that failed, which is exactly when a
  // reader is looking for its ring — and drops the ones that are not.
  const providers = useMemo(
    () => (snapshot?.providers ?? []).filter((p) => preferences.enabledProviders.includes(p.id))
      .sort((a, b) => preferences.providerOrder.indexOf(a.id) - preferences.providerOrder.indexOf(b.id)),
    [snapshot, preferences.enabledProviders, preferences.providerOrder],
  )

  // A mark's colour is identity, never status — the ring around it is what
  // carries how close a limit is. Dealt across the whole rail at once, so no
  // two neighbouring marks look alike; that is the one thing the mark must not
  // be, because it is the thing that says which ring this is.
  const markColours = useMemo(
    () => deal(providers.map((p) => brandColour(p.id))),
    [providers],
  )

  const entries: RailEntry[] = useMemo(
    () =>
      providers.map((p, index) => {
        const appearance={...defaultAccountAppearance,...preferences.accountAppearance[p.id]}
        const refreshing=refreshingIds.has(p.id)||refreshingIds.has('*')
        const headline = headlineWindow(p, preferences.pinnedWindows[p.id])
        const second = secondWindow(p, preferences.pinnedWindows[p.id])
        const used = headline?.percentUsed
        const hasReading = p.windows.length > 0 || (balanceOnly(p) && !p.error)
        return ({
        id: p.id,
        title: p.error ? `${p.name} — ${p.error}` : p.name,
        usedFraction: used === null || used === undefined ? null : used / 100,
        figure: !headline && !p.error ? creditRailText(p.creditRemaining) : null,
        isSpent: headline?.isExhausted ?? false,
        windowClockFraction: preferences.showResetClock ? clockFraction(headline, now, preferences.clockRemaining) : null,
        isRefreshing: refreshing,
        isBusy: activity.running.includes(p.id),
        // A provider with one limit draws one ring: an empty second ring reads
        // as a limit at zero, or as a fault.
        secondFraction: !preferences.showSecondRing ? null : second?.percentUsed == null ? null : second.percentUsed / 100,
        secondIsSpent: second?.isExhausted ?? false,
        hasReading,
        chosenTint: appearance.ringColour,
        showsBotMark: appearance.animatedMark,
        icon: appearance.animatedMark ? (
          <BotMark
            persona={personaAt(appearance.persona,index)}
            body={appearance.body}
            gaze={edge}
            isPointedAt={cardId===p.id}
            isQuiet={quietSince(activity.lastWrite,now)}
            pointer={region===null?null:undefined}
            finishedAt={activity.finishedAt[p.id]}
            resetAt={resets[p.id]}
            resetCelebrationAllowed={preferences.resetCelebration}
            mood={resolveMood({
              isBusy: activity.running.includes(p.id),
              isRefreshing: refreshing,
              isSpent: windowIsSpent(headline),
              hasReading,
            })}
            colour={appearance.markColour ?? markColours[index] ?? markColours[0] ?? 'oklch(0.78 0.13 0deg)'}
          />
        ) : <ProviderIcon provider={p.id} />,
      })}),
    [providers, refreshingIds, activity, resets, markColours, preferences.accountAppearance, preferences.showSecondRing, preferences.resetCelebration, edge, cardId, region, preferences.pinnedWindows, preferences.showResetClock, preferences.clockRemaining, now],
  )

  // ---- geometry -----------------------------------------------------------
  const activeIndex = dragging ? -1 : entries.findIndex((e) => e.id === cardId)
  const activeProvider = activeIndex >= 0 ? providers[activeIndex] : undefined
  const detailed = !!(activeProvider && preferences.accountAppearance[activeProvider.id]?.detailedCard)
  const hasDetailed = providers.some(p=>preferences.accountAppearance[p.id]?.detailedCard)
  // Every length below is a budget taken from the original's layout constants.
  // Nothing here measures the DOM: the panel's frame has to be known *before*
  // anything is drawn in it, which is what lets a card open without a resize.
  const layout = useMemo(
    () => dockLayout({ scale: PANEL_SIZE, spacing: RAIL_SPACING, usesRoundEnds: preferences.roundEnds }),
    [PANEL_SIZE, RAIL_SPACING, preferences.roundEnds],
  )
  const cardLayout = useMemo(() => detailCardLayout(PANEL_SIZE < 1 ? 'small' : PANEL_SIZE > 1 ? 'large' : 'standard', preferences.showForecast, detailed), [PANEL_SIZE, preferences.showForecast,detailed])
  const maximumCardLayout = useMemo(() => detailCardLayout(PANEL_SIZE < 1 ? 'small' : PANEL_SIZE > 1 ? 'large' : 'standard', preferences.showForecast, hasDetailed), [PANEL_SIZE, preferences.showForecast,hasDetailed])

  const railW = layout.width
  /** The rail's length along the edge it is docked to. */
  const railLength = railHeight(layout, entries.length, isDocked)
  const endPadding = layout.verticalPadding - (isDocked ? 0 : layout.flareHeight)

  // The largest card any provider could open, so the frame never has to grow
  // while one is opening.
  const maxRows = useMemo(
    () => providers.reduce((most, p) => Math.max(most, p.windows.length), 6),
    [providers],
  )
  const tallest = useMemo(() => cardBox(edge, maximumCardLayout, maxRows, false, true, true), [edge, maximumCardLayout, maxRows])

  const isTop = edge === 'top'
  const gap = cardLayout.horizontalGap

  // The frame has to hold the rail plus an open card, on whichever axis the
  // rail was docked: a side rail grows across the screen, a top rail grows down
  // it. Nothing here measures the DOM — the frame is known before anything is
  // drawn in it, which is what lets a card open without a resize.
  const frameW = isTop ? Math.max(railLength, tallest.width) : railW + gap + tallest.width
  const frameH = isTop ? railW + gap + tallest.height : Math.max(railLength, tallest.height)

  const railLeft = isTop ? (frameW - railLength) / 2 : edge === 'right' ? frameW - railW : 0
  const railTop = isTop ? 0 : (frameH - railLength) / 2
  useEffect(() => {
    if (frameW <= 0 || frameH <= 0) return
    setPanelMetrics(frameW, frameH, railLeft, railTop, isTop ? railLength : railW, isTop ? railW : railLength).catch(() => undefined)
  }, [frameW, frameH, railLeft, railTop, railLength, railW, isTop, isDocked])

  /** The `index`-th slot's offset *along* the rail, in frame coordinates. */
  const slotStart = useCallback(
    (index: number) =>
      (isTop ? railLeft : railTop) +
      endPadding +
      index * (layout.itemHeight + layout.itemSpacing),
    [isTop, railLeft, railTop, layout, endPadding],
  )

  const cardSize = useMemo(
    () => cardBox(edge, cardLayout, activeProvider?.windows.length ?? 0, balanceOnly(activeProvider),!!activeProvider?.stale,!!activeProvider?.error),
    [edge, cardLayout, activeProvider],
  )

  /** Where the hovered ring's centre sits, along the rail. */
  const anchor = activeIndex >= 0 ? slotStart(activeIndex) + layout.ringDiameter/2 + (preferences.labelAbove && (isTop ? preferences.topShowPercentages : preferences.showPercentages) ? layout.percentTextHeight + layout.ringToTextSpacing : 0) : 0

  // Along the rail the card is centred on its ring; across it the card sits
  // just off the rail, on the side away from the edge.
  const cardTop = isTop
    ? railW + gap
    : clamp(anchor - cardSize.height / 2, 0, Math.max(0, frameH - cardSize.height))
  const cardLeft = isTop
    ? clamp(anchor - cardSize.width / 2, 0, Math.max(0, frameW - cardSize.width))
    : edge === 'right'
      ? 0
      : railW + gap
  /** Where the tail's tip sits within the card: along the card, or across it. */
  const pointerCentre = isTop ? anchor - cardLeft : anchor - cardTop

  // ---- hit regions --------------------------------------------------------
  // The frame is mostly empty space, so the backend has to be told which parts
  // of it are real; the rest is handed back to the desktop.
  const regions: ShapeHitRegion[] = useMemo(() => {
    const geometry={edge,left:railLeft,top:railTop,width:railW,length:railLength,layout,openness,isDocked}
    // The wider sliver target is tracking forgiveness, not a painted press
    // target. Its transparent band must still pass clicks to the desktop.
    if (!expanded) {
      return [sliverTrackingRegion(geometry),railSurfaceRegion(geometry,'sliver')]
    }

    // Original isOverContent retains the rectangular rail and the card's
    // full cross-window band. PanelSurface.contentShape alone claims presses.
    const out: ShapeHitRegion[] = [{ id: 'rail', hoverOnly:true, x: railLeft, y: railTop, w: isTop ? railLength : railW, h: isTop ? railW : railLength }]
    const card=activeProvider?cardSurfaceRegion({edge,left:cardLeft,top:cardTop,width:cardSize.width,height:cardSize.height,pointerCentre,layout:cardLayout,usesRoundEnds:preferences.roundEnds}):null
    if(card)out.push(cardTrackingRegion(edge,frameW,frameH,card))
    out.push(railSurfaceRegion(geometry))
    entries.forEach((entry, index) => {
      // The whole slot, across the rail's full thickness: a ring is a small
      // hover target. Clip the slot to the actual surface so a transparent
      // capsule corner cannot select an account or intercept a press.
      out.push({
        id: `entry:${entry.id}`,
        clipTo:'rail',
        ...(isTop
          ? { x: slotStart(index), y: railTop, w: layout.itemHeight, h: railW }
          : { x: railLeft, y: slotStart(index), w: railW, h: layout.itemHeight }),
      })
    })
    if(card)out.push(card)
    return out
  }, [
    expanded,
    isTop,
    edge,
    entries,
    railLeft,
    railTop,
    railW,
    railLength,
    layout.itemHeight,
    layout.collapsedHitWidth,
    layout.collapsedHeight,
    slotStart,
    activeProvider,
    cardLeft,
    cardTop,
    cardSize,
    cardLayout,
    pointerCentre,
    preferences.roundEnds,
    frameW,
    frameH,
    openness,
    isDocked,
    layout,
  ])

  useEffect(() => {
    setHitRegions(regions).catch(() => undefined)
  }, [regions])

  // Picking the rail up is the frontend's job — only the webview sees the press.
  // Following the pointer and choosing the edge belong to Rust, which owns the
  // window.
  const handleDragStart = useCallback(() => {
    setExpanded(true)
    beginDrag().catch(() => undefined)
  }, [])

  return (
    <div className="panel-root" style={{ position: 'relative' }}>
      <div
        className="panel__dock"
        style={{
          position: 'absolute',
          top: railTop,
          left: railLeft,
          width: isTop ? railLength : railW,
          height: isTop ? railW : railLength,
        }}
      >
        <Rail
          entries={entries}
          edge={edge}
          isDocked={isDocked}
          size={PANEL_SIZE}
          spacing={RAIL_SPACING}
          usesRoundEnds={preferences.roundEnds}
          showsRemaining={preferences.showsRemaining}
          showPercentages={edge==='top' ? preferences.topShowPercentages : preferences.showPercentages}
          animateActivity={preferences.animateActivity}
          dockAlertColour={preferences.dockAlertColour}
          onContextMenu={() => { void showPanelMenu().catch(() => undefined) }}
          labelAboveRing={preferences.labelAbove}
          openness={openness}
          highlightId={cardId}
          warningAt={preferences.warningThreshold}
          onClick={handleRefresh}
          onDragStart={handleDragStart}
        />
      </div>

      {activeProvider ? (
        <div
          className="panel__card"
          style={{
            position: 'absolute',
            top: cardTop,
            left: cardLeft,
            width: cardSize.width,
            height: cardSize.height,
          }}
        >
          <UsageCard
            provider={activeProvider}
            layout={cardLayout}
            edge={edge}
            pointerCentre={pointerCentre}
            top={0}
            warningAt={preferences.warningThreshold}
            showsRemaining={preferences.showsRemaining}
            usesRoundEnds={preferences.roundEnds}
            tokenSpendEnabled={preferences.tokenSpendEnabled}
            onRetry={handleRefresh}
            refreshing={refreshingIds.has(activeProvider.id) || refreshingIds.has('*')}
          />
        </div>
      ) : null}
    </div>
  )
}

/**
 * `openness` interpolated over `OPENNESS_MS`.
 *
 * The collapsed sliver is the rail's own silhouette wound down — the flare and
 * the corners give way together — so the two are one shape at two values rather
 * than two views swapped over. That makes `openness` a number the frame has to
 * be redrawn against, which means a frame loop rather than a CSS transition.
 */
function useOpenness(open: boolean): number {
  const [value, setValue] = useState(open ? 1 : 0)
  const from = useRef(value)
  const started = useRef(0)

  useEffect(() => {
    from.current = value
    started.current = performance.now()
    let raf = 0

    const step = (now: number) => {
      const t = Math.min(1, (now - started.current) / OPENNESS_MS)
      // Ease-out: the rail has to look like it is settling, not sliding.
      const eased = 1 - (1 - t) * (1 - t)
      setValue(from.current + ((open ? 1 : 0) - from.current) * eased)
      if (t < 1) raf = requestAnimationFrame(step)
    }

    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
    // `value` is deliberately not a dependency: including it would restart the
    // animation on every frame it produces.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  return value
}
