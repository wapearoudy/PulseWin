import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { SpendSpan } from './token-spend/types'

export interface AccountAppearance {
  detailedCard: boolean
  animatedMark: boolean
  persona: string
  body: string
  markColour: string | null
  ringColour: string | null
}
export const defaultAccountAppearance: AccountAppearance = { detailedCard: false, animatedMark: false, persona: 'automatic', body: 'blob', markColour: null, ringColour: null }
export interface NetworkProxySettings {
  mode: 'system' | 'manual'
  kind: 'http' | 'socks5'
  host: string
  port: number | null
}
export const defaultNetworkProxy: NetworkProxySettings = { mode: 'system', kind: 'http', host: '', port: null }
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
  roundEnds: false, showsRemaining: false, showPercentages: true,
  labelAbove: false, autoCollapse: true, warningThreshold: 0.75, refreshSeconds: 120, refreshAutomatic: true,
  showSecondRing: false, topShowPercentages: false, animateActivity: true, dockAlertColour: true, accountAppearance: {},
  showResetClock: false, clockRemaining: false, showForecast: false, hideInFullscreen:true, followActiveDisplay:false, hideFromCapture:false, pinnedWindows: {},
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
