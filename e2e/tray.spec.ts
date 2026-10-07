import { test, expect } from '@playwright/test'

test.use({viewport:{width:320,height:560}})
test.beforeEach(async({page})=>{
  await page.addInitScript(()=>{
    const w=window as any;let seq=0;const callbacks:Record<number,Function>={},listeners:Record<number,any>={}
    w.trayCommands=[];w.trayFail=false
    w.trayPrefs={enabledProviders:['claude-code','codex','deepseek','codex--account-ab'],providerOrder:['claude-code','codex','deepseek','codex--account-ab'],panelVisible:true,showsRemaining:false,tokenSpendEnabled:false,accounts:[{id:'codex--account-ab',provider:'codex'}],accountLabels:{'codex--account-ab':'工作账号'}}
    const time=new Date().toISOString(),reset=new Date(Date.now()+3600000).toISOString()
    const quota=(label:string,percentUsed:number|null,extra:any={})=>({label,percentUsed,resetsAt:reset,detail:null,...extra})
    const provider=(id:string,name:string,windows:any[],extra:any={})=>({id,name,windows,configured:true,stale:false,error:null,creditRemaining:null,plan:'Pro',account:null,fetchedAt:time,...extra})
    w.traySnapshot={providers:[provider('claude-code','Claude Code',[quota('5 小时',76),quota('每周',null)]),provider('codex','Codex',[quota('5 小时',100,{isExhausted:true})],{stale:true,error:'连接暂时失败，显示旧读数'}),provider('deepseek','DeepSeek',[],{creditRemaining:{currency:'USD',amount:12.5}}),provider('codex--account-ab','工作账号',[quota('每周',12)])],fetchedAt:time}
    w.trayEmit=(event:string,payload:unknown)=>Object.values(listeners).filter(l=>l.event===event).forEach(l=>callbacks[l.handler]?.({event,payload}))
    w.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:(_:string,id:number)=>delete listeners[id]}
    w.__TAURI_INTERNALS__={transformCallback:(fn:Function)=>{callbacks[++seq]=fn;return seq},invoke:async(cmd:string,args:any={})=>{
      w.trayCommands.push({cmd,args})
      if(cmd==='plugin:event|listen'){listeners[++seq]=args;return seq}
      if(cmd==='plugin:event|unlisten'){delete listeners[args.eventId];return}
      if(cmd==='get_preferences')return structuredClone(w.trayPrefs)
      if(cmd==='get_snapshot')return structuredClone(w.traySnapshot)
      if(cmd==='get_usage_pages')return {'claude-code':'https://claude.ai/settings/usage',codex:'https://chatgpt.com/codex/settings/usage',deepseek:'https://platform.deepseek.com/usage'}
      if(cmd==='set_panel_visible'){if(w.trayFail)throw Error('保存失败');w.trayPrefs.panelVisible=args.visible;w.trayEmit('preferences-changed',structuredClone(w.trayPrefs));return}
      if(cmd==='refresh_provider'||cmd==='refresh_now'){if(w.traySlowRefresh)await new Promise(resolve=>{w.releaseRefresh=resolve});if(w.trayFail)throw Error('刷新失败');return structuredClone(w.traySnapshot)}
      if(cmd==='spend_card_history')return {status:'unavailable',snapshot:null}
      if(cmd==='save_preferences'){Object.assign(w.trayPrefs,args.value);w.trayEmit('preferences-changed',structuredClone(w.trayPrefs));return}
      if(cmd==='provider_settings')return w.traySnapshot.providers.map((p:any)=>({id:p.id,name:p.name,configured:true,stored:false,credentialPath:null,hints:[]}))
    }}
  })
  await page.goto('/?view=usage')
  await expect(page.getByRole('button',{name:'查看Claude Code用量'})).toBeVisible()
})

test('opening and switching accounts never refreshes quota or scans disabled local history',async({page})=>{
  await page.getByRole('tab',{name:'Claude Code',exact:true}).click()
  await expect(page.getByRole('progressbar',{name:'5 小时'})).toHaveAttribute('aria-valuenow','76')
  await expect(page.getByText('— 已用',{exact:true})).toBeVisible()
  await page.getByRole('tab',{name:'工作账号',exact:true}).click()
  await expect(page.getByRole('heading',{name:'工作账号',exact:true})).toBeVisible()
  expect(await page.evaluate(()=>(window as any).trayCommands.filter((c:any)=>['refresh_provider','refresh_now','spend_card_history'].includes(c.cmd)))).toEqual([])
})
test('account refresh is scoped, overview refresh includes enabled accounts',async({page})=>{
  await page.getByRole('tab',{name:'工作账号',exact:true}).click()
  await page.getByRole('button',{name:'刷新此账号'}).click()
  await expect(page.getByRole('button',{name:'刷新此账号'})).toBeEnabled()
  expect(await page.evaluate(()=>(window as any).trayCommands.find((c:any)=>c.cmd==='refresh_provider').args)).toEqual({provider:'codex--account-ab'})
  await page.getByRole('tab',{name:'概览',exact:true}).click()
  await page.getByRole('button',{name:'刷新全部账号'}).click()
  await expect.poll(()=>page.evaluate(()=>(window as any).trayCommands.filter((c:any)=>c.cmd==='refresh_now').length)).toBe(1)
})
test('balance only accounts show real money without zero percent or quota bars',async({page})=>{
  await page.getByRole('tab',{name:'DeepSeek',exact:true}).click()
  await expect(page.getByText('USD 12.50 剩余')).toBeVisible()
  await expect(page.getByRole('progressbar')).toHaveCount(0)
  await expect(page.getByText('0%',{exact:true})).toHaveCount(0)
})
test('remaining exhausted quota remains a full warning bar, stale readings are explicit',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.trayPrefs.showsRemaining=true;w.trayEmit('preferences-changed',w.trayPrefs)})
  await page.getByRole('tab',{name:'Codex',exact:true}).click()
  await expect(page.getByRole('progressbar',{name:'5 小时'})).toHaveAttribute('aria-valuenow','100')
  await expect(page.getByText('0% 剩余 · 已限额')).toBeVisible()
  await expect(page.getByText(/分钟前更新|刚刚更新/)).toContainText('旧读数')
  await expect(page.getByText('连接暂时失败，显示旧读数')).toBeVisible()
})

test('tray overview and account detail stay normal when quota matches time progress',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any,p=w.traySnapshot.providers[0];Object.assign(p.windows[0],{percentUsed:75,windowSeconds:18000,resetsAt:new Date(Date.now()+4500000).toISOString()});w.trayPrefs.warningThreshold=.75;w.trayEmit('preferences-changed',w.trayPrefs);w.trayEmit('usage-updated',w.traySnapshot)
  })
  const overview=page.getByRole('button',{name:'查看Claude Code用量'})
  await expect(overview.locator('.tray-warning')).toHaveCount(0)
  await page.getByRole('tab',{name:'Claude Code',exact:true}).click()
  await expect(page.getByRole('progressbar',{name:'5 小时'})).not.toHaveClass(/tray-warning/)
  await page.evaluate(()=>{const w=window as any;w.traySnapshot.providers[0].windows[0].percentUsed=80;w.trayEmit('usage-updated',w.traySnapshot)})
  await expect(page.getByRole('progressbar',{name:'5 小时'})).toHaveClass(/tray-warning/)
  await page.getByRole('tab',{name:'概览',exact:true}).click()
  await expect(overview.locator('.tray-warning')).toContainText('80%')
})
test('hide panel persists visibility without disabling accounts, failures retain the action',async({page})=>{
  await page.getByRole('button',{name:'隐藏桌面助手'}).click()
  await expect(page.getByRole('button',{name:'显示桌面助手'})).toBeVisible()
  expect(await page.evaluate(()=>(window as any).trayPrefs.enabledProviders)).toHaveLength(4)
  await page.evaluate(()=>(window as any).trayFail=true)
  await page.getByRole('button',{name:'显示桌面助手'}).click()
  await expect(page.getByRole('alert')).toContainText('保存失败')
  await expect(page.getByRole('button',{name:'显示桌面助手'})).toBeEnabled()
})
test('disabled selected account falls back to overview and never reads another account history',async({page})=>{
  await page.getByRole('tab',{name:'工作账号',exact:true}).click()
  await page.evaluate(()=>{const w=window as any;w.trayPrefs.tokenSpendEnabled=true;w.trayEmit('preferences-changed',w.trayPrefs)})
  expect(await page.evaluate(()=>(window as any).trayCommands.filter((c:any)=>c.cmd==='spend_card_history'))).toEqual([])
  await page.evaluate(()=>{const w=window as any;w.trayPrefs.enabledProviders=['claude-code','deepseek'];w.trayEmit('preferences-changed',w.trayPrefs)})
  await expect(page.getByRole('tab',{name:'概览',exact:true})).toHaveAttribute('aria-selected','true')
})
test('official page resolves the exact additional account and Escape hides the popup',async({page})=>{
  await page.getByRole('tab',{name:'工作账号',exact:true}).click()
  await page.getByRole('button',{name:/打开官方用量页面/}).click()
  expect(await page.evaluate(()=>(window as any).trayCommands.find((c:any)=>c.cmd==='open_usage_page').args)).toEqual({account:'codex--account-ab'})
  await page.keyboard.press('Escape')
  await expect.poll(()=>page.evaluate(()=>(window as any).trayCommands.filter((c:any)=>c.cmd==='hide_usage_dashboard').length)).toBe(1)
})
test('keyboard navigates tabs and too many accounts remain accessible without horizontal page overflow',async({page})=>{
  await page.evaluate(()=>{const w=window as any;for(let i=0;i<8;i++){const p={...w.traySnapshot.providers[0],id:`other${i}`,name:`其他账号${i}`};w.traySnapshot.providers.push(p);w.trayPrefs.enabledProviders.push(p.id);w.trayPrefs.providerOrder.push(p.id)}w.trayEmit('preferences-changed',w.trayPrefs);w.trayEmit('usage-updated',w.traySnapshot)})
  await page.getByRole('tab',{name:'概览',exact:true}).focus()
  await page.keyboard.press('End')
  await expect(page.getByRole('tab',{name:'其他账号7'})).toHaveAttribute('aria-selected','true')
  expect(await page.evaluate(()=>document.documentElement.scrollWidth)).toBe(320)
  await expect(page.getByRole('button',{name:'刷新此账号'})).toBeVisible()
})
test('manual refresh failure preserves cached data and can be retried',async({page})=>{
  await page.getByRole('tab',{name:'Claude Code',exact:true}).click()
  await page.evaluate(()=>(window as any).trayFail=true)
  await page.getByRole('button',{name:'刷新此账号'}).click()
  await expect(page.getByRole('alert')).toContainText('刷新失败')
  await expect(page.getByRole('progressbar',{name:'5 小时'})).toHaveAttribute('aria-valuenow','76')
  await expect(page.getByRole('button',{name:'刷新此账号'})).toBeEnabled()
})
test('local spend is read only after enabling it for a primary account',async({page})=>{
  await page.getByRole('tab',{name:'Claude Code',exact:true}).click()
  await page.evaluate(()=>{const w=window as any;w.trayPrefs.tokenSpendEnabled=true;w.trayEmit('preferences-changed',w.trayPrefs)})
  await expect(page.getByText('没有可归属此账号的本机记录')).toBeVisible()
  expect(await page.evaluate(()=>(window as any).trayCommands.filter((c:any)=>c.cmd==='spend_card_history').map((c:any)=>c.args.provider))).toEqual(['claude-code'])
})
for(const colour of ['light','dark'] as const)test(`320px dashboard ${colour} matches compact tab and quota layout`,async({page})=>{
  await page.emulateMedia({colorScheme:colour})
  await page.getByRole('tab',{name:'Claude Code',exact:true}).click()
  const icon=page.locator('.tray-provider-icon').first()
  expect((await icon.boundingBox())?.width).toBe(15)
  expect((await icon.boundingBox())?.height).toBe(15)
  await expect(page.getByRole('button',{name:'刷新此账号'})).toBeVisible()
  await page.screenshot({path:`test-results/tray-${colour}.png`})
})

test('a slow refresh never blocks closing the usage popup',async({page})=>{
  await page.evaluate(()=>(window as any).traySlowRefresh=true)
  await page.getByRole('button',{name:'刷新全部账号'}).click()
  await expect(page.getByRole('button',{name:'正在刷新…'})).toBeDisabled()
  await page.getByRole('button',{name:'关闭用量概览'}).click()
  await expect.poll(()=>page.evaluate(()=>(window as any).trayCommands.filter((c:any)=>c.cmd==='hide_usage_dashboard').length)).toBe(1)
  await page.evaluate(()=>(window as any).releaseRefresh())
})
