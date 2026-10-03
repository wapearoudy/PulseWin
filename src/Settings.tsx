import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { getSnapshot, onUsageUpdated, onSettingsAccount, panelPlacement, onPlacementChanged, setPanelPosition, providerSettings, refreshProvider, saveProviderCredential, type ProviderSetting } from './api'
import { usePreferences, savePreferences, defaultAccountAppearance, type Preferences } from './preferences'
import { formatReset, type ProviderUsage, type Snapshot } from './types'
import { percentFigure } from './usagePresentation'
import { creditText } from './creditAmount'
import { ProviderIcon } from './panel/ProviderIcon'
import { BotMark } from './panel/botmark/BotMark'
import { PERSONAS, PERSONA_LABELS, personaAt } from './panel/botmark/programme'
import { SHAPE_ORDER, SHAPE_LABELS } from './panel/botmark/data'
import { brandColour } from './panel/botmark/tint'
import './settings.css'
import { NotificationsPane, LowBalanceAlert } from './NotificationsPane'
import { GeneralPane } from './GeneralPane'
import { TokenSpendPane, releaseSpendSession } from './token-spend/TokenSpendPane'

export default function Settings() {
  const { preferences: prefs, loaded, error: loadError } = usePreferences()
  const [providers, setProviders] = useState<ProviderSetting[]>([])
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null)
  const [pane, setPane] = useState(()=>new URLSearchParams(window.location.search).get('account') ?? 'appearance'), [query, setQuery] = useState('')
  useEffect(()=>{let live=true,off:(()=>void)|undefined;void onSettingsAccount(id=>{if(live){setPane(id);setChoose(false);setDismissed(true)}}).then(fn=>{if(live)off=fn;else fn()}).catch(()=>undefined);return()=>{live=false;off?.()}},[])
  const [error, setError] = useState<string | null>(null), [busy, setBusy] = useState(false)
  const [selection, setSelection] = useState<string[]>([]), [choose, setChoose] = useState(false), [dismissed, setDismissed] = useState(false)
  const [position,setPosition]=useState<'left'|'right'|'top'|'free'>('right')
  useEffect(()=>{let live=true,off:(()=>void)|undefined;const sync=(p:{dockEdge:'left'|'right'|'top'|null;railPosition:[number,number]|null})=>{if(live)setPosition(p.dockEdge??(p.railPosition?'free':'right'))};panelPlacement().then(sync).catch(()=>undefined);onPlacementChanged(sync).then(fn=>{if(live)off=fn;else fn()}).catch(()=>undefined);return()=>{live=false;off?.()}},[])
  const reload = () => providerSettings().then(setProviders).catch(e => setError(String(e)))
  useEffect(() => {
    let live = true, off: (() => void) | undefined
    void providerSettings().then(p => { if(live) setProviders(p) }).catch(e => { if(live) setError(String(e)) })
    void (async () => {
      try {
        let revision = 0
        off = await onUsageUpdated(s => { revision++; if(live) setSnapshot(s) })
        if(!live) { off(); return }
        const version = revision, s = await getSnapshot()
        if(live && version === revision) setSnapshot(s)
      } catch(e) { if(live) setError(String(e)) }
    })()
    return () => { live = false; off?.() }
  }, [])
  const update = async (patch: Partial<Preferences>) => {
    setBusy(true); setError(null)
    try { await savePreferences({ ...prefs, ...patch }) }
    catch(e) { setError(String(e)); throw e }
    finally { setBusy(false) }
  }
  const change = (patch: Partial<Preferences>) => { void update(patch).catch(() => undefined) }
  useEffect(()=>()=>{releaseSpendSession()},[])
  const shown = useMemo(() => [...providers].sort((a,b) => prefs.providerOrder.indexOf(a.id)-prefs.providerOrder.indexOf(b.id)).filter(p => `${p.name} ${p.id}`.toLowerCase().includes(query.toLowerCase())), [providers,prefs.providerOrder,query])
  const provider = providers.find(p => p.id === pane), enabled = prefs.enabledProviders.includes(pane)
  const accountUsage=snapshot?.providers.find(p=>p.id===pane)
  const appearance = { ...defaultAccountAppearance, ...prefs.accountAppearance[pane] }
  const setAppearance = (patch:Partial<typeof appearance>) => change({accountAppearance:{...prefs.accountAppearance,[pane]:{...appearance,...patch}}})
  const choosing = loaded && (choose || (!dismissed && !prefs.enabledProviders.length))
  const orderedEnabled=prefs.providerOrder.filter(id=>prefs.enabledProviders.includes(id))
  function moveTo(id:string,target:string) {
    const order=[...orderedEnabled],index=order.indexOf(id),next=order.indexOf(target)
    if(index<0||next<0||index===next)return
    order.splice(index,1);order.splice(next,0,id)
    change({providerOrder:[...order,...prefs.providerOrder.filter(item=>!prefs.enabledProviders.includes(item))]})
  }
  function move(id: string, by: number) {
    const next=orderedEnabled[orderedEnabled.indexOf(id)+by]
    if(next)moveTo(id,next)
  }
  return <div className="settings-shell">
    <aside className="settings-sidebar" aria-label="设置导航">
      <div className="settings-brand"><span className="brand-mark">◉</span><strong>Pulse</strong><span>Windows</span></div>
      <input type="search" aria-label="搜索设置与账号" placeholder="搜索设置与账号" value={query} onChange={e => setQuery(e.target.value)} />
      <nav><p className="nav-section">面板</p>
        {[['appearance','外观','◐','大小 间距 圆角 动画'],['rings','圆环与数字','◉','百分比 剩余 预测 时间 第二圆环'],['behavior','位置与行为','↔','收起 刷新 停靠 悬浮'],['accounts','管理账号','⊞','服务商 排序 添加']].filter(x => x.join(' ').toLowerCase().includes(query.toLowerCase())).map(([id,title,icon]) => <button key={id} className={pane === id ? 'selected' : ''} onClick={() => { setPane(id); setChoose(false); setDismissed(true) }}><span aria-hidden="true">{icon}</span>{title}</button>)}
        {[['notifications','通知'],['token-spend','Token 消耗'],['general','通用']].filter(x=>x.join(' ').toLowerCase().includes(query.toLowerCase())).map(([id,title])=><button key={id} className={pane===id?'selected':''} onClick={()=>{setPane(id);setChoose(false);setDismissed(true)}}>{title}</button>)}
        <p className="nav-section">账号</p>
        {shown.map(p => <button key={p.id} className={pane === p.id ? 'selected' : ''} onClick={() => { setPane(p.id); setChoose(false); setDismissed(true) }}><span style={{width:18,height:18,flexShrink:0}}><ProviderIcon provider={p.id}/></span><span className="account-name">{p.name}</span></button>)}
        {!shown.length && query && <p className="muted">没有匹配的账号</p>}
      </nav><div className="sidebar-footer">{prefs.enabledProviders.length} 个账号显示在面板</div>
    </aside>
    <main className="settings-content">
      {(error || loadError) && <div role="alert" className="settings-alert">{error || loadError}<button aria-label="关闭错误提示" onClick={() => setError(null)}>×</button></div>}
      {!loaded ? <p role="status">正在读取设置…</p> : choosing ? <>
        <h1>选择要监控的服务</h1><p className="page-description">只监控你选择的服务。发现本机凭据不会自动启用监控。</p>
        <div className="toolbar"><button disabled={busy} onClick={() => setSelection(providers.filter(p => p.configured).map(p => p.id))}>选择已检测到的服务</button><button disabled={busy} onClick={() => setSelection([])}>清空选择</button></div>
        <section className="settings-group chooser-list">{[...providers].sort((a,b) => Number(b.configured)-Number(a.configured) || a.name.localeCompare(b.name)).map(p => <label key={p.id} className="setting-row"><div><strong>{p.name}</strong><small>{p.configured ? '已发现本机凭据，启用后检查连接' : '启用后可在账号设置中配置连接'}</small></div><input type="checkbox" disabled={busy} checked={selection.includes(p.id)} onChange={e => setSelection(e.target.checked ? [...selection,p.id] : selection.filter(id => id !== p.id))} /></label>)}</section>
        <div className="chooser-actions"><button disabled={busy} onClick={() => { setDismissed(true); setChoose(false) }}>暂不设置</button><button className="primary" disabled={busy || !selection.length} onClick={() => { void update({ enabledProviders: selection }).then(() => { setChoose(false); setDismissed(true) }).catch(() => undefined) }}>{busy ? '正在启动…' : '完成'}</button></div>
      </> : pane === 'appearance' ? <>
        <h1>外观</h1><p className="page-description">调整屏幕边缘的用量面板。</p>
        {!prefs.enabledProviders.length && <div className="empty-banner">尚未选择监控服务。<button onClick={() => { setSelection([]); setChoose(true) }}>选择服务</button></div>}
        <Group title="尺寸与形状"><Row title="大小"><select disabled={busy} value={prefs.panelSize} onChange={e => change({ panelSize: Number(e.target.value) })}><option value="0.82">较小</option><option value="1">标准</option><option value="1.22">较大</option></select></Row>
        <Row title="间距"><select disabled={busy} value={prefs.railSpacing} onChange={e => change({ railSpacing: Number(e.target.value) })}><option value="0.6">紧凑</option><option value="1">标准</option><option value="1.4">宽松</option></select></Row>
        <Toggle title="圆润端点" value={prefs.roundEnds} disabled={busy} onChange={v => change({ roundEnds: v })} /><Toggle title="显示工作与刷新动画" value={prefs.animateActivity} disabled={busy} onChange={v=>change({animateActivity:v})}/><Toggle title="额度重置庆祝" subtitle="机器人在见证额度窗口重置时播放庆祝动作。" value={prefs.resetCelebration} disabled={busy} onChange={v=>change({resetCelebration:v})}/></Group>
        </> : pane === 'rings' ? <>
        <h1>圆环与数字</h1><p className="page-description">圆环、数字与额度窗口的显示方式。</p><Group title="用量显示"><Toggle title="显示剩余额度" subtitle="圆环和数字显示剩余量，颜色仍表示使用程度。" value={prefs.showsRemaining} disabled={busy} onChange={v => change({ showsRemaining: v })} />
        <Toggle title="显示百分比" subtitle="侧边显示数字，顶部面板保持紧凑。" value={prefs.showPercentages} disabled={busy} onChange={v => change({ showPercentages: v })} />
        <Toggle title="顶部显示百分比" value={prefs.topShowPercentages} disabled={busy} onChange={v=>change({topShowPercentages:v})}/><Toggle title="显示第二个用量圆环" subtitle="在主环内显示第二个额度窗口，单一窗口保持单环。" value={prefs.showSecondRing} disabled={busy} onChange={v=>change({showSecondRing:v})}/><Toggle title="收起时显示预警颜色" value={prefs.dockAlertColour} disabled={busy} onChange={v=>change({dockAlertColour:v})}/><Toggle title="数字显示在圆环上方" value={prefs.labelAbove} disabled={busy} onChange={v => change({ labelAbove: v })} />
        <Toggle title="显示用量预测" subtitle="按窗口内的平均使用速度估算，只在有足够信息时显示。" value={prefs.showForecast} disabled={busy} onChange={v=>change({showForecast:v})}/><Toggle title="重置时间圆环" subtitle="仅在服务明确报告窗口时长时显示。" value={prefs.showResetClock} disabled={busy} onChange={v => change({ showResetClock: v })} /><Toggle title="时间圆环显示剩余时间" value={prefs.clockRemaining} disabled={busy || !prefs.showResetClock} onChange={v => change({ clockRemaining: v })} /><Row title="变红阈值"><select disabled={busy} value={prefs.warningThreshold} onChange={e => change({ warningThreshold: Number(e.target.value) })}>{[.6,.7,.75,.8,.85,.9].map(v => <option key={v} value={v}>{Math.round(v*100)}%</option>)}</select></Row></Group>
      </> : pane === 'behavior' ? <>
        <h1>位置与行为</h1><p className="page-description">面板交互与用量检查。</p>
        <Group title="面板"><Row title="位置" subtitle="靠近边缘停靠，也可以自由悬浮。"><select aria-label="面板位置" value={position} onChange={e=>{void setPanelPosition(e.target.value as typeof position).catch(err=>setError(String(err)))}}><option value="left">左侧</option><option value="top">顶部</option><option value="free">自由悬浮</option><option value="right">右侧</option></select></Row><Toggle title="全屏时隐藏" subtitle="在面板所在屏幕播放全屏内容时暂时隐藏，退出后恢复。" value={prefs.hideInFullscreen} disabled={busy} onChange={v=>change({hideInFullscreen:v})}/><Toggle title="跟随鼠标所在屏幕" subtitle="移到另一块屏幕时，保留面板在屏幕上的相对位置。" value={prefs.followActiveDisplay} disabled={busy} onChange={v=>change({followActiveDisplay:v})}/><Toggle title="自动收起" subtitle="鼠标离开后收起至屏幕边缘。" value={prefs.autoCollapse} disabled={busy} onChange={v => change({ autoCollapse: v })} /></Group>
        <Group title="刷新"><Row title="检查间隔" subtitle="自动按工作活动和读数变化调整，仅检查已启用的账号。"><select disabled={busy} value={prefs.refreshAutomatic ? 'automatic' : String(prefs.refreshSeconds)} onChange={e => change(e.target.value === 'automatic' ? { refreshAutomatic: true } : { refreshAutomatic: false, refreshSeconds: Number(e.target.value) })}><option value="automatic">自动（2–30 分钟）</option>{[120,300,600,1800].map(v => <option key={v} value={v}>{v/60} 分钟</option>)}</select></Row></Group>
      </> : pane === 'notifications' ? <NotificationsPane alerts={prefs.alerts} disabled={busy} onChange={patch=>change({alerts:{...prefs.alerts,...patch}})}/>
        : pane === 'general' ? <GeneralPane/>
        : pane === 'token-spend' ? <TokenSpendPane enabled={prefs.tokenSpendEnabled} onEnabledChange={v=>change({tokenSpendEnabled:v})} span={prefs.tokenSpendSpan} onSpanChange={v=>change({tokenSpendSpan:v})}/>
        : pane === 'accounts' ? <>
        <h1>管理账号</h1><p className="page-description">控制面板显示的服务和排列顺序。</p><button onClick={() => { setSelection(prefs.enabledProviders); setChoose(true) }}>选择服务</button>
        <Group title="排列顺序">{shown.filter(p=>prefs.enabledProviders.includes(p.id)).map(p => <div key={p.id} draggable onDragStart={e=>e.dataTransfer.setData('application/x-pulse-account',p.id)} onDragOver={e=>{if(e.dataTransfer.types.includes('application/x-pulse-account'))e.preventDefault()}} onDrop={e=>{e.preventDefault();moveTo(e.dataTransfer.getData('application/x-pulse-account'),p.id)}}><Row title={p.name} subtitle={prefs.enabledProviders.includes(p.id) ? '显示在面板' : '未显示'}><div className="toolbar"><button disabled={busy || orderedEnabled.indexOf(p.id) === 0} aria-label={`上移 ${p.name}`} onClick={() => move(p.id,-1)}>↑</button><button disabled={busy || orderedEnabled.indexOf(p.id) === orderedEnabled.length-1} aria-label={`下移 ${p.name}`} onClick={() => move(p.id,1)}>↓</button></div></Row></div>)}</Group>
      </> : provider ? <>
        <h1>{provider.name}</h1><p className="page-description">{provider.configured ? '已发现凭据；连接结果以下方实际检查为准。' : '尚未发现可用凭据。'}</p>
        {provider.credentialError&&<div role="alert" className="settings-alert">{provider.credentialError}</div>}
        {enabled&&accountUsage?.creditRemaining&&<LowBalanceAlert value={prefs.alerts.lowBalance[pane]} currency={accountUsage.creditRemaining.currency} disabled={busy} onChange={v=>{const lowBalance={...prefs.alerts.lowBalance};if(v===undefined)delete lowBalance[pane];else lowBalance[pane]=v;change({alerts:{...prefs.alerts,lowBalance}})}}/>}
        <Group title="面板"><Toggle title="显示在面板" subtitle="关闭后停止此账号的用量检查。" disabled={busy} value={enabled} onChange={v => change({ enabledProviders: v ? [...prefs.enabledProviders,pane] : prefs.enabledProviders.filter(id => id !== pane) })} /></Group>
        {enabled ? <><Group title="圆环显示"><Row title="显示的额度窗口"><select aria-label="显示的额度窗口" disabled={busy} value={accountUsage?.windows.find(w=>w.label===prefs.pinnedWindows[pane])?.id ?? prefs.pinnedWindows[pane] ?? ''} onChange={e => { const pins = { ...prefs.pinnedWindows }; if(e.target.value) pins[pane] = e.target.value; else delete pins[pane]; change({ pinnedWindows: pins }) }}><option value="">自动选择最接近上限的窗口</option>{accountUsage?.windows.map((w,i) => <option key={w.id ?? `${w.label}:${i}`} value={w.id ?? w.label}>{w.label}{w.scope ? ` · ${w.scope}` : ''}</option>)}</select></Row></Group><Group title="详情卡"><Toggle title="显示详细用量卡" subtitle="显示更新时刻、用量历史与金额估算。读取本机历史需开启 Token 消耗。" value={appearance.detailedCard} disabled={busy} onChange={v=>setAppearance({detailedCard:v})}/></Group><Group title="动画标记"><Toggle title="使用动画机器人" subtitle="替代此账号的供应商图标，随用量状态变化。" value={appearance.animatedMark} disabled={busy} onChange={v=>setAppearance({animatedMark:v})}/>{appearance.animatedMark && <>
        <div className="bot-preview"><BotMark mood="idle" colour={appearance.markColour??brandColour(pane)??'#8e9dff'} body={appearance.body} persona={personaAt(appearance.persona,Math.max(0,prefs.providerOrder.indexOf(pane)))}/><span>闲置时的动作预览</span></div>
        <Row title="人格"><select aria-label="机器人人格" disabled={busy} value={appearance.persona} onChange={e=>setAppearance({persona:e.target.value})}><option value="automatic">自动分配</option>{PERSONAS.map((id,i)=><option key={id} value={id}>{PERSONA_LABELS[i]}</option>)}</select></Row>
        <Row title="形状"><select aria-label="机器人形状" disabled={busy} value={appearance.body} onChange={e=>setAppearance({body:e.target.value})}>{SHAPE_ORDER.map(id=><option key={id} value={id}>{SHAPE_LABELS[id]?.zh??id}</option>)}</select></Row>
        <Row title="机器人颜色"><div className="toolbar"><input type="color" aria-label="机器人颜色" value={appearance.markColour??'#8e9dff'} disabled={busy} onChange={e=>setAppearance({markColour:e.target.value})}/><button disabled={busy||!appearance.markColour} onClick={()=>setAppearance({markColour:null})}>自动</button></div></Row></>}</Group>
        <Group title="圆环颜色"><Row title="高亮色" subtitle="自动随用量变化；用尽时仍显示深红。"><div className="toolbar"><input type="color" aria-label="圆环高亮色" value={appearance.ringColour??'#00e68c'} disabled={busy} onChange={e=>setAppearance({ringColour:e.target.value})}/><button disabled={busy||!appearance.ringColour} onClick={()=>setAppearance({ringColour:null})}>自动</button></div></Row></Group><AccountDetails key={provider.id} provider={provider} usage={snapshot?.providers.find(p => p.id === pane)} onError={setError} onChanged={reload} /></> : <Group title="当前用量"><p className="group-copy">未显示。启用后配置连接并检查用量。</p></Group>}
      </> : null}
    </main>
  </div>
}
function Group({ title, children }: { title: string; children: ReactNode }) { return <section className="group-wrap"><h2>{title}</h2><div className="settings-group">{children}</div></section> }
function Row({ title, subtitle, children }: { title: string; subtitle?: string; children: ReactNode }) { return <div className="setting-row"><div><strong>{title}</strong>{subtitle && <small>{subtitle}</small>}</div><div className="setting-control">{children}</div></div> }
function Toggle({ title, subtitle, value, disabled, onChange }: { title: string; subtitle?: string; value: boolean; disabled: boolean; onChange: (v:boolean) => void }) { return <Row title={title} subtitle={subtitle}><input type="checkbox" role="switch" aria-label={title} checked={value} disabled={disabled} onChange={e => onChange(e.target.checked)} /></Row> }
function AccountDetails({ provider, usage, onError, onChanged }: { provider: ProviderSetting; usage?: ProviderUsage; onError:(e:string|null)=>void; onChanged:()=>void }) {
  const [key,setKey] = useState(''), [address,setAddress] = useState(''), [busy,setBusy] = useState(false), [saved,setSaved] = useState(false)
  const action = async (save:boolean) => {
    setBusy(true); setSaved(false); onError(null)
    try { if(save) { await saveProviderCredential(provider.id,key||null,address||null); setKey(''); setAddress(''); setSaved(true); onChanged() } else await refreshProvider(provider.id) }
    catch(e) { onError(String(e)) } finally { setBusy(false) }
  }
  return <><Group title="连接"><Row title="访问凭据" subtitle={provider.stored ? '已保存凭据。留空保留现有值。' : '填写 API Key、Token 或 Cookie。'}><input type="password" aria-label="访问凭据" autoComplete="off" placeholder="粘贴凭据" value={key} onChange={e => { setKey(e.target.value); setSaved(false) }} /></Row>
    <Row title="服务器地址" subtitle="仅自建网关需要；留空保留现有地址。"><input type="url" aria-label="服务器地址" placeholder="https://…" value={address} onChange={e => { setAddress(e.target.value); setSaved(false) }} /></Row>
    <div className="group-actions"><span role="status">{saved ? '已保存，连接结果见下方' : ''}</span><button disabled={busy || (!key.trim() && !address.trim())} onClick={() => { void action(true) }}>{busy ? '正在检查…' : '保存并检查'}</button></div>
    <details><summary>凭据读取位置</summary>{provider.hints.map(p => <code key={p}>{p}</code>)}</details></Group>
    {provider.id === 'opencode-go' && <p className="group-copy">需要当前密钥所属账号和工作区的 Go 订阅。默认读取 OpenCode CLI 登录；在这里保存的 API key 优先于 CLI。若已订阅却提示未检测到订阅，请核对 OpenCode 控制台中的账号、工作区与密钥。</p>}
    {usage?.source && <p className="group-copy" data-testid="credential-source">凭据来源：{usage.source}</p>}
    <Group title="当前用量">{usage?.error&&<div className="connection-error"><strong>{usage.windows.length||creditText(usage.creditRemaining)?'检查失败，显示上次有效读数':'无法获取用量'}</strong><p>{usage.error}</p></div>}{usage?.stale&&<p className="group-copy">旧读数 · {new Date(usage.fetchedAt).toLocaleString()}</p>}{usage?.windows.length ? usage.windows.map((w,i) => <Row key={`${w.label}-${i}`} title={w.label} subtitle={[w.detail,formatReset(w.resetsAt)].filter(Boolean).join(' · ')}><span className="usage-value">{w.percentUsed === null ? '—' : `${percentFigure(w.percentUsed/100)}%`}</span></Row>) : !usage?.error&&!creditText(usage?.creditRemaining)&&<p className="group-copy">尚未完成检查。</p>}
    {creditText(usage?.creditRemaining)&&<Row title="余额"><span className="usage-value">{creditText(usage?.creditRemaining)}</span></Row>}
    <div className="group-actions"><small>{usage ? `最近检查：${new Date(usage.fetchedAt).toLocaleString()}` : ''}</small><button disabled={busy} onClick={() => { void action(false) }}>{busy ? '正在检查…' : '刷新此账号'}</button></div></Group></>
}


