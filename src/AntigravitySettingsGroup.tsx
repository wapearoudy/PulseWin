import { type AntigravityUsageSource, groupsShown } from './preferences'

/// The three routes, in the order the original offers them, with the words the
/// picker shows. `cli` asks the `agy` command; `desktop` asks the language
/// server Antigravity runs while it is open; `auto` tries the app first and the
/// CLI when the app reported no figures.
export const SOURCE_OPTIONS: { value: AntigravityUsageSource; label: string; subtitle: string }[] = [
  {
    value: 'auto',
    label: '自动',
    subtitle: '优先读取兼容的桌面端。新版桌面端的用量通过已登录的 agy CLI 读取。',
  },
  {
    value: 'cli',
    label: 'CLI（agy）',
    subtitle: '使用官方 agy CLI 中已登录的账号读取用量。',
  },
  {
    value: 'desktop',
    label: '桌面端',
    subtitle: '从运行中的兼容桌面端读取。新版桌面端请选择「自动」或「CLI（agy）」。',
  },
]

/// Antigravity's own switches, on its pane.
///
/// **Two routes read the same allowances, and the reader picks one.** The app
/// answers only while it is open; the CLI answers on its own but must be new
/// enough to report usage, which is why "自动" exists and is the default.
///
/// **One ring per model group is off until asked for.** Antigravity reports the
/// Gemini allowance and the Claude/GPT allowance as scopes of one login, so an
/// unsplit account is one ring; switching the split on gives each group its own,
/// and each group can then be left off — except the last one drawn, because an
/// account with no rings at all cannot be opened.
export function AntigravitySettingsGroup({ source, split, groups, shown, disabled, onSource, onSplit, onGroup }: {
  source: AntigravityUsageSource
  split: boolean
  /** The groups the current reading carries, in the provider's order. */
  groups: string[]
  /** The groups the reader chose; empty means every group. */
  shown: string[]
  disabled?: boolean
  onSource: (source: AntigravityUsageSource) => void
  onSplit: (split: boolean) => void
  onGroup: (group: string, on: boolean) => void
}) {
  const current = SOURCE_OPTIONS.find(option => option.value === source) ?? SOURCE_OPTIONS[0]
  const drawn = groupsShown(groups, shown)
  return <section className="group-wrap"><h2>读取用量</h2><div className="settings-group">
    <div className="setting-row"><div><strong>读取用量来源</strong><small>{current.subtitle}</small></div>
      <div className="setting-control">
        <select aria-label="读取用量来源" disabled={disabled} value={current.value}
          onChange={e => onSource(e.target.value as AntigravityUsageSource)}>
          {SOURCE_OPTIONS.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
        </select>
      </div></div>
    <div className="setting-row"><div><strong>每个模型组一个圆环</strong><small>Antigravity 把 Gemini 与 Claude/GPT 的额度分成几组报告。打开后每组各占一个圆环，共用同一个登录，刷新其中一个就刷新全部。</small></div>
      <div className="setting-control"><input type="checkbox" role="switch" aria-label="每个模型组一个圆环" checked={split} disabled={disabled}
        onChange={e => onSplit(e.target.checked)}/></div></div>
    {split && groups.length > 0 && <div className="setting-row"><div><strong>显示哪几组</strong><small>{groups.length > 1
      ? '关闭一组就少一个圆环，这一组也不会出现在面板上。最后一个显示的组不能关闭。'
      : '这份用量只有一组，所以只有一个圆环。'}</small></div>
      <div className="setting-control toolbar">
        {groups.map(group => {
          const on = drawn.includes(group)
          return <label key={group} className="setting-check">
            <input type="checkbox" aria-label={`${group} 的圆环`} checked={on} disabled={disabled || (on && drawn.length === 1)}
              onChange={e => onGroup(group, e.target.checked)}/>
            {group}
          </label>
        })}
      </div></div>}
  </div></section>
}
