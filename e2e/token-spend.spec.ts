import { test, expect } from '@playwright/test'

// Every IPC reply below is synthetic. No native agent logs, credentials, prices
// or browser profiles are read by these interaction checks.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any
    let sequence = 0
    const callbacks: Record<number, Function> = {}, listeners: Record<number, any> = {}
    const prefs = { enabledProviders: ['claude-code'], providerOrder: ['claude-code','codex'], tokenSpendEnabled: false, tokenSpendSpan: 'week' }
    const date = new Date(), day = `${date.getFullYear()}-${String(date.getMonth()+1).padStart(2,'0')}-${String(date.getDate()).padStart(2,'0')}`
    const records = Array.from({ length: 12 }, (_, i) => ({ agent: i % 2 ? 'codex' : 'claude', model: `raw-${i}`, modelName: i === 11 ? null : `Model ${i}`, day, hour: i,
      session: `fixture-${i}`, project: 'E:\\synthetic-project', tally: { input:100+i, output:20, cacheWrite:5, cacheRead:25 },
      cost: i === 11 ? null : i === 0 ? 0 : 0.01 * i, costBreakdown: i === 11 ? null : [i * 0.01,0,0,0] }))
    const sources = [['claude','Claude Code'],['codex','Codex'],['qwen','Qwen Code'],['gemini','Gemini CLI'],['cursor','Cursor'],['antigravity','Antigravity'],['hindsight','Hindsight'],['mcode','MCode']].map(([id,name],i) => ({
      id,name,status:i < 2 ? 'counted' : 'not-detected',files:i < 2 ? 6 : 0,cachedFiles:0,records:i < 2 ? 6 : 0,
      origin:i >= 4 ? 'export' : 'native',roots:['E:\\synthetic-token-source\\' + id],
    }))
    w.spendTestCommands = []; w.spendTestPrefs = prefs; w.spendHold = false
    const emit = (event: string, payload: unknown) => Object.values(listeners).filter(x => x.event === event).forEach(x => callbacks[x.handler]?.({ event, payload }))
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_: string, id: number) => delete listeners[id] }
    w.__TAURI_INTERNALS__ = {
      transformCallback: (fn: Function) => { callbacks[++sequence] = fn; return sequence },
      invoke: async (command: string, args: any = {}) => {
        w.spendTestCommands.push({ command, args })
        if(command === 'plugin:event|listen') { listeners[++sequence] = args; return sequence }
        if(command === 'plugin:event|unlisten') return
        if(command === 'get_preferences') return structuredClone(prefs)
        if(command === 'save_preferences') { Object.assign(prefs,args.value); emit('preferences-changed',structuredClone(prefs)); return }
        if(command === 'provider_settings') return [{id:'claude-code',name:'Claude Code',configured:true,stored:false,hints:[],credentialPath:null},{id:'codex',name:'Codex',configured:true,stored:false,hints:[],credentialPath:null}]
        if(command === 'panel_placement') return {dockEdge:'right',railPosition:null}
        if(command === 'get_snapshot') return {providers:[],fetchedAt:new Date().toISOString()}
        if(command === 'spend_begin_scan') return `fixture-scan-${++sequence}`
        if(command === 'spend_get_scan') return { id: args.scanId, status: w.spendHold ? 'running' : 'completed', currentSource:'Claude Code',sourceIndex:1,sourceCount:sources.length,error:null,
          snapshot: w.spendHold ? null : { scannedAt:new Date().toISOString(), records, sources,notes:[],pricesAt:new Date().toISOString(),pricingStatus:'fresh' } }
        if(command === 'spend_cancel_scan' || command === 'spend_clear_snapshot') return
      },
    }
  })
  await page.goto('/?view=settings')
  await page.getByRole('button',{name:'Token 消耗',exact:true}).click()
})

test('default off reads nothing; model navigation, range changes and sidebar reuse do not reread', async ({page}) => {
  await expect(page.getByRole('switch',{name:'读取本机 Token 用量记录'})).not.toBeChecked()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command.startsWith('spend_')))).toHaveLength(0)
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  await expect(page.getByRole('heading',{name:'每日 Token',exact:true})).toBeVisible()
  await expect(page.locator('.spend-sources>div')).toHaveCount(8)
  await expect(page.getByText('其他 43 个原版来源尚未移植')).toBeVisible()
  await page.getByText('支持格式与读取位置 · 8 个实际 reader').click()
  await expect(page.getByText('需要事先导出 Cursor', { exact: false })).toBeVisible()
  expect(await page.evaluate(() => (window as any).spendTestPrefs.tokenSpendEnabled)).toBe(true)
  const models = page.getByRole('heading',{name:'模型',exact:true}).locator('..')
  await expect(models.locator('.spend-group-row')).toHaveCount(8)
  await models.getByRole('button',{name:'下一页',exact:true}).click()
  await expect(models.getByRole('button',{name:'Model 0',exact:false})).toBeVisible()
  await models.getByRole('button',{name:'Model 0',exact:false}).click()
  await expect(page.getByRole('heading',{name:'Model 0',exact:true})).toBeVisible()
  await expect(page.getByRole('heading',{name:'Token 分类',exact:true})).toBeVisible()
  await expect(page.locator('.spend-headline')).toContainText('US$0.00')
  await page.getByRole('button',{name:'今天',exact:true}).click()
  await expect(page.getByRole('button',{name:'今天',exact:true})).toHaveAttribute('aria-pressed','true')
  expect(await page.evaluate(() => (window as any).spendTestPrefs.tokenSpendSpan)).toBe('today')
  await page.getByRole('button',{name:'← 返回模型列表',exact:true}).click()
  await page.getByRole('button',{name:'外观',exact:true}).click()
  await page.getByRole('button',{name:'Token 消耗',exact:true}).click()
  await expect(page.getByRole('heading',{name:'每日 Token',exact:true})).toBeVisible()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command === 'spend_begin_scan'))).toHaveLength(1)
  await page.getByRole('button',{name:'重新扫描',exact:true}).click()
  await expect(page.getByRole('heading',{name:'每日 Token',exact:true})).toBeVisible()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command === 'spend_begin_scan'))).toHaveLength(2)
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).uncheck()
  await expect(page.getByRole('heading',{name:'每日 Token',exact:true})).toHaveCount(0)
  expect(await page.evaluate(() => (window as any).spendTestCommands.some((c:any)=>c.command === 'spend_clear_snapshot'))).toBe(true)
})

test('an unfinished scan can be stopped and is cancelled on sidebar departure', async ({page}) => {
  await page.evaluate(() => { (window as any).spendHold = true })
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  await expect(page.getByRole('button',{name:'停止读取',exact:true})).toBeVisible()
  await page.getByRole('button',{name:'停止读取',exact:true}).click()
  await expect(page.getByText('读取已停止；未完成的结果没有保存')).toBeVisible()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command === 'spend_cancel_scan'))).toHaveLength(1)
  await page.getByRole('button',{name:'开始读取',exact:true}).click()
  await expect(page.getByRole('button',{name:'停止读取',exact:true})).toBeVisible()
  await page.getByRole('button',{name:'外观',exact:true}).click()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command === 'spend_cancel_scan'))).toHaveLength(2)
  await page.evaluate(() => { (window as any).spendHold = false })
  await page.getByRole('button',{name:'Token 消耗',exact:true}).click()
  await expect(page.getByRole('heading',{name:'每日 Token',exact:true})).toBeVisible()
  await page.screenshot({path:'test-results/token-spend-overview.png',fullPage:true})
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})
