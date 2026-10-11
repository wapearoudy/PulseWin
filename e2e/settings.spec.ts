import { test, expect } from '@playwright/test'

// IPC is deliberately isolated from real credentials and the native application.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any
    let seq = 0
    const callbacks: Record<number, Function> = {}, listeners: Record<number, any> = {}
    const prefs: any = { accounts: [], accountLabels: {}, enabledProviders: [], providerOrder: ['codex','claude-code','deepseek'], panelSize: 1, railSpacing: 1, roundEnds: false, showsRemaining: false, showPercentages: true, labelAbove: false, autoCollapse: true, warningThreshold: .75, refreshSeconds: 120, showResetClock: false, clockRemaining: false, pinnedWindows: {} }
    let providers: any[] = [{id:'codex',name:'Codex',configured:true},{id:'claude-code',name:'Claude Code',configured:true},{id:'deepseek',name:'DeepSeek',configured:false}].map(x=>({...x,providerId:x.id,stored:false,hints:['Local tool sign-in'],credentialPath:null}))
    w.testCommands = []; w.testPrefs = prefs
    const emit = (event: string, payload: unknown) => Object.values(listeners).filter(x => x.event === event).forEach(x => callbacks[x.handler]?.({ event, payload }))
    w.testEmit=emit
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_: string, id: number) => delete listeners[id] }
    w.__TAURI_INTERNALS__ = {
      transformCallback: (fn: Function) => { callbacks[++seq] = fn; return seq },
      invoke: async (cmd: string, args: any = {}) => {
        w.testCommands.push({ cmd, args })
        if(cmd === 'plugin:event|listen') { listeners[++seq] = args; return seq }
        if(cmd === 'plugin:event|unlisten') return
        if(cmd === 'get_deepseek_console_connection') return {connected:!!w.testConsoleConnected}
        if(cmd === 'begin_deepseek_console_login') return
        if(cmd === 'finish_deepseek_console_login') {w.testConsoleConnected=true;return {connected:true}}
        if(cmd === 'remove_deepseek_console_login') {w.testConsoleConnected=false;return {connected:false}}
        if(cmd === 'get_deepseek_console_history') {if(w.testConsoleError)throw new Error(w.testConsoleError);return structuredClone(w.testConsoleHistory)}
        if(cmd === 'get_opencode_console_connection') return {connected:!!w.testOpenCodeConnected}
        if(cmd === 'begin_opencode_console_login') return
        if(cmd === 'finish_opencode_console_login') {w.testOpenCodeConnected=true;return {connected:true}}
        if(cmd === 'remove_opencode_console_login') {w.testOpenCodeConnected=false;return {connected:false}}
        if(cmd === 'get_opencode_console_history') {if(w.testOpenCodeError)throw new Error(w.testOpenCodeError);if(w.testOpenCodeHold)return await new Promise(resolve=>w.resolveOpenCode=resolve);return structuredClone(w.testOpenCodeHistory)}
        if(cmd === 'get_service_status') { if(w.testStatusError) throw new Error(w.testStatusError); return structuredClone(w.testStatus ?? null) }
        if(cmd === 'get_codex_signals') { if(w.testSignalsError) throw new Error(w.testSignalsError); return structuredClone(w.testSignals ?? {models:[],changes:[],sessions:0,judgedSessions:0,notes:[]}) }
        if(cmd === 'get_prompt_cache') { if(w.testCacheError) throw new Error(w.testCacheError); return structuredClone(w.testCache ?? {live:[],lastLapsed:null,notes:[]}) }
        if(cmd === 'get_preferences') return structuredClone(prefs)
        if(cmd === 'save_preferences') { Object.assign(prefs,args.value); emit('preferences-changed',structuredClone(prefs)); return }
        if(cmd === 'provider_settings') return structuredClone([...providers,...(w.testOpenCodeProvider?[{id:'opencode-go',providerId:'opencode-go',name:'OpenCode Go',configured:true,stored:false,hints:[],credentialPath:null}]:[])])
        if(cmd === 'add_account') {
          const id=args.provider+'--account-ab';prefs.accounts.push({id,provider:args.provider});prefs.accountLabels[id]=args.label;
          if(args.enabled!==false)prefs.enabledProviders.push(id);prefs.providerOrder.push(id);
          providers.push({id,providerId:args.provider,additional:true,name:args.label,configured:true,stored:true,hints:['Independent encrypted login'],credentialPath:null});
          emit('preferences-changed',structuredClone(prefs));return id
        }
        if(cmd === 'remove_account') {
          providers=providers.filter(p=>p.id!==args.account);prefs.accounts=prefs.accounts.filter((a:any)=>a.id!==args.account);
          prefs.enabledProviders=prefs.enabledProviders.filter((id:string)=>id!==args.account);prefs.providerOrder=prefs.providerOrder.filter((id:string)=>id!==args.account);
          delete prefs.accountLabels[args.account];emit('preferences-changed',structuredClone(prefs));return
        }
        if(cmd === 'get_snapshot') return {providers:[],fetchedAt:new Date().toISOString()}
        if(cmd === 'save_provider_credential') throw new Error('模拟保存失败：磁盘不可写')
        if(cmd === 'refresh_provider') {
          const snapshot = {providers:[{id:args.provider,name:'Codex',windows:[{label:'5h',percentUsed:37,resetsAt:null,detail:'Test fixture'}],configured:true,fetchedAt:new Date().toISOString(),error:null}],fetchedAt:new Date().toISOString()}
          emit('usage-updated',snapshot); return snapshot
        }
      },
    }
  })
  await page.goto('/?view=settings')
})

test('official status shows source colours, missing history and publisher uptime',async({page})=>{
  await page.evaluate(()=>{(window as any).testStatus={page:'claude',checkedAt:'2026-10-11T01:00:00Z',historyError:null,components:[{id:'code',name:'Claude Code API',state:'partialOutage',uptime:99.95,days:[{date:'2026-10-09',state:null,colour:null},{date:'2026-10-10',state:'partialOutage',colour:'#e75f36'},{date:'2026-10-11',state:'operational',colour:'#76ad2a'}]}]}})
  await page.getByRole('button',{name:'暂不设置'}).click();await page.getByRole('button',{name:'Claude Code',exact:true}).click()
  const group=page.getByRole('region',{name:'官方服务状态'});await expect(group.getByText('部分故障',{exact:true})).toBeVisible();await expect(group.getByText('99.95% 正常运行 · 官方报告')).toBeVisible()
  await expect(group.getByRole('listitem')).toHaveCount(3);await expect(group.getByRole('listitem').first()).toHaveAttribute('title','2026-10-09 · 无数据');await expect(group.getByRole('listitem').nth(1)).toHaveCSS('background-color','rgb(231, 95, 54)')
  await expect(group.getByRole('link',{name:'打开'})).toHaveAttribute('href','https://status.claude.com/')
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='refresh_provider'))).toHaveLength(0)
  await page.screenshot({path:'test-results/service-status.png',fullPage:true})
})

test('status rereads at five minutes and failed reads remove healthy rows; leaving stops polling',async({page})=>{
  await page.clock.install();await page.evaluate(()=>{(window as any).testStatus={page:'openAI',checkedAt:'2026-10-11T01:00:00Z',historyError:null,components:[{id:'cli',name:'CLI',state:'operational',uptime:null,days:[]}]}})
  await page.getByRole('button',{name:'暂不设置'}).click();await page.getByRole('button',{name:'Codex',exact:true}).click()
  const group=page.getByRole('region',{name:'官方服务状态'});await expect(group.getByText('运行正常',{exact:true})).toBeVisible()
  await page.evaluate(()=>{(window as any).testStatusError='官方状态页请求失败'});await page.clock.fastForward(300_000)
  await expect(group.getByText(/官方状态页请求失败/)).toBeVisible();await expect(group.getByText('运行正常',{exact:true})).toHaveCount(0)
  const count=await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='get_service_status').length)
  await page.getByRole('button',{name:'外观',exact:true}).click();await page.clock.fastForward(600_000)
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='get_service_status').length)).toBe(count)
})

test('outage notifications default off and persist independently of quota alerts',async({page})=>{
  await page.getByRole('button',{name:'暂不设置'}).click();await page.getByRole('button',{name:'通知',exact:true}).click()
  const toggle=page.getByRole('switch',{name:'服务故障与恢复通知'});await expect(toggle).not.toBeChecked();await toggle.check()
  expect(await page.evaluate(()=>(window as any).testPrefs.alerts.onServiceOutage)).toBe(true)
  await expect(page.getByLabel('额度提醒阈值')).toHaveValue('');await toggle.uncheck()
})

test('first-run selection is explicit and disabled accounts cannot refresh', async ({page}) => {
  await expect(page.getByRole('heading',{name:'选择要监控的服务'})).toBeVisible()
  await expect(page.getByRole('button',{name:'完成',exact:true})).toBeDisabled()
  expect(await page.evaluate(() => (window as any).testCommands.filter((x:any) => x.cmd === 'refresh_provider'))).toHaveLength(0)
  await page.getByRole('button',{name:'暂不设置'}).click()
  await page.getByRole('button',{name:'DeepSeek',exact:true}).click()
  await expect(page.getByRole('button',{name:'刷新此账号'})).toHaveCount(0)
  await expect(page.getByText('未显示。启用后配置连接并检查用量。')).toBeVisible()
})

test('appearance persists, account refresh is scoped, and save errors are visible', async ({page}) => {
  await page.getByRole('button',{name:'选择已检测到的服务'}).click()
  await page.getByRole('button',{name:'完成',exact:true}).click()
  await expect(page.getByRole('heading',{name:'外观',exact:true})).toBeVisible()
  await page.getByRole('button',{name:'圆环与数字',exact:true}).click()
  await page.getByRole('switch',{name:'显示剩余额度'}).check()
  await expect(page.getByRole('switch',{name:'显示剩余额度'})).toBeChecked()
  expect(await page.evaluate(() => (window as any).testPrefs.showsRemaining)).toBe(true)
  await page.screenshot({path:'test-results/settings-appearance.png'})
  await page.getByRole('button',{name:'Codex',exact:true}).click()
  await page.getByRole('button',{name:'刷新此账号'}).click()
  await expect(page.getByText('37%',{exact:true})).toBeVisible()
  expect(await page.evaluate(() => (window as any).testCommands.filter((x:any) => x.cmd === 'refresh_provider').map((x:any) => x.args.provider))).toEqual(['codex'])
  await page.getByLabel('访问凭据',{exact:true}).fill('test-only-not-a-real-key')
  await page.getByRole('button',{name:'保存并检查'}).click()
  await expect(page.getByRole('alert')).toContainText('磁盘不可写')
  await page.screenshot({path:'test-results/settings-account.png'})
  await page.setViewportSize({width:720,height:480})
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.screenshot({path:'test-results/settings-small.png'})
})


test('account robot preferences preserve other accounts and all eighteen bodies are selectable', async ({page})=>{
  await page.getByRole('button',{name:'选择已检测到的服务'}).click()
  await page.getByRole('button',{name:'完成',exact:true}).click()
  await page.getByRole('button',{name:'Codex',exact:true}).click()
  await page.getByRole('switch',{name:'使用动画机器人'}).check()
  await page.getByLabel('机器人人格',{exact:true}).selectOption('curious')
  await page.getByLabel('机器人形状',{exact:true}).selectOption('gem')
  await expect(page.locator('.bot-preview .botmark')).toHaveAttribute('data-persona','curious')
  await expect(page.locator('.bot-preview .botmark')).toHaveAttribute('data-body','gem')
  await expect(page.getByLabel('机器人形状',{exact:true}).locator('option')).toHaveCount(18)
  const saved=await page.evaluate(()=>(window as any).testPrefs.accountAppearance)
  expect(saved.codex).toMatchObject({animatedMark:true,persona:'curious',body:'gem'})
  expect(saved['claude-code']).toBeUndefined()
  await page.waitForTimeout(450)
  await page.screenshot({path:'test-results/account-bot-settings.png',fullPage:true})
})

test('panel connection action selects the matching account in an existing settings window',async({page})=>{
  await page.getByRole('button',{name:'选择已检测到的服务'}).click();await page.getByRole('button',{name:'完成',exact:true}).click()
  await page.evaluate(()=>(window as any).testEmit('settings-account','codex'))
  await expect(page.getByRole('heading',{name:'Codex',exact:true})).toBeVisible()
  await page.getByRole('switch',{name:'显示详细用量卡'}).check()
  expect(await page.evaluate(()=>(window as any).testPrefs.accountAppearance.codex.detailedCard)).toBe(true)
  await page.evaluate(()=>(window as any).testEmit('usage-updated',{providers:[{id:'codex',name:'Codex',windows:[],configured:true,error:'当前密钥所属账号或工作区未检测到订阅。',source:'OpenCode CLI：synthetic/auth.json',fetchedAt:new Date().toISOString()}]}))
  await expect(page.getByTestId('credential-source')).toContainText('synthetic/auth.json')
})

test('reordering moves only the enabled accounts with arrows and drag',async({page})=>{
  await page.getByRole('button',{name:'选择已检测到的服务'}).click()
  await page.getByRole('button',{name:'完成',exact:true}).click()
  await page.evaluate(()=>{const w=window as any;return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,providerOrder:['codex','deepseek','claude-code']}})})
  await page.getByRole('button',{name:'管理账号',exact:true}).click()
  await expect(page.locator('[draggable=true]')).toHaveCount(2)
  await page.getByRole('button',{name:'上移 Claude Code',exact:true}).click()
  expect(await page.evaluate(()=>(window as any).testPrefs.providerOrder)).toEqual(['claude-code','codex','deepseek'])
  await page.locator('[draggable=true]').filter({hasText:'Claude Code'}).dragTo(page.locator('[draggable=true]').filter({hasText:'Codex'}))
  expect(await page.evaluate(()=>(window as any).testPrefs.providerOrder)).toEqual(['codex','claude-code','deepseek'])
})

test('notifications are opt-in and reset warnings depend on the threshold',async({page})=>{
  await page.getByRole('button',{name:'暂不设置'}).click()
  await page.getByRole('button',{name:'通知',exact:true}).click()
  await expect(page.getByText('达到门槛且用量进度高于时间进度时提醒。')).toBeVisible()
  await expect(page.getByText('例如门槛为 75%',{exact:false})).toBeVisible()
  await expect(page.getByLabel('额度提醒阈值')).toHaveValue('')
  await expect(page.getByRole('switch',{name:'额度重置时通知'})).toBeDisabled()
  await expect(page.getByRole('switch',{name:'连续检查失败时通知'})).not.toBeChecked()
  await page.getByLabel('额度提醒阈值').selectOption('90')
  await page.getByRole('switch',{name:'额度重置时通知'}).check()
  await page.getByRole('switch',{name:'连续检查失败时通知'}).check()
  expect(await page.evaluate(()=>(window as any).testPrefs.alerts)).toEqual({threshold:90,onReset:true,onFailure:true,lowBalance:{},onServiceOutage:false,onRecap:false})
  await page.getByLabel('额度提醒阈值').selectOption('')
  await expect(page.getByRole('switch',{name:'额度重置时通知'})).toBeDisabled()
})

test('a failed check retains dated quota and money alerts stay scoped to its account',async({page})=>{
  await page.getByRole('button',{name:'选择已检测到的服务'}).click()
  await page.getByRole('button',{name:'完成',exact:true}).click()
  await page.getByRole('button',{name:'Codex',exact:true}).click()
  await page.evaluate(()=>{(window as any).testEmit('usage-updated',{providers:[{id:'codex',name:'Codex',windows:[{label:'5h',percentUsed:99.6,resetsAt:null,detail:null}],creditRemaining:{amount:3,currency:'USD'},error:'HTTP 503',stale:true,configured:true,fetchedAt:new Date(Date.now()-60000).toISOString()}],fetchedAt:new Date().toISOString()})})
  await expect(page.getByText('检查失败，显示上次有效读数')).toBeVisible()
  await expect(page.getByText('99%',{exact:true})).toBeVisible()
  await expect(page.getByText('旧读数 ·',{exact:false})).toBeVisible()
  const threshold=page.getByLabel('余额提醒金额')
  await threshold.fill('5');await threshold.press('Enter')
  expect(await page.evaluate(()=>(window as any).testPrefs.alerts.lowBalance)).toEqual({codex:5})
  await threshold.fill('');await threshold.press('Enter')
  expect(await page.evaluate(()=>(window as any).testPrefs.alerts.lowBalance)).toEqual({})
  await page.getByRole('button',{name:'Claude Code',exact:true}).click()
  await expect(page.getByLabel('余额提醒金额')).toHaveCount(0)
})

test('same-service accounts add, rename, refresh and remove independently',async({page})=>{
  await page.getByRole('button',{name:'选择已检测到的服务'}).click();await page.getByRole('button',{name:'完成',exact:true}).click()
  await page.getByRole('button',{name:'管理账号',exact:true}).click();await page.getByRole('button',{name:'添加账号',exact:true}).click()
  await page.getByLabel('新账号名称',{exact:true}).fill('工作 Codex')
  await page.getByLabel('新账号登录来源',{exact:true}).selectOption('paste')
  await page.getByLabel('新账号登录信息',{exact:true}).fill('synthetic-token-only')
  await page.getByRole('button',{name:'添加并检查',exact:true}).click()
  await expect(page.getByRole('heading',{name:'工作 Codex',exact:true})).toBeVisible()
  await expect(page.locator('nav').getByRole('button',{name:'Codex',exact:true})).toBeVisible()
  const icons=await page.locator('nav [data-provider-icon="codex--account-ab"]').evaluate(e=>getComputedStyle(e).maskImage)
  const baseIcon=await page.locator('nav [data-provider-icon="codex"]').evaluate(e=>getComputedStyle(e).maskImage)
  expect(icons).toBe(baseIcon)
  await page.getByLabel('账号显示名称',{exact:true}).fill('公司账号');await page.getByRole('button',{name:'保存名称',exact:true}).click()
  await expect(page.getByRole('heading',{name:'公司账号',exact:true})).toBeVisible()
  await page.getByRole('switch',{name:'使用动画机器人'}).check()
  await page.getByRole('button',{name:'刷新此账号',exact:true}).click()
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='refresh_provider').map((x:any)=>x.args.provider))).toEqual(['codex--account-ab'])
  const prefs=await page.evaluate(()=>(window as any).testPrefs)
  expect(prefs.accountAppearance['codex--account-ab'].animatedMark).toBe(true);expect(prefs.accountAppearance.codex).toBeUndefined()
  await page.screenshot({path:'test-results/settings-multi-account.png',fullPage:true})
  await page.getByRole('button',{name:'移除账号',exact:true}).click();await page.getByRole('button',{name:'确认移除',exact:true}).click()
  await expect(page.getByRole('heading',{name:'管理账号',exact:true})).toBeVisible()
  await expect(page.locator('nav').getByRole('button',{name:'公司账号',exact:true})).toHaveCount(0)
  expect(await page.evaluate(()=>(window as any).testPrefs.enabledProviders)).toEqual(['codex','claude-code'])
})


test('local Codex analysis requires consent, uses requested range and cancels on exit',async({page})=>{
  await page.getByRole('button',{name:'暂不设置'}).click();await page.getByRole('button',{name:'Codex',exact:true}).click()
  const group=page.getByRole('region',{name:'Codex 本地分析'});await expect(group.getByLabel('Codex 分析范围')).toBeDisabled()
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='get_codex_signals'))).toHaveLength(0)
  await page.evaluate(()=>{const w=window as any;w.testSignals={models:[{model:'gpt-test',responses:10,reached:10,hits:5,share:.5,measurable:false,suspicious:false}],changes:[],sessions:3,judgedSessions:1,notes:[]};return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,tokenSpendEnabled:true}})})
  await expect(group.getByText('样本不足',{exact:true})).toBeVisible();await expect(group.getByText(/1 \/ 3 个会话可判断/)).toBeVisible()
  await page.evaluate(()=>{(window as any).testSignalsError='本地文件读取失败'});await group.getByLabel('Codex 分析范围').selectOption('quarter')
  await expect(group.getByRole('alert')).toContainText('本地文件读取失败');await expect(group.getByText('gpt-test',{exact:true})).toHaveCount(0)
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='get_codex_signals').map((x:any)=>x.args.span))).toEqual(['month','quarter'])
  const before=await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='cancel_codex_signal_read').length)
  await page.getByRole('button',{name:'外观',exact:true}).click()
  await expect.poll(()=>page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='cancel_codex_signal_read').length)).toBeGreaterThan(before)
})

test('prompt cache countdown refreshes, errors discard readings and navigation stops polling',async({page})=>{
  await page.clock.install({time:new Date('2026-10-11T00:00:00Z')});await page.clock.pauseAt(new Date('2026-10-11T00:00:00Z'));await page.getByRole('button',{name:'暂不设置'}).click()
  await page.evaluate(()=>{const w=window as any;w.testCache={live:[{id:'cache',title:'可计时会话',project:'工程',lastRequest:new Date().toISOString(),expiresAt:new Date(Date.now()+31*60_000).toISOString(),lifetime:1800,isMinimum:true}],lastLapsed:null,notes:[]};return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,tokenSpendEnabled:true}})})
  await page.getByRole('button',{name:'Codex',exact:true}).click();const group=page.getByRole('region',{name:'提示词缓存'})
  await expect(group.getByText('可计时会话',{exact:true})).toBeVisible();await expect(group.getByText('至少 31分 0秒',{exact:true})).toBeVisible()
  await page.clock.fastForward(1_000);await expect(group.getByText('至少 30分 59秒',{exact:true})).toBeVisible()
  await page.evaluate(()=>{(window as any).testCacheError='缓存记录不可读'});await page.clock.fastForward(30_000)
  await expect(group.getByRole('alert')).toContainText('缓存记录不可读');await expect(group.getByText('可计时会话',{exact:true})).toHaveCount(0)
  const count=await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='get_prompt_cache').length)
  await page.getByRole('button',{name:'外观',exact:true}).click();await page.clock.fastForward(60_000)
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='get_prompt_cache').length)).toBe(count)
})


test('DeepSeek console login is independent of API keys and never reads disabled billing',async({page})=>{
 await page.getByRole('button',{name:'暂不设置'}).click();await page.getByRole('button',{name:'DeepSeek',exact:true}).click()
 const group=page.getByRole('region',{name:'DeepSeek 控制台账单'});await group.getByRole('button',{name:'登录控制台',exact:true}).click();await group.getByRole('button',{name:'已登录，连接控制台'}).click()
 await expect(group.getByText('已连接控制台',{exact:true})).toBeVisible();await expect(group.getByRole('button',{name:'刷新账单'})).toBeDisabled()
 expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='get_deepseek_console_history'))).toHaveLength(0)
 await page.evaluate(()=>{const w=window as any;w.testConsoleHistory={days:[{date:'2026-10-11',input:100,output:20,cacheRead:30,tokens:'150',cost:null,models:{'deepseek-chat':150}}],currency:null,checkedAt:new Date().toISOString(),notes:[]};return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,enabledProviders:['deepseek']}})})
 await expect(group.getByRole('cell',{name:'150',exact:true})).toBeVisible();await expect(group.getByRole('cell',{name:'—',exact:true})).toBeVisible()
 await page.evaluate(()=>{(window as any).testConsoleError='控制台登录已失效'});await group.getByRole('button',{name:'刷新账单'}).click();await expect(group.getByRole('alert')).toContainText('登录已失效');await expect(group.getByRole('cell',{name:'150',exact:true})).toHaveCount(0)
 await group.getByRole('button',{name:'断开控制台'}).click();await expect(group.getByText('连接 DeepSeek 控制台',{exact:true})).toBeVisible()
 expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='save_provider_credential'))).toHaveLength(0)
})


test('OpenCode console remains separate from the Go key, preserves exact counters and clears signed-out bills',async({page})=>{
 await page.getByRole('button',{name:'暂不设置'}).click()
 await page.evaluate(()=>{const w=window as any;w.testOpenCodeProvider=true;return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,accounts:[]}})})
 await page.getByRole('button',{name:'OpenCode Go',exact:true}).click();const group=page.getByRole('region',{name:'OpenCode Go 控制台账单'})
 await group.getByRole('button',{name:'登录控制台',exact:true}).click();await group.getByRole('button',{name:'已登录，连接控制台'}).click();await expect(group.getByRole('button',{name:'刷新账单'})).toBeDisabled()
 expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='get_opencode_console_history'))).toHaveLength(0)
 await page.evaluate(()=>{const w=window as any;w.testOpenCodeHistory={workspace:'org',workspaceName:'测试工作区',days:[{date:'2026-10-11',input:'9007199254740993',output:'20',cacheRead:'30',cacheWrite:'5',tokens:'9007199254741048',cost:null,unpricedTokens:'9007199254741048',models:{m:'9007199254741048'}}],checkedAt:new Date().toISOString(),partial:true,notes:['未读区间已保留']};return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,enabledProviders:['opencode-go']}})})
 await expect(group.getByRole('cell',{name:'9,007,199,254,741,048',exact:true})).toBeVisible();await expect(group.getByRole('cell',{name:'—',exact:true})).toBeVisible();await expect(group.getByRole('status')).toContainText('未读区间')
 await page.evaluate(()=>{(window as any).testOpenCodeError='控制台登录已失效'});await group.getByRole('button',{name:'刷新账单'}).click();await expect(group.getByRole('alert')).toContainText('登录已失效');await expect(group.getByRole('cell',{name:'9,007,199,254,741,048',exact:true})).toHaveCount(0)
 await group.getByRole('button',{name:'断开控制台'}).click();await expect(group.getByText('连接 OpenCode 控制台',{exact:true})).toBeVisible()
 expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='save_provider_credential'))).toHaveLength(0)
})

test('arriving console rows stay visible during paging and disabled panes ignore late progress',async({page})=>{
 await page.getByRole('button',{name:'暂不设置'}).click()
 await page.evaluate(()=>{const w=window as any;w.testOpenCodeProvider=true;w.testOpenCodeConnected=true;w.testOpenCodeHold=true;return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,accounts:[],enabledProviders:['opencode-go']}})})
 await page.getByRole('button',{name:'OpenCode Go',exact:true}).click();const group=page.getByRole('region',{name:'OpenCode Go 控制台账单'});await expect.poll(()=>page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='get_opencode_console_history').length)).toBe(1)
 const result={workspace:'org',workspaceName:null,days:[{date:'2026-10-11',input:'100',output:'20',cacheRead:'30',cacheWrite:'5',tokens:'155',cost:.01,unpricedTokens:'0',models:{m:'155'}}],checkedAt:null,partial:true,notes:[]}
 await page.evaluate(value=>(window as any).testEmit('opencode-console-history',value),result);await expect(group.getByRole('cell',{name:'155',exact:true})).toBeVisible();await expect(group.getByRole('button',{name:'刷新账单'})).toBeDisabled()
 await page.evaluate(()=>{const w=window as any;return w.__TAURI_INTERNALS__.invoke('save_preferences',{value:{...w.testPrefs,enabledProviders:[]}})})
 await expect(group.getByRole('cell',{name:'155',exact:true})).toHaveCount(0)
 await page.evaluate(value=>{const w=window as any;w.testEmit('opencode-console-history',value);w.resolveOpenCode(value)},result);await expect(group.getByRole('cell',{name:'155',exact:true})).toHaveCount(0);await expect(group.getByRole('button',{name:'刷新账单'})).toBeDisabled()
})
