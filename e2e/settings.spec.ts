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
        if(cmd === 'get_preferences') return structuredClone(prefs)
        if(cmd === 'save_preferences') { Object.assign(prefs,args.value); emit('preferences-changed',structuredClone(prefs)); return }
        if(cmd === 'provider_settings') return structuredClone(providers)
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
  await expect(page.getByLabel('额度提醒阈值')).toHaveValue('')
  await expect(page.getByRole('switch',{name:'额度重置时通知'})).toBeDisabled()
  await expect(page.getByRole('switch',{name:'连续检查失败时通知'})).not.toBeChecked()
  await page.getByLabel('额度提醒阈值').selectOption('90')
  await page.getByRole('switch',{name:'额度重置时通知'}).check()
  await page.getByRole('switch',{name:'连续检查失败时通知'}).check()
  expect(await page.evaluate(()=>(window as any).testPrefs.alerts)).toEqual({threshold:90,onReset:true,onFailure:true,lowBalance:{}})
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
