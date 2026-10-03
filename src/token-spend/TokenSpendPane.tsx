import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { beginSpendScan, cancelSpendScan, clearSpendSnapshot, getSpendScan } from './api'
import { agentName, formatCost, shortTokens, sortDays, summarize } from './summary'
import type { SpendAmounts, SpendGroup, SpendSnapshot, SpendSpan } from './types'
import './token-spend.css'

// A completed result belongs to this Settings webview. Sidebar round trips do
// not discover stores again. Closing this webview naturally releases this copy.
let retainedSnapshot: SpendSnapshot | null = null
let activeScanId: string | null = null
/** Settings close/disable can release the backend task even on another pane. */
export function releaseSpendSession() {
  const hadSession = retainedSnapshot !== null || activeScanId !== null
  retainedSnapshot = null; activeScanId = null
  if (hadSession) void clearSpendSnapshot().catch(() => undefined)
}

export interface TokenSpendPaneProps {
  enabled: boolean
  onEnabledChange: (enabled: boolean) => void
  span?: SpendSpan
  onSpanChange?: (span: SpendSpan) => void
}
const spans: [SpendSpan, string][] = [['today', '今天'], ['week', '最近 7 天'], ['month', '最近 30 天']]
// This list is about actual readers, not provider registrations. Antigravity's
// original catalog uses separate labels for CLI/IDE native stores and capture.
const pendingSources = ['Grok Build','Kimi CLI','Devin','Pi','Oh My Pi','OmO Native','Kimchi','Prime Agent','Amp','Droid','OpenClaw','Roo Code','Kilo Code','Cline','CodeBuddy','WorkBuddy','Cherry Studio','Command Code','OpenCodeReview','ZCode','Hermes','Goose','Zed','Kiro','Crush','Unsloth','Antigravity CLI','Antigravity IDE','Devin Desktop','Mux','Codebuff','Freebuff','JCode','Augment','Gajae Code','Junie','Fx','LM Studio','Reasonix','Trae','Warp','Copilot']
const exportHelp: Record<string, string> = {
  dsh: '自动读取 DeepSeek Harness 的 sessions 会话目录（默认 ~/.dsh，可通过 DSH_HOME 指定）。支持普通、版本化及 Zstandard 压缩 JSONL，分别统计输入、输出和缓存；分叉历史与迁移副本去重。CLI 与当前桌面版共享此目录。',
  cursor: '需要事先导出 Cursor usageEventsDisplay JSON 或带四类计数列的 CSV。不会读取 Cursor 原生日志或自动登录；CSV 只用于日统计。',
  antigravity: '需要同步工具事先生成语言服务器用量缓存 JSONL；Antigravity CLI 与 IDE 的原生数据库读取均未移植。不会自行连接或同步。',
  hindsight: '需要事先镜像服务 llm-requests 为 JSONL 用量账本；不会直接访问服务。',
  mcode: '需要事先捕获 mcode exec --output-format stream-json 输出为 JSONL；不会运行客户端。',
}

export function TokenSpendPane({ enabled, onEnabledChange, span: savedSpan, onSpanChange }: TokenSpendPaneProps) {
  const [localSpan, setLocalSpan] = useState<SpendSpan>('week')
  const span = savedSpan && spans.some(([id]) => id === savedSpan) ? savedSpan : localSpan
  const [snapshot, setSnapshot] = useState<SpendSnapshot | null>(retainedSnapshot)
  const [loading, setLoading] = useState(false), [progress, setProgress] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [agent, setAgent] = useState<string | null>(null), [model, setModel] = useState<string | null>(null)
  const generation = useRef(0), mounted = useRef(true), scanId = useRef<string | null>(null)
  const [dayPage, setDayPage] = useState(0), [modelPage, setModelPage] = useState(0), [sessionPage, setSessionPage] = useState(0)
  const [sortKey, setSortKey] = useState<'day' | 'tokens' | 'cost'>('day'), [descending, setDescending] = useState(true)
  const summary = useMemo(() => summarize(snapshot?.records ?? [], span, new Date(), agent, model), [snapshot, span, agent, model])
  const sortedDays = useMemo(() => sortDays(summary.days, sortKey, descending), [summary.days, sortKey, descending])
  const start = useCallback(async (force: boolean) => {
    if (!enabled) return
    const turn = ++generation.current
    retainedSnapshot = null; setSnapshot(null); setError(null); setLoading(true); setProgress('正在准备读取…')
    try {
      const id = await beginSpendScan(true, force)
      if (!mounted.current || generation.current !== turn) { await cancelSpendScan(id); return }
      scanId.current = id; activeScanId = id
      for (;;) {
        const result = await getSpendScan(id)
        if (!mounted.current || generation.current !== turn) return
        if (result.status === 'completed' && result.snapshot) {
          retainedSnapshot = result.snapshot; setSnapshot(result.snapshot); setLoading(false); scanId.current = null; activeScanId = null; return
        }
        if (result.status === 'error') throw new Error(result.error ?? '读取失败')
        if (result.status === 'cancelled') { setLoading(false); scanId.current = null; activeScanId = null; return }
        setProgress(result.sourceIndex > 0 ? `正在读取 ${result.currentSource ?? ''} · ${result.sourceIndex} / ${result.sourceCount}` : '正在获取 models.dev 公开价格…')
        await new Promise(resolve => setTimeout(resolve, 200))
        if (!mounted.current || generation.current !== turn) return
      }
    } catch (reason) {
      if (mounted.current && generation.current === turn) { setError(String(reason)); setLoading(false); scanId.current = null; activeScanId = null }
    }
  }, [enabled])
  useEffect(() => {
    mounted.current = true
    if (enabled) { if (retainedSnapshot) setSnapshot(retainedSnapshot); else void start(false) }
    else { generation.current++; releaseSpendSession(); setSnapshot(null); setLoading(false); setAgent(null); setModel(null) }
    return () => {
      mounted.current = false; generation.current++
      if (scanId.current) { const id = scanId.current; scanId.current = null; activeScanId = null; void cancelSpendScan(id).catch(() => undefined) }
    }
  }, [enabled, start])
  useEffect(() => { setDayPage(0); setModelPage(0); setSessionPage(0) }, [span, agent, model, snapshot])
  const stop = () => {
    generation.current++; setLoading(false); setProgress('读取已停止；未完成的结果没有保存')
    if (scanId.current) { void cancelSpendScan(scanId.current).catch(() => undefined); scanId.current = null; activeScanId = null }
  }
  const changeSpan = (next: SpendSpan) => { setLocalSpan(next); onSpanChange?.(next) }
  const sort = (key: typeof sortKey) => { if (sortKey === key) setDescending(!descending); else { setSortKey(key); setDescending(true) } setDayPage(0) }
  return <section className="token-spend-pane" aria-labelledby="token-spend-title">
    <h1 id="token-spend-title">Token 花费</h1>
    <p className="page-description">从本机记录查看 Token 去向，按公开 API 价格估算金额。</p>
    <div className="spend-opt-in">
      <div><strong>读取本机用量记录</strong><p>默认关闭。开启后读取 Claude Code、Codex、Qwen Code、Gemini CLI、DeepSeek Harness 等工具的本机计数及已有导出文件，并获取 models.dev 公开价格；不保存对话正文。</p></div>
      <input role="switch" aria-label="读取本机 Token 用量记录" type="checkbox" checked={enabled} onChange={event => onEnabledChange(event.target.checked)} />
    </div>
    {enabled && <>
      <div className="spend-toolbar">
        <div className="spend-span" role="group" aria-label="统计时间范围">{spans.map(([value, label]) => <button key={value} aria-pressed={span === value} className={span === value ? 'active' : ''} onClick={() => changeSpan(value)}>{label}</button>)}</div>
        <button disabled={loading} onClick={() => void start(true)}>重新扫描</button>
      </div>
      {loading && <div className="spend-loading" role="status"><span className="spend-spinner" />{progress}<button onClick={stop}>停止读取</button></div>}
      {error && <div className="settings-alert" role="alert">{error}<button onClick={() => void start(true)}>重试</button></div>}
      {!loading && !snapshot && !error && <p className="spend-empty">{progress || '尚未读取数据。'}<button onClick={() => void start(false)}>开始读取</button></p>}
      {snapshot && <>
        {(agent || model) && <div className="spend-breadcrumb"><button onClick={() => model ? setModel(null) : setAgent(null)}>← 返回{model ? '模型列表' : '总览'}</button><span>{agent ? agentName(agent) : '全部 Agent'}{model ? ` / ${model}` : ''}</span></div>}
        {model && <h2>{model}</h2>}
        {agent && !model && <h2>{agentName(agent)}</h2>}
        <div className="spend-headline">
          <Metric label="Token 总量" value={shortTokens(summary.tokens)} help={summary.tokens.toLocaleString('zh-CN')} />
          <Metric label={summary.unpricedTokens && summary.cost !== null ? '部分 API 金额估算' : 'API 金额估算'} value={formatCost(summary.cost)} help={summary.cost === null ? '无公开价格' : `US$${summary.cost.toPrecision(8)}`} />
          <Metric label="活跃天数" value={`${summary.activeDays}`} />
          <Metric label="最活跃时段" value={summary.peakHour === null ? '—' : `${summary.peakHour}:00`} />
        </div>
        {summary.unpricedTokens > 0 && <p className="spend-notice">{shortTokens(summary.unpricedTokens)} Token 无法计价，已计入 Token 总量；可能缺少公开价格或 Token 分类，金额估算未包含这部分。</p>}
        {snapshot.notes.length > 0 && <details className="spend-notice"><summary>计数可能不完整 · {snapshot.notes.length} 条读取说明</summary>{snapshot.notes.map((note, index) => <p key={index}>{note}</p>)}</details>}
        {summary.tokens === 0 && <p className="spend-empty">此时间范围内没有读到 Token 用量。可以扩大时间范围或重新扫描。</p>}
        <div className="spend-charts"><Chart title="每日 Token" labels={summary.days.map(d => d.day)} values={summary.days.map(d => d.tokens)} />{summary.hasAggregateTiming ? <section className="spend-card spend-chart"><h2>小时分布</h2><p className="spend-empty">此范围包含仅能用于日统计的导出记录，无法确定请求时段。小时分布与最活跃时段不可用。</p></section> : <Chart title="小时分布" labels={summary.hours.map((_, hour) => `${String(hour).padStart(2,'0')}:00`)} values={summary.hours} />}</div>
        <section className="spend-card"><h2>Token 分类</h2><div className="spend-splits">
          {([['新输入',summary.tally.input,0],['输出',summary.tally.output,1],['缓存写入',summary.tally.cacheWrite,2],['缓存读取',summary.tally.cacheRead,3]] as const).map(([name,value,index]) => <div key={name}><span>{summary.unclassifiedTokens > 0 ? '已分类 · ' : ''}{name}</span><strong title={summary.unclassifiedTokens === summary.tokens && summary.tokens > 0 ? '来源仅报告总量，分类不可用' : value.toLocaleString('zh-CN')}>{summary.unclassifiedTokens === summary.tokens && summary.tokens > 0 ? '—' : shortTokens(value)}</strong><small>{formatCost(summary.costBreakdown?.[index] ?? null)}</small></div>)}
        </div>{summary.unclassifiedTokens > 0 && <p className="spend-footnote">未分类：{shortTokens(summary.unclassifiedTokens)} Token。来源报告了总量但不能证明各分类；计入总量，不计价。</p>}{summary.unpricedTokens > 0 && summary.cost !== null && <p className="spend-footnote">分类金额覆盖可计价部分。</p>}</section>
        {!model && <section className="spend-card"><h2>模型</h2>{summary.models.length ? <>
          <GroupRows groups={summary.models.slice(modelPage * 8, (modelPage + 1) * 8)} onOpen={group => setModel(group.id)} />
          <Pagination page={modelPage} total={summary.models.length} size={8} onChange={setModelPage} />
        </> : <p className="spend-empty">此范围没有模型用量。</p>}</section>}
        {(!agent || model) && <section className="spend-card"><h2>{model ? 'Agent 贡献' : 'Agent'}</h2><GroupRows groups={summary.agents} onOpen={group => { setAgent(group.id); if (!model) setModel(null) }} /></section>}
        <section className="spend-card"><h2>每日明细</h2><div className="spend-table-scroll"><table className="spend-table"><thead><tr>
          {([['day','日期'],['tokens','Token'],['cost','API 金额估算']] as const).map(([key,label]) => <th key={key} aria-sort={sortKey === key ? descending ? 'descending' : 'ascending' : 'none'}><button onClick={() => sort(key)}>{label}{sortKey === key ? descending ? ' ↓' : ' ↑' : ''}</button></th>)}
        </tr></thead><tbody>{sortedDays.slice(dayPage * 10, (dayPage + 1) * 10).map(day => <tr key={day.day}><td>{day.day}</td><td title={day.tokens.toLocaleString('zh-CN')}>{shortTokens(day.tokens)}</td><td title={day.cost === null ? '金额不可用' : `US$${day.cost.toPrecision(8)}`}>{formatCost(day.cost)}{day.unpricedTokens > 0 && day.cost !== null ? ' *' : ''}</td></tr>)}</tbody></table></div><Pagination page={dayPage} total={sortedDays.length} size={10} onChange={setDayPage} /></section>
        {!model && <><section className="spend-card"><h2>会话 · {summary.sessions.length}</h2><GroupRows groups={summary.sessions.slice(sessionPage * 8, (sessionPage + 1) * 8)} /><Pagination page={sessionPage} total={summary.sessions.length} size={8} onChange={setSessionPage} /><p className="spend-footnote">会话按本机记录标识区分，不读取对话标题或正文。</p></section>
          <section className="spend-card"><h2>项目</h2><GroupRows groups={summary.projects} /><p className="spend-footnote">仅显示记录中明确写出的项目路径；缺少路径的会话仍计入总量。</p></section></>}
        <section className="spend-card spend-sources"><h2>读取来源</h2>{snapshot.sources.map(source => <div key={source.id}><strong>{source.name}{source.origin === 'export' ? ' · 导出 / 捕获' : ''}</strong><span>{source.status === 'counted' ? `${source.files} 个记录文件 · ${source.cachedFiles} 个缓存命中` : source.status === 'no-data' ? '未读取到用量' : source.origin === 'export' ? '尚无导出 / 捕获文件' : '未检测到记录目录'}</span></div>)}
          <details><summary>支持格式与读取位置 · {snapshot.sources.length} 个实际 reader</summary>{snapshot.sources.map(source => <section key={source.id}><strong>{source.name}</strong>{exportHelp[source.id] && <p>{exportHelp[source.id]}</p>}{source.roots?.map(root => <p key={root}>{root}</p>)}</section>)}<p>UsageImports 文件夹需要自行放入符合格式的导出；不会扫描其他任意文件夹。</p></details>
          <details><summary>其他 {pendingSources.length} 个原版来源尚未移植</summary><p>{pendingSources.join('、')}</p><p>这些来源未扫描，未被算作已覆盖，也未以零用量代替读取。</p></details>
        </section>
        <p className="spend-footnote">本机记录按 models.dev 公布的 API 价格估算，货币为美元。这不是订阅账单，其他设备的工作不会自动计入。</p>
        <p className="spend-footnote">扫描时间：{new Date(snapshot.scannedAt).toLocaleString('zh-CN')} · {snapshot.pricingStatus === 'unavailable' ? '公开价格暂不可用' : snapshot.pricingStatus === 'stale' ? '使用离线价格缓存' : '价格来源：models.dev'}{snapshot.pricesAt ? ` · ${new Date(snapshot.pricesAt).toLocaleDateString('zh-CN')}` : ''}</p>
      </>}
    </>}
  </section>
}
function Metric({ label, value, help }: { label: string; value: string; help?: string }) { return <div><span>{label}</span><strong title={help}>{value}</strong></div> }
function GroupRows({ groups, onOpen }: { groups: SpendGroup[]; onOpen?: (group: SpendGroup) => void }) {
  return <div className="spend-groups">{groups.map(group => <div key={group.id} className="spend-group-row">{onOpen ? <button className="spend-link" onClick={() => onOpen(group)} title={group.name}>{group.name}<span aria-hidden="true"> ›</span></button> : <span className="spend-group-name" title={group.name}>{group.name}</span>}<strong title={group.tokens.toLocaleString('zh-CN')}>{shortTokens(group.tokens)}</strong><Amount value={group} /></div>)}{!groups.length && <p className="spend-empty">没有读到用量。</p>}</div>
}
function Amount({ value }: { value: SpendAmounts }) { return <span title={value.cost === null ? '没有公开价格' : `US$${value.cost.toPrecision(8)}${value.unpricedTokens ? ' · 部分金额估算' : ''}`}>{formatCost(value.cost)}{value.cost !== null && value.unpricedTokens > 0 ? ' *' : ''}</span> }
function Pagination({ page, total, size, onChange }: { page: number; total: number; size: number; onChange: (page: number) => void }) {
  const pages = Math.ceil(total / size); if (pages <= 1) return null
  return <div className="spend-pagination"><button aria-label="上一页" disabled={page === 0} onClick={() => onChange(page - 1)}>←</button><span>{page + 1} / {pages}</span><button aria-label="下一页" disabled={page + 1 >= pages} onClick={() => onChange(page + 1)}>→</button></div>
}
function Chart({ title, labels, values }: { title: string; labels: string[]; values: number[] }) {
  const [hover, setHover] = useState<number | null>(null)
  const max = Math.max(...values, 1)
  useEffect(() => setHover(null), [labels, values])
  const position = hover === null ? 0 : (hover + 0.5) / values.length * 100
  return <section className="spend-card spend-chart"><h2>{title}</h2><div className="spend-chart-plot" onMouseMove={event => { const box = event.currentTarget.getBoundingClientRect(); setHover(Math.min(values.length - 1, Math.max(0, Math.floor((event.clientX - box.left) / box.width * values.length)))) }} onMouseLeave={() => setHover(null)}>
    {values.map((value, index) => <button key={labels[index]} className="spend-bar-slot" aria-label={`${labels[index]}：${value.toLocaleString('zh-CN')} Token`} onFocus={() => setHover(index)} onBlur={() => setHover(null)} onMouseEnter={() => setHover(index)}><span className="spend-bar" style={{ height: `${value / max * 100}%` }} /></button>)}
    {hover !== null && <><div className="spend-chart-guide" style={{ left: `${position}%` }} /><div className={`spend-chart-tooltip ${position > 65 ? 'at-right' : ''}`} style={{ left: `${position}%` }}>{labels[hover]}<strong>{shortTokens(values[hover])} Token</strong></div></>}
  </div><div className="spend-chart-axis"><span>{labels[0]}</span><span>{labels[Math.floor(labels.length / 2)]}</span><span>{labels[labels.length - 1]}</span></div></section>
}
