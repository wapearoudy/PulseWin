import { invoke } from '@tauri-apps/api/core'
import type { SpendScan } from './types'
export const beginSpendScan = (enabled: boolean, force = false) => invoke<string>('spend_begin_scan', { enabled, force })
export const getSpendScan = (scanId: string) => invoke<SpendScan>('spend_get_scan', { scanId })
export const cancelSpendScan = (scanId: string) => invoke<void>('spend_cancel_scan', { scanId })
export const clearSpendSnapshot = () => invoke<void>('spend_clear_snapshot')
