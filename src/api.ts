import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { Snapshot } from './types'

/** Event name emitted by the Rust refresh loop — keep in sync with lib.rs. */
const EVENT_USAGE_UPDATED = 'usage-updated'

/** Event name carrying the id of the region the pointer is on, or null. */
const EVENT_HOVER_CHANGED = 'hover-changed'

/** Event name carrying the edge the rail has just docked to. */
const EVENT_EDGE_CHANGED = 'edge-changed'

/** Which screen edge the rail is fused to. Mirrors `PanelEdge` in lib.rs. */
export type PanelEdgeName = 'left' | 'right' | 'top'

/** One row of the settings list. Mirrors `ProviderSetting` in lib.rs. */
export interface ProviderSetting {
  providerId?: string
  additional?: boolean
  credentialError?: string | null
  id: string
  name: string
  /** The provider's own verdict: this tool is on the machine. */
  configured: boolean
  /** A credential is stored in PulseWin's own file for it. */
  stored: boolean
  credentialPath: string | null
  /** Everything the provider reads, in order. */
  hints: string[]
}

/** A rectangle the panel claims as interactive, in CSS pixels from its origin. */
export interface HitRegion {
  id: string
  x: number
  y: number
  w: number
  h: number
}

export function getSnapshot(): Promise<Snapshot> {
  return invoke<Snapshot>('get_snapshot')
}
export interface Activity { running: string[]; lastWrite: number | null; finishedAt: Record<string,number> }
export const getActivity = () => invoke<Activity>('get_activity')
export const onActivityChanged = (handler:(activity:Activity)=>void) => listen<Activity>('activity-changed',e=>handler(e.payload))
export const getResets = () => invoke<Record<string,number>>('get_resets')
export const onResetsChanged = (handler:(resets:Record<string,number>)=>void) => listen<Record<string,number>>('resets-changed',e=>handler(e.payload))

export function refreshNow(): Promise<Snapshot> {
  return invoke<Snapshot>('refresh_now')
}

export function refreshProvider(provider: string): Promise<Snapshot> {
  return invoke<Snapshot>('refresh_provider', { provider })
}

/** Paths PulseWin reads for a provider, shown when a card says "not detected". */
export function credentialHint(provider: string): Promise<string[]> {
  return invoke<string[]>('credential_hint', { provider })
}

export function onUsageUpdated(handler: (snapshot: Snapshot) => void): Promise<UnlistenFn> {
  return listen<Snapshot>(EVENT_USAGE_UPDATED, (event) => handler(event.payload))
}

/** Hide the panel (the tray keeps the app running). */
export async function hidePanel(): Promise<void> {
  const { getCurrentWindow } = await import('@tauri-apps/api/window')
  await getCurrentWindow().hide()
}

/**
 * Tell the backend which parts of the window are real.
 *
 * The window is deliberately larger than the rail so a card can open inside it
 * without the frame changing, and everything in that margin is empty space. A
 * transparent window still eats clicks, so the backend needs the list to know
 * what to hand back to the desktop.
 */
export function setHitRegions(regions: HitRegion[]): Promise<void> {
  return invoke('set_hit_regions', { regions })
}

/** Ask for a frame big enough to hold the rail plus an open card, and re-dock. */
export function setPanelMetrics(width: number, height: number, railX: number, railY: number, railWidth: number, railHeight: number): Promise<void> {
  return invoke('set_panel_metrics', { width, height, railX, railY, railWidth, railHeight })
}

export function onHoverChanged(handler: (id: string | null) => void): Promise<UnlistenFn> {
  return listen<string | null>(EVENT_HOVER_CHANGED, (event) => handler(event.payload ?? null))
}

/** Ask which edge the rail is docked to. A restored edge is already in force. */
export function panelEdge(): Promise<PanelEdgeName> {
  return invoke<PanelEdgeName>('panel_edge')
}

/** Every provider PulseWin ships, with what it knows about each. Offline. */
export function providerSettings(): Promise<ProviderSetting[]> {
  return invoke<ProviderSetting[]>('provider_settings')
}

/** Open the settings window, from the panel. */
export function openSettings(provider?: string): Promise<void> {
  return invoke('show_settings', { provider: provider ?? null })
}
export const onSettingsAccount = (handler:(provider:string)=>void) => listen<string>('settings-account',e=>handler(e.payload))

/**
 * Store a pasted credential and re-read the tools that can now answer.
 *
 * An empty string is sent as `null`: a provider handed an empty key reports
 * "refused" where the honest answer is "not configured".
 */
export function saveProviderCredential(
  provider: string,
  apiKey: string | null,
  baseUrl: string | null,
): Promise<void> {
  return invoke('save_provider_credential', {
    provider,
    apiKey: apiKey?.trim() ? apiKey.trim() : null,
    baseUrl: baseUrl?.trim() ? baseUrl.trim() : null,
  })
}

export function onEdgeChanged(handler: (edge: PanelEdgeName) => void): Promise<UnlistenFn> {
  return listen<PanelEdgeName>(EVENT_EDGE_CHANGED, (event) => handler(event.payload))
}

/**
 * Pick the rail up.
 *
 * Only the webview sees the button press, but only Rust can follow the pointer
 * and decide which edge the rail lands on — so the press is handed over here
 * and the window is moved from the other side.
 */
export function beginDrag(): Promise<void> {
  return invoke('begin_drag')
}

export interface PanelPlacement { dockEdge: PanelEdgeName | null; railPosition: [number, number] | null }
export const panelPlacement = () => invoke<PanelPlacement>('panel_placement')
export const onPlacementChanged = (handler: (p: PanelPlacement) => void) => listen<PanelPlacement>('placement-changed', e => handler(e.payload))
export const onDragChanged = (handler: (dragging: boolean) => void) => listen<boolean>('drag-changed', e => handler(e.payload))

export const showPanelMenu = () => invoke<void>('show_panel_menu')
export const onMenuChanged = (handler:(open:boolean)=>void) => listen<boolean>('menu-changed',e=>handler(e.payload))

export const onRefreshChanged = (handler:(event:{ids:string[];refreshing:boolean})=>void) => listen<{ids:string[];refreshing:boolean}>('refresh-changed',e=>handler(e.payload))

export const setPanelPosition = (position:'left'|'right'|'top'|'free') => invoke<PanelPlacement>('set_panel_position',{position})

export const addAccount = (provider:string,label:string,credential:string|null,importLocal:boolean,enabled=true) => invoke<string>('add_account',{provider,label,credential,importLocal,enabled})
export const removeAccount = (account:string) => invoke<void>('remove_account',{account})
export const importAccountLogin = (provider:string) => invoke<void>('save_provider_credential',{provider,apiKey:null,baseUrl:null,importLocal:true})
