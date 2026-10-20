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
    w.spendTestCommands = []; w.spendTestPrefs = prefs; w.spendHold = false; w.spendTestData = { records, sources }
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
        if(command === 'spend_kept_snapshot') return structuredClone(w.spendKept ?? null)
        if(command === 'spend_begin_scan') return `fixture-scan-${++sequence}`
        if(command === 'spend_get_scan') return { id: args.scanId, status: w.spendHold ? 'running' : 'completed', currentSource:'Claude Code',sourceIndex:1,sourceCount:sources.length,error:null,
          snapshot: w.spendHold ? null : { scannedAt:new Date().toISOString(), ...w.spendTestData,notes:[],pricesAt:new Date().toISOString(),pricingStatus:'fresh' } }
        if(command === 'spend_cancel_scan' || command === 'spend_clear_snapshot') return
        if(command === 'recap_export') {if(w.recapFailure)throw new Error('模拟导出失败');return 'saved'}
      },
    }
  })
  await page.goto('/?view=settings')
  await page.getByRole('button',{name:'Token 消耗',exact:true}).click()
})

test('recap uses full-size images, persists privacy and price, and exports each real card',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.recapPaint=[];const original=CanvasRenderingContext2D.prototype.fillText;CanvasRenderingContext2D.prototype.fillText=function(...args:any[]){w.recapPaint.push(args[0]);return original.apply(this,args as [string,number,number])}})
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check();await page.getByRole('button',{name:'月度 / 年度回顾'}).click()
  const region=page.getByRole('region',{name:'月度与年度回顾'}),image=region.getByRole('img');await expect(image).toBeVisible();expect(await image.evaluate((img:HTMLImageElement)=>[img.naturalWidth,img.naturalHeight])).toEqual([1080,1920])
  await region.getByRole('textbox',{name:'回顾月订阅价'}).fill('20');await region.getByRole('textbox',{name:'回顾月订阅价'}).press('Enter');await expect(region.getByRole('button',{name:'订阅回报',exact:true})).toBeVisible()
  await region.getByRole('button',{name:'成绩单',exact:true}).click();expect(await page.evaluate(()=>(window as any).recapPaint)).toContain('E:\\synthetic-project')
  await page.evaluate(()=>(window as any).recapPaint=[]);await region.getByRole('checkbox',{name:'隐藏项目名称'}).check();expect(await page.evaluate(()=>(window as any).recapPaint)).toContain('Project 1');expect(await page.evaluate(()=>(window as any).recapPaint)).not.toContain('E:\\synthetic-project')
  await region.getByRole('button',{name:'保存全部卡片'}).click();await expect(region.getByRole('status')).toContainText('已保存')
  const exported=await page.evaluate(()=>{const calls=(window as any).spendTestCommands.filter((c:any)=>c.command==='recap_export');return calls[0].args.cards.map((card:any)=>{const b=atob(card.png);const u=(offset:number)=>[0,1,2,3].reduce((n,i)=>n*256+b.charCodeAt(offset+i),0);return{name:card.name,width:u(16),height:u(20)}})})
  expect(exported).toHaveLength(6);expect(exported.every((c:any)=>c.width===1080&&c.height===1920&&c.name.endsWith('.png'))).toBe(true)
  await region.getByRole('button',{name:'年度',exact:true}).click();await expect(region.getByRole('button',{name:'年历',exact:true})).toBeVisible();await expect(region.getByRole('button',{name:'月份',exact:true})).toBeVisible()
  await page.evaluate(()=>(window as any).recapFailure=true);await region.getByRole('button',{name:'保存图片',exact:true}).click();await expect(region.getByRole('alert')).toContainText('导出失败')
  expect(await page.evaluate(()=>(window as any).spendTestPrefs.recapMonthlyPrice)).toBe(20);expect(await page.evaluate(()=>(window as any).spendTestPrefs.recapHidesProjects)).toBe(true)
  expect(await page.evaluate(()=>(window as any).spendTestCommands.filter((c:any)=>c.command==='spend_begin_scan'))).toHaveLength(1)
  await page.screenshot({path:'test-results/recap-year.png',fullPage:true})
})

test('year activity views persist, remain independent of range and reuse the scan',async({page})=>{
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  const activity=page.locator('.token-activity');await expect(activity.getByRole('heading',{name:'Token 活动'})).toBeVisible();await expect(activity.getByRole('img')).toHaveAttribute('aria-label',/最近 12 个月/)
  await activity.getByRole('button',{name:'每周',exact:true}).click();expect(await page.evaluate(()=>(window as any).spendTestPrefs.spendActivityView)).toBe('weekly')
  const caption=await activity.getByRole('img').getAttribute('aria-label');await page.getByRole('button',{name:'今天',exact:true}).click();await expect(activity.getByRole('img')).toHaveAttribute('aria-label',caption!)
  await activity.getByRole('button',{name:'累计',exact:true}).click();await expect(activity.locator('svg path')).toHaveCount(2)
  await page.getByRole('button',{name:'外观',exact:true}).click();await page.getByRole('button',{name:'Token 消耗',exact:true}).click();await expect(activity.getByRole('button',{name:'累计',exact:true})).toHaveAttribute('aria-pressed','true')
  expect(await page.evaluate(()=>(window as any).spendTestCommands.filter((c:any)=>c.command==='spend_begin_scan'))).toHaveLength(1)
  await activity.getByRole('button',{name:'每日',exact:true}).click();await page.screenshot({path:'test-results/token-activity.png',fullPage:true})
})

test('unclassified daily counters and missing cache fields are shown without invented zeros',async({page})=>{
  await page.evaluate(()=>{const w=window as any;const row=w.spendTestData.records[0];w.spendTestData.records=[{...row,tally:{input:100,output:20,cacheWrite:0,cacheRead:0,repliesWithoutCacheFields:1},unclassifiedTokens:25,cost:null,costBreakdown:null}]})
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check();await expect(page.getByRole('columnheader',{name:'未分类 Token',exact:true})).toBeVisible()
  const cache=page.locator('.spend-splits>div').filter({hasText:'缓存读取'});await expect(cache.locator('strong')).toHaveText('—');await expect(cache.locator('strong')).toHaveAttribute('title','来源未报告缓存分类')
  await expect(page.locator('.spend-table tbody tr').first().locator('td').last()).toHaveText('25')
})

test('native exact counters drive large-integer sorting and full-value tooltips',async({page})=>{
  await page.evaluate(()=>{const w=window as any;const row=w.spendTestData.records[0];const d=new Date();d.setDate(d.getDate()-1);const previous=`${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`;w.spendTestData.records=[{...row,day:previous,tally:{input:9007199254740992,output:0,cacheWrite:0,cacheRead:0},exactTally:{input:'9007199254740993',output:'0',cacheWrite:'0',cacheRead:'0'},cost:null,costBreakdown:null},{...row,tally:{input:9007199254740992,output:0,cacheWrite:0,cacheRead:0},exactTally:{input:'9007199254740992',output:'0',cacheWrite:'0',cacheRead:'0'},cost:null,costBreakdown:null}]})
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check();await page.getByRole('button',{name:/^Token$/}).click()
  await expect(page.locator('.spend-table tbody tr').first().locator('td').nth(1)).toHaveAttribute('title','9,007,199,254,740,993');await expect(page.locator('.spend-table tbody tr').nth(1).locator('td').nth(1)).toHaveAttribute('title','9,007,199,254,740,992')
})

test('default off reads nothing; model navigation, range changes and sidebar reuse do not reread', async ({page}) => {
  await expect(page.getByRole('switch',{name:'读取本机 Token 用量记录'})).not.toBeChecked()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command.startsWith('spend_')))).toHaveLength(0)
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  await expect(page.getByRole('heading',{name:'每日 Token',exact:true})).toBeVisible()
  await expect(page.locator('.spend-sources>div')).toHaveCount(8)
  await expect(page.locator('.spend-sources')).not.toContainText('尚未移植')
  await expect(page.locator('.spend-sources')).not.toContainText('Grok Build')
  await expect(page.locator('.spend-sources')).not.toContainText('Copilot')
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
  await expect(page.locator('.spend-headline')).toContainText('$0.00')
  await page.getByRole('button',{name:'今天',exact:true}).click()
  await expect(page.getByRole('button',{name:'今天',exact:true})).toHaveAttribute('aria-pressed','true')
  expect(await page.evaluate(() => (window as any).spendTestPrefs.tokenSpendSpan)).toBe('today')
  await page.getByRole('button',{name:'← 返回模型列表',exact:true}).click()
  await page.getByRole('button',{name:'外观',exact:true}).click()
  await page.getByRole('button',{name:'Token 消耗',exact:true}).click()
  await expect(page.getByRole('heading',{name:'小时分布',exact:true})).toBeVisible()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command === 'spend_begin_scan'))).toHaveLength(1)
  await page.getByRole('button',{name:'重新扫描',exact:true}).click()
  await expect(page.getByRole('heading',{name:'小时分布',exact:true})).toBeVisible()
  expect(await page.evaluate(() => (window as any).spendTestCommands.filter((c:any)=>c.command === 'spend_begin_scan'))).toHaveLength(2)
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).uncheck()
  await expect(page.getByRole('heading',{name:'小时分布',exact:true})).toHaveCount(0)
  expect(await page.evaluate(() => (window as any).spendTestCommands.some((c:any)=>c.command === 'spend_clear_snapshot'))).toBe(true)
})

test('retained history stays visible after source files disappear',async({page})=>{
  await page.evaluate(()=>{
    const source=(window as any).spendTestData.sources[0]
    source.files=0;source.cachedFiles=0;source.retainedRecords=6
  })
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).click()
  await expect(page.getByText('0 个记录文件 · 0 个缓存命中 · 保留 6 条历史计数',{exact:true})).toBeVisible()
  await expect(page.getByRole('heading',{name:'每日明细',exact:true})).toBeVisible()
})

test('Harness source, cache buckets, model drilldown and unpriced amounts appear together', async ({page}) => {
  await page.evaluate(() => {
    const w = window as any, day = w.spendTestData.records[0].day
    w.spendTestData = { records: [{ agent: 'dsh', model: 'deepseek-chat', modelName: 'DeepSeek Chat', day, hour: 9,
      session: 'synthetic-harness', project: 'E:/synthetic-harness', tally: { input: 100, output: 40, cacheWrite: 10, cacheRead: 200 },
      cost: null, costBreakdown: null }], sources: [{ id: 'dsh', name: 'DeepSeek Harness', status: 'counted', files: 3,
      cachedFiles: 2, records: 1, origin: 'native', roots: ['E:/synthetic-home/.dsh/sessions'] }] }
  })
  await page.getByRole('switch', { name: '读取本机 Token 用量记录' }).check()
  await expect(page.locator('.spend-headline')).toContainText('350')
  await expect(page.locator('.spend-sources')).toContainText('DeepSeek Harness')
  await expect(page.locator('.spend-sources')).toContainText('3 个记录文件 · 2 个缓存命中')
  const agents = page.getByRole('heading', { name: 'Agent', exact: true }).locator('..')
  await agents.getByRole('button', { name: 'DeepSeek Harness', exact: false }).click()
  await expect(page.getByRole('heading', { name: 'DeepSeek Harness', exact: true })).toBeVisible()
  const models = page.getByRole('heading', { name: '模型', exact: true }).locator('..')
  await models.getByRole('button', { name: 'DeepSeek Chat', exact: false }).click()
  await expect(page.getByRole('heading', { name: 'DeepSeek Chat', exact: true })).toBeVisible()
  await expect(page.locator('.spend-splits')).toContainText('200')
  await expect(page.locator('.spend-splits')).toContainText('40')
  await expect(page.getByText('350 Token 无法计价', { exact: false })).toBeVisible()
  await page.getByText('支持格式与读取位置 · 1 个实际 reader').click()
  await expect(page.getByText('支持普通、版本化及 Zstandard 压缩 JSONL', { exact: false })).toBeVisible()
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


test('fresh native kept counters appear without scanning and expired ones refresh behind the visible result',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.spendKept={scannedAt:new Date().toISOString(),...w.spendTestData,notes:[],pricesAt:null,pricingStatus:'cached'}})
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  await expect(page.getByRole('button',{name:'月度 / 年度回顾'})).toBeVisible()
  expect(await page.evaluate(()=>(window as any).spendTestCommands.filter((c:any)=>c.command==='spend_begin_scan'))).toHaveLength(0)
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).uncheck()
  await page.evaluate(()=>{const w=window as any;w.spendHold=true;w.spendKept.scannedAt=new Date(Date.now()-180_000).toISOString()})
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  await expect(page.getByRole('button',{name:'停止读取'})).toBeVisible();await expect(page.getByRole('button',{name:'月度 / 年度回顾'})).toBeVisible()
  expect(await page.evaluate(()=>(window as any).spendTestCommands.filter((c:any)=>c.command==='spend_begin_scan'))).toHaveLength(1)
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).uncheck();await expect(page.getByRole('button',{name:'月度 / 年度回顾'})).toHaveCount(0)
})


test('Crush recognition explains why reported dollars are not token counters',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.spendTestData.sources.push({id:'crush',name:'Crush',status:'no-data',files:0,cachedFiles:0,records:0,origin:'native',roots:['fixture/projects.json']})})
  await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).check()
  await expect(page.locator('.spend-sources')).toContainText('已识别 · 来源只报告费用，无法确定 Token 计数')
})
