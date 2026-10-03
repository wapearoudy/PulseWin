import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

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
  alerts: { threshold: number | null; onReset: boolean; onFailure: boolean; lowBalance: Record<string, number> }
  tokenSpendEnabled: boolean
  tokenSpendSpan: 'today' | 'week' | 'month'
  resetCelebration: boolean
  enabledProviders: string[]
  providerOrder: string[]
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
  accountAppearance: Record<string, AccountAppearance>
  pinnedWindows: Record<string, string>
}
export const defaultPreferences: Preferences = {
  networkProxy: { ...defaultNetworkProxy },
  panelVisible:true, trayShowsUsage:false, trayStyle:'figure', trayAccount:null,
  accounts: [], accountLabels: {},
  openSettingsShortcut: null, togglePanelShortcut: null,
  alerts: { threshold: null, onReset: false, onFailure: false, lowBalance: {} }, tokenSpendEnabled: false, tokenSpendSpan: 'week', resetCelebration: true,
  enabledProviders: [], providerOrder: [], panelSize: 1, railSpacing: 1,
  roundEnds: false, showsRemaining: false, showPercentages: true,
  labelAbove: false, autoCollapse: true, warningThreshold: 0.75, refreshSeconds: 120, refreshAutomatic: true,
  showSecondRing: false, topShowPercentages: false, animateActivity: true, dockAlertColour: true, accountAppearance: {},
  showResetClock: false, clockRemaining: false, showForecast: false, hideInFullscreen:true, followActiveDisplay:false, pinnedWindows: {},
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
