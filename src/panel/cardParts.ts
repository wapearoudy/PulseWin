import type { AccountAppearance } from '../preferences'

export const cardParts = [
  ['plan', '套餐'], ['updated', '更新时间'], ['forecast', '用尽预测'],
  ['value', '估算额度价值'], ['amounts', '额度数值'],
  ['figures', '今天、7 天与 31 天用量'], ['chart', '31 天用量图'],
  ['topModel', '主要模型'], ['cacheHitRate', '缓存命中率'],
  ['promptCache', '提示词缓存'], ['lastStep', '最近一步速度'],
  ['resetCredits', '额度重置券'],
] as const

export type CardPart = typeof cardParts[number][0]
export const historyParts: readonly CardPart[] = ['figures', 'chart', 'topModel', 'cacheHitRate', 'lastStep']

// These ids have account-scoped readers. Additional accounts cannot borrow
// the primary login's local transcript, prompt cache or console ledger.
const localHistory = new Set(['claude-code', 'codex', 'cursor', 'antigravity', 'devin'])
const consoleHistory = new Set(['deepseek', 'opencode-go'])

export function availableCardParts(account: string): CardPart[] {
  const hasLocalHistory = localHistory.has(account)
  const hasHistory = hasLocalHistory || consoleHistory.has(account)
  return cardParts.map(([part]) => part).filter(part => {
    switch (part) {
      case 'plan': case 'updated': case 'forecast': return true
      case 'value': return hasLocalHistory
      case 'figures': case 'chart': return hasHistory || account === 'qoder'
      case 'topModel': case 'cacheHitRate': return hasHistory
      case 'amounts': return account === 'copilot' || account === 'qoder'
      case 'promptCache': return account === 'claude-code'
      case 'lastStep': case 'resetCredits': return account === 'codex'
    }
  })
}

export function accountCardParts(account: string, appearance: AccountAppearance | undefined, defaultForecast: boolean): CardPart[] {
  return availableCardParts(account).filter(part => {
    const selected = appearance?.cardParts
    if (selected != null) return selected.includes(part)
    if (part === 'forecast') return defaultForecast
    return part !== 'amounts' && part !== 'resetCredits'
  })
}

export function cardPartNeedsLocalSpend(account: string, part: CardPart): boolean {
  return localHistory.has(account) && (part === 'value' || part === 'promptCache' || historyParts.includes(part))
}

export function visibleCardParts(account: string, appearance: AccountAppearance | undefined, forecast: boolean, readsLocalSpend: boolean): Set<CardPart> {
  return new Set(accountCardParts(account, appearance, forecast).filter(part => readsLocalSpend || !cardPartNeedsLocalSpend(account, part)))
}
