import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { SpendSpan } from './token-spend/types'
import type { CardPart } from './panel/cardParts'

export interface AccountAppearance {
  /** Retained for older versions. The card defaults to all eligible sections. */
  detailedCard: boolean
  /** Missing/null uses the defaults; an empty list keeps only core limits. */
  cardParts?: CardPart[] | null
  animatedMark: boolean
  robotFigure?: string | null
  robotMaterial?: 'flat' | 'fluffy'
  /** Kept when loading older profiles. New robots choose a figure separately. */
  persona: string
  body: string
  markColour: string | null
  ringColour: string | null
}
export const defaultAccountAppearance: AccountAppearance = { detailedCard: false, animatedMark: false, persona: 'automatic', body: 'blob', markColour: null, ringColour: null }
export interface NetworkProxySettings {
  mode: 'system' | 'none' | 'manual'
  kind: 'http' | 'socks5'
  host: string
  port: number | null
}
export const defaultNetworkProxy: NetworkProxySettings = { mode: 'system', kind: 'http', host: '', port: null }
/** The three routes the original offers for Antigravity's figures. */
export type AntigravityUsageSource = 'auto' | 'cli' | 'desktop'
/** MiniMax's two storefronts: one service, one key each. */
export type MiniMaxSite = 'international' | 'china'
export interface Preferences {
  networkProxy: NetworkProxySettings
  panelVisible: boolean
  trayShowsUsage: boolean
  trayStyle: 'figure' | 'ring' | 'split'
  trayAccount: string | null
  accounts: {id:string;provider:string}[]
  accountLabels: Record<string,string>
  openSettingsShortcut: string | null
  togglePanelShortcut: string | null
  alerts: { threshold: number | null; onReset: boolean; onFailure: boolean; lowBalance: Record<string, number>; onServiceOutage: boolean;onRecap:boolean }
  tokenSpendEnabled: boolean
  /** The nine spans the original offers: rolling, calendar, open-ended, own. */
  tokenSpendSpan: SpendSpan
  /** Two `YYYY-MM-DD` days for the custom span; empty until the reader picks. */
  spendCustomDays: string[]
  spendActivityView: 'daily'|'weekly'|'cumulative'
  recapMonthlyPrice:number|null
  recapHidesProjects:boolean
  resetCelebration: boolean
  enabledProviders: string[]
  providerOrder: string[]
  panelColour: 'black' | 'light'
  panelSize: number
  railSpacing: number
  roundEnds: boolean
  /** How limit bars are drawn: continuous fills or discrete ticks. */
  usageBarStyle: 'continuous' | 'segmented'
  /** Experimental liquid tether while pulling a docked rail off its edge. */
  liquidDocking: boolean
  showsRemaining: boolean
  showPercentages: boolean
  labelAbove: boolean
  autoCollapse: boolean
  warningThreshold: number
  refreshSeconds: number
  refreshAutomatic: boolean
  showResetClock: boolean
  clockRemaining: boolean
  showSecondRing: boolean
  topShowPercentages: boolean
  animateActivity: boolean
  dockAlertColour: boolean
  showForecast: boolean
  hideInFullscreen: boolean
  followActiveDisplay: boolean
  /**
   * Keep the panel out of screenshots and recordings.
   *
   * A request, not a guarantee: Windows honours it for the capture APIs that
   * ask, and a tool that reads the screen buffer directly can still include
   * the panel. Off by default, because it hides the panel from your own
   * pictures of it too.
   */
  hideFromCapture: boolean
  accountAppearance: Record<string, AccountAppearance>
  pinnedWindows: Record<string, string>
  /** Which route reads Antigravity: the app it runs, its `agy` CLI, or the app
   *  with the CLI as the fallback. An explicit field rather than a map keyed by
   *  provider, so the choice survives the account being switched off. */
  antigravityUsageSource: AntigravityUsageSource
  /** Accounts drawn as one rail slot per model group. Off for everyone by
   *  default; only a provider that reports more than one group can be split. */
  splitAccounts: string[]
  /** The groups a split account draws, keyed by account id. A missing entry is
   *  every group the reading carries. */
  shownModelGroups: Record<string, string[]>
  /** Which MiniMax storefront the account buys from. The two sites are one
   *  service with two keys, so this is a setting on one provider rather than a
   *  provider of its own — which is what `minimax-cn` used to be. */
  minimaxSite: MiniMaxSite
  /** The window starter. Empty means off; it is the one setting that
   *  sends something on the reader's behalf, so nothing is on by default. */
  /** The currency estimates are shown in — one of the seven
   *  `exchange_rates::CURRENCIES`. **Only estimates are converted** (Token
   *  spend, the value estimate, card history, the recap); a balance or a
   *  charge a service reports stays in the currency it gave. */
  displayCurrency: 'usd' | 'cny' | 'hkd' | 'twd' | 'jpy' | 'krw' | 'rub'
  /** Rates the reader typed, per code, which win over the fetched ones. */
  manualExchangeRates: Record<string, number>
  primedProviders: string[]
  primerHours: { start: number; end: number }
  primerRunTimes: Record<string, number>
  primerRunOutcomes: Record<string, string>
}
export const defaultPreferences: Preferences = {
  networkProxy: { ...defaultNetworkProxy },
  panelVisible:true, trayShowsUsage:false, trayStyle:'figure', trayAccount:null,
  accounts: [], accountLabels: {},
  openSettingsShortcut: null, togglePanelShortcut: null,
  alerts: { threshold: null, onReset: false, onFailure: false, lowBalance: {}, onServiceOutage: false,onRecap:false }, tokenSpendEnabled: false, tokenSpendSpan: 'week', spendCustomDays: [], spendActivityView:'daily', recapMonthlyPrice:null,recapHidesProjects:false,resetCelebration: true,
  enabledProviders: [], providerOrder: [], panelColour: 'black', panelSize: 1, railSpacing: 1,
  roundEnds: false, usageBarStyle: 'continuous', liquidDocking: false, showsRemaining: false, showPercentages: true,
  labelAbove: false, autoCollapse: true, warningThreshold: 0.75, refreshSeconds: 120, refreshAutomatic: true,
  showSecondRing: false, topShowPercentages: false, animateActivity: true, dockAlertColour: true, accountAppearance: {},
  showResetClock: false, clockRemaining: false, showForecast: false, hideInFullscreen:true, followActiveDisplay:false, hideFromCapture:false, pinnedWindows: {},
  antigravityUsageSource: 'auto', splitAccounts: [], shownModelGroups: {}, minimaxSite: 'international',
  primedProviders: [], primerHours: { start: 7, end: 23 }, primerRunTimes: {}, primerRunOutcomes: {},
  displayCurrency: 'usd', manualExchangeRates: {},
}
export const savePreferences = (value: Preferences) => invoke<void>('save_preferences', { value })

export function usePreferences() {
  const [preferences, setPreferences] = useState(defaultPreferences)
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let disposed = false
    let off: (() => void) | undefined
    let revision = 0
    void (async () => {
      try {
        off = await listen<Preferences>('preferences-changed', e => {
          revision++
          if (!disposed) { setPreferences({ ...defaultPreferences, ...e.payload }); setLoaded(true) }
        })
        if (disposed) { off(); return }
        const version = revision
        const value = await invoke<Preferences>('get_preferences')
        if (!disposed && version === revision) { setPreferences({ ...defaultPreferences, ...value }); setLoaded(true) }
      } catch (e) { if (!disposed) setError(String(e)) }
    })()
    return () => { disposed = true; off?.() }
  }, [])
  return { preferences, loaded, error }
}

/** How many rings a split account may draw. The original fixes it rather than
 *  counting a reading, so the rail's budget does not move when a reply arrives
 *  one group short. */
export const MODEL_GROUP_BUDGET = 3

/** The shape of a usage window this file needs: real windows carry far more,
 *  but nothing below reads the rest. */
export interface ScopedWindow { id?: string | null; scope?: string | null }

/** The model groups a reading carries, in the order the provider gave them.
 *
 * All or nothing: a reading that mixes scoped and unscoped windows is one
 * account with no groups, because splitting it would drop the unscoped rows. */
export function modelGroups(windows: ScopedWindow[]): string[] {
  if (windows.length === 0) return []
  if (!windows.every(w => typeof w.scope === 'string' && w.scope.length > 0)) return []
  const seen: string[] = []
  for (const window of windows) {
    const scope = window.scope as string
    if (!seen.includes(scope)) seen.push(scope)
  }
  return seen
}

/** Whether an account draws one slot per group. More groups than the budget
 *  keeps the account whole rather than dropping a group nobody would see. */
export function splitsIntoGroups(split: boolean, groups: string[]): boolean {
  return split && groups.length > 1 && groups.length <= MODEL_GROUP_BUDGET
}

/** The groups a split account draws: the reader's choice, filtered to the groups
 *  the reading carries. A choice matching nothing (or no choice at all) draws
 *  every group, so a stale choice cannot leave an account with no rings. */
export function groupsShown(groups: string[], chosen?: string[] | null): string[] {
  if (!chosen) return groups
  const shown = groups.filter(group => chosen.includes(group))
  return shown.length > 0 ? shown : groups
}

/** Whether a group switch can be turned off: the last one drawn cannot, because
 *  that would leave the account with no rings at all. */
export function canHideGroup(groups: string[], shown: string[], group: string): boolean {
  if (!shown.includes(group)) return true
  const drawn = groupsShown(groups, shown)
  return drawn.length > 1
}

/** The rail slot an account draws for a group. Unsplit slots keep the bare
 *  account id so stored order and hover survive; `@` cannot collide with the
 *  `#` used for added accounts. */
export function groupSlotId(account: string, group?: string | null): string {
  return group ? `${account}@${group}` : account
}
