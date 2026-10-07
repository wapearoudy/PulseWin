import { test, expect } from '@playwright/test'

test.beforeEach(async ({page}) => {
  page.on('pageerror',error=>console.error(`Assistant page error: ${error.stack??error.message}`))
  await page.addInitScript(() => {
    const w = window as any
    const callbacks: Record<number, Function> = {}; const listeners: Record<number, any> = {}; let seq=0
    const ids=['claude-code','codex','cursor','copilot','deepseek','kiro','glm-coding']
    const prefs={enabledProviders:ids,providerOrder:ids,panelSize:1,railSpacing:1,roundEnds:false,showsRemaining:false,showPercentages:true,labelAbove:false,autoCollapse:true,warningThreshold:.75,refreshSeconds:120,showResetClock:false,clockRemaining:false,pinnedWindows:{},accountAppearance:Object.fromEntries(ids.map((id,i)=>[id,{animatedMark:true,persona:['calm','eager','steady','curious','sleepy','playful','stoic'][i],body:'blob'}]))}
    const snapshot={providers:ids.map((id,i)=>({id,name:id,plan:null,account:null,windows:[{label:'5h',percentUsed:[24,13,2,1,0,0,0][i],resetsAt:null,detail:null}],configured:true,error:null,fetchedAt:new Date().toISOString()})),fetchedAt:new Date().toISOString()}
    w.testCommands=[]; w.testPrefs=prefs; w.testSnapshot=snapshot
    w.testEmit=(event:string,payload:unknown)=>Object.values(listeners).filter(x=>x.event===event).forEach(x=>callbacks[x.handler]?.({event,payload}))
    w.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:(_:string,id:number)=>delete listeners[id]}
    w.__TAURI_INTERNALS__={transformCallback:(fn:Function)=>{callbacks[++seq]=fn;return seq},invoke:async(cmd:string,args:any={})=>{
      w.testCommands.push({cmd,args})
      if(cmd==='plugin:event|listen'){listeners[++seq]=args;return seq}
      if(cmd==='get_preferences')return prefs
      if(cmd==='get_activity')return {running:[],lastWrite:null,finishedAt:{}}
      if(cmd==='get_resets')return {}
      if(cmd==='get_snapshot'||cmd==='refresh_provider')return snapshot
      if(cmd==='panel_edge')return 'right'
      if(cmd==='panel_placement')return {dockEdge:null,railPosition:[800,80]}
    }}
  })
  await page.goto('/')
  await expect(page.locator('.rail__item')).toHaveCount(7)
})

test('floating assistant matches original capsule geometry and stays expanded',async({page})=>{
  const rail=page.locator('.rail')
  await expect(rail).toHaveCSS('width','64px')
  await expect(rail).toHaveCSS('height','630px')
  const a=await page.locator('.rail__item').nth(0).boundingBox(); const b=await page.locator('.rail__item').nth(1).boundingBox()
  expect(b!.y-a!.y).toBe(88)
  const ring=await page.locator('.ring').first().boundingBox(); const frame=await rail.boundingBox()
  await expect(page.locator('.botmark').first()).toHaveCSS('width','28px')
  expect(ring!.width).toBe(36)
  expect(ring!.y-frame!.y+ring!.height/2).toBe(40)
  await page.waitForTimeout(1200)
  await expect(page.locator('.rail__items')).toHaveCSS('opacity','1')
  await expect(page.locator('.rail__surface')).toHaveCSS('fill','rgb(0, 0, 0)')
  const regions=await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_hit_regions').at(-1).args.regions)
  expect(regions.some((r:any)=>r.id==='rail')).toBe(true)
  const bounds = await page.locator('.rail__surface').evaluate((path: SVGPathElement) => { const b=path.getBBox(); return {x:b.x,y:b.y,w:b.width,h:b.height} })
  expect(bounds).toEqual({x:0,y:0,w:64,h:630})
  await rail.screenshot({path:'test-results/assistant-capsule.png'})
})

test('only circle clicks refresh and a drag never refreshes',async({page})=>{
  const item=page.locator('.rail__item').first(); const box=(await item.boundingBox())!
  await item.click({position:{x:box.width/2,y:50}})
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='refresh_provider'))).toHaveLength(0)
  await item.click({position:{x:box.width/2,y:18}})
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='refresh_provider'))).toHaveLength(1)
  await page.mouse.move(box.x+box.width/2,box.y+18); await page.mouse.down(); await page.mouse.move(box.x+box.width/2+10,box.y+18); await page.mouse.up()
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='refresh_provider'))).toHaveLength(1)
})

test('docked assistant collapses and card hover does not change its frame',async({page})=>{
  await page.evaluate(()=>(window as any).testEmit('hover-changed','entry:codex'))
  await expect(page.locator('.panel__card')).toBeVisible()
  const before=await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)
  await page.evaluate(()=>(window as any).testEmit('hover-changed','card'))
  await expect(page.locator('.panel__card')).toBeVisible()
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)).toBe(before)
  await page.evaluate(()=>{(window as any).testEmit('hover-changed',null);(window as any).testEmit('placement-changed',{dockEdge:'right',railPosition:[1000,0]})})
  await expect(page.locator('.rail__items')).toHaveCSS('opacity','0')
})

test('the refreshing animation belongs only to the clicked account and lasts at least 650ms',async({page})=>{
  const item=page.locator('.rail__item').first()
  await item.click({position:{x:21,y:18}})
  await expect(item.locator('.ring__spin')).toHaveCount(1)
  await expect(page.locator('.rail__item').nth(1).locator('.ring__spin')).toHaveCount(0)
  await page.waitForTimeout(250)
  await expect(item.locator('.ring__spin')).toHaveCount(1)
  await expect(item.locator('.ring__spin')).toHaveCount(0)
})
test('a per-account switch restores the original provider glyph and right click opens the menu',async({page})=>{
  await page.evaluate(()=>{const w=window as any;w.testPrefs.accountAppearance.codex.animatedMark=false;w.testEmit('preferences-changed',w.testPrefs)})
  await expect(page.locator('[data-provider-icon="codex"]')).toBeVisible()
  await expect(page.locator('.rail .botmark')).toHaveCount(6)
  await page.locator('.rail__item').first().click({button:'right'})
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='show_panel_menu'))).toHaveLength(1)
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='begin_drag'))).toHaveLength(0)
})

test('detail card has a continuous filled body and its tail touches the allocated edge',async({page})=>{
  await page.evaluate(()=>(window as any).testEmit('hover-changed','entry:codex'))
  await expect(page.locator('.card')).toHaveCSS('padding','0px')
  await expect(page.locator('.card')).toHaveCSS('border-width','0px')
  await expect(page.locator('.card')).toHaveCSS('background-color','rgba(0, 0, 0, 0)')
  await expect(page.locator('.panel__card')).toBeVisible()
  const shape=await page.locator('.card__surface').evaluate((path:SVGPathElement)=>{
    const box=path.getBBox(),holes=[]
    for(let x=22;x<248;x+=8)for(let y=22;y<box.height-20;y+=8)if(!path.isPointInFill(new DOMPoint(x,y)))holes.push([x,y])
    return {holes,right:box.x+box.width}
  })
  expect(shape.holes).toEqual([])
  expect(shape.right).toBe(270)
  await page.screenshot({path:'test-results/assistant-detail-card.png'})
})

test('detailed card is opt-in, uses measured history and keeps the native frame fixed',async({page})=>{
  await page.setViewportSize({width:960,height:1100})
  await page.evaluate(()=>{
    const w=window as any,invoke=w.__TAURI_INTERNALS__.invoke
    w.__TAURI_INTERNALS__.invoke=async(cmd:string,args:any)=>{
      if(cmd!=='spend_card_history')return invoke(cmd,args)
      w.testCommands.push({cmd,args});const now=new Date(),day=now.toLocaleDateString('sv-SE')
      const tokens=args.provider==='codex'?180:720
      return {status:'ready',snapshot:{scannedAt:now.toISOString(),records:[{agent:args.provider==='codex'?'codex':'claude',model:'fixture-only-model',modelName:'Fixture model',day,hour:now.getHours(),session:'synthetic',project:null,tally:{input:tokens-80,output:25,cacheWrite:5,cacheRead:50},unclassifiedTokens:0,cost:.5,costBreakdown:[.3,.1,.02,.08],sourceTimestamp:Math.floor(now.getTime()/1000)}],sources:[],notes:[],pricesAt:null,pricingStatus:'unavailable'}}
    }
    w.testPrefs.accountAppearance.codex.detailedCard=true
    w.testPrefs.accountAppearance['claude-code'].detailedCard=true
    w.testEmit('preferences-changed',structuredClone(w.testPrefs))
    w.testEmit('hover-changed','entry:codex')
  })
  await expect(page.getByText('刚刚更新',{exact:true})).toBeVisible()
  await expect(page.getByLabel('本机用量记录')).toHaveCount(0)
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='spend_card_history'))).toHaveLength(0)
  await page.evaluate(()=>{const w=window as any;w.testPrefs.tokenSpendEnabled=true;w.testEmit('preferences-changed',structuredClone(w.testPrefs))})
  await expect(page.getByLabel('本机用量记录')).toBeVisible()
  await expect(page.getByLabel('本机用量记录').getByLabel('今天')).toContainText('180')
  await expect(page.getByRole('img',{name:'最近31天 Token 消耗'})).toBeVisible()
  await expect(page.getByText('32%',{exact:true})).toBeVisible()
  const frameCount=await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='set_panel_metrics').length)
  await page.evaluate(()=>(window as any).testEmit('hover-changed','entry:claude-code'))
  await expect(page.getByLabel('本机用量记录').getByLabel('今天')).toContainText('720')
  await page.evaluate(()=>(window as any).testEmit('hover-changed','entry:codex'))
  await expect(page.getByLabel('本机用量记录').getByLabel('今天')).toContainText('180')
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='spend_card_history'))).toHaveLength(2)
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='set_panel_metrics').length)).toBe(frameCount)
  await page.locator('.panel__card').screenshot({path:'test-results/assistant-detailed-card.png'})
  await page.evaluate(()=>{const w=window as any;w.testPrefs.tokenSpendEnabled=false;w.testEmit('preferences-changed',structuredClone(w.testPrefs))})
  await expect(page.getByLabel('本机用量记录')).toHaveCount(0)
})

test('OpenCode entitlement failure shows no invented zero and opens its own settings',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any;w.testPrefs.enabledProviders=['opencode-go'];w.testPrefs.providerOrder=['opencode-go'];w.testEmit('preferences-changed',w.testPrefs)
    w.testEmit('usage-updated',{fetchedAt:new Date().toISOString(),providers:[{id:'opencode-go',name:'OpenCode Go',configured:true,windows:[],fetchedAt:new Date().toISOString(),source:'OpenCode CLI：synthetic/auth.json',error:'当前密钥所属账号或工作区未检测到 OpenCode Go 订阅。请核对订阅账号。'}]})
    w.testEmit('hover-changed','entry:opencode-go')
  })
  await expect(page.locator('.rail__item')).toHaveCount(1)
  await expect(page.locator('.card__message')).toContainText('未检测到 OpenCode Go 订阅')
  await expect(page.locator('.rail__item')).not.toContainText('0%')
  await page.getByRole('button',{name:'连接设置…'}).click()
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='show_settings').at(-1).args)).toEqual({provider:'opencode-go'})
})

test('local activity drives only its account and a witnessed finish plays once',async({page})=>{
  const codex=page.locator('.rail__item').nth(1),other=page.locator('.rail__item').first()
  await page.evaluate(()=>(window as any).testEmit('activity-changed',{running:['codex'],lastWrite:Date.now(),finishedAt:{}}))
  await expect(codex.locator('.ring__spin')).toHaveCount(0)
  await expect(other.locator('.ring__spin')).toHaveCount(0)
  await page.evaluate(()=>{const w=window as any;w.testPrefs.accountAppearance.codex.animatedMark=false;w.testEmit('preferences-changed',w.testPrefs)})
  await expect(codex.locator('.ring__spin')).toHaveCount(1)
  await page.evaluate(()=>{const w=window as any;w.testPrefs.accountAppearance.codex.animatedMark=true;w.testEmit('preferences-changed',w.testPrefs)})
  await expect(codex.locator('.botmark')).toHaveCount(1)
  await page.evaluate(()=>(window as any).testEmit('activity-changed',{running:[],lastWrite:Date.now(),finishedAt:{codex:Date.now()}}))
  await expect(codex.locator('.ring__spin')).toHaveCount(0)
  await expect(codex.locator('.botmark')).toHaveAttribute('data-state','excited')
  await expect(codex.locator('.botmark')).not.toHaveAttribute('data-state','excited',{timeout:4000})
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='refresh_provider'))).toHaveLength(0)
})

test('a live reset celebrates once without resizing the panel',async({page})=>{
  const bot=page.locator('.rail__item').nth(1).locator('.botmark')
  const count=await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)
  const reset=Date.now()
  await page.evaluate(at=>(window as any).testEmit('resets-changed',{codex:at}),reset)
  await expect(bot).toHaveAttribute('data-state','celebrate')
  await expect(bot).not.toHaveAttribute('data-state','celebrate',{timeout:8500})
  await page.evaluate(at=>(window as any).testEmit('resets-changed',{codex:at}),reset)
  await page.waitForTimeout(100)
  await expect(bot).not.toHaveAttribute('data-state','celebrate')
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)).toBe(count)
})

test('reduced motion settles every robot instead of running frames',async({page})=>{
  await page.emulateMedia({reducedMotion:'reduce'})
  const bots=page.locator('.botmark')
  await page.waitForTimeout(150)
  const before=await bots.evaluateAll(nodes=>nodes.map(n=>n.innerHTML))
  await page.waitForTimeout(1200)
  expect(await bots.evaluateAll(nodes=>nodes.map(n=>n.innerHTML))).toEqual(before)
})

test('forecast is opt-in, evidence-based and stays inside the card',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any;w.testPrefs.showForecast=true;w.testEmit('preferences-changed',w.testPrefs)
    Object.assign(w.testSnapshot.providers[1].windows[0],{windowSeconds:18000,resetsAt:new Date(Date.now()+9000000).toISOString(),percentUsed:80})
    w.testEmit('usage-updated',w.testSnapshot);w.testEmit('hover-changed','entry:codex')
  })
  await expect(page.locator('.card__forecast')).toContainText('预计')
  const row=(await page.locator('.card__forecast').boundingBox())!,card=(await page.locator('.card').boundingBox())!
  expect(row.y+row.height).toBeLessThanOrEqual(card.y+card.height-10)
  const count=await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)
  await page.evaluate(()=>(window as any).testEmit('hover-changed','entry:claude-code'))
  await expect(page.locator('.card__forecast')).toHaveText('')
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)).toBe(count)
})

test('a failed reading keeps its message and connection action within the card',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any;Object.assign(w.testSnapshot.providers[1],{windows:[],error:'无法获取用量，请检查连接凭据。'.repeat(12)})
    w.testEmit('usage-updated',w.testSnapshot);w.testEmit('hover-changed','entry:codex')
  })
  const action=page.getByRole('button',{name:'连接设置…',exact:true})
  await expect(action).toBeVisible()
  const a=(await action.boundingBox())!,b=(await page.locator('.card').boundingBox())!
  expect(a.y+a.height).toBeLessThanOrEqual(b.y+b.height-10)
  await action.click()
  expect(await page.evaluate(()=>(window as any).testCommands.some((c:any)=>c.cmd==='show_settings'))).toBe(true)
})

test('cached Codex quota shows the failed refresh and retries only its account',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any
    Object.assign(w.testSnapshot.providers[1],{windows:[{id:'primary_window',label:'7d',percentUsed:37}],stale:true,error:'连接超时，请检查系统代理。',fetchedAt:new Date(Date.now()-9*3600000).toISOString()})
    w.testEmit('usage-updated',w.testSnapshot);w.testEmit('hover-changed','entry:codex')
  })
  await expect(page.locator('.card__percent')).toContainText('37% 已用')
  await expect(page.locator('.card__footnote')).toContainText('旧读数 · 截至')
  await expect(page.locator('.card__refresh-error')).toContainText('连接超时，请检查系统代理。')
  const retry=page.getByRole('button',{name:'重新检查',exact:true})
  await expect(retry).toBeVisible()
  const a=(await retry.boundingBox())!,b=(await page.locator('.card').boundingBox())!
  expect(a.y+a.height).toBeLessThanOrEqual(b.y+b.height-10)
  await retry.click()
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='refresh_provider').map((c:any)=>c.args.provider))).toEqual(['codex'])
  await page.locator('.card').screenshot({path:'test-results/codex-cached-refresh-error.png'})
  await page.evaluate(()=>{
    const w=window as any;Object.assign(w.testSnapshot.providers[1],{windows:[{id:'primary_window',label:'7d',percentUsed:41}],stale:false,error:null,fetchedAt:new Date().toISOString()});w.testEmit('usage-updated',w.testSnapshot)
  })
  await expect(page.locator('.card__percent')).toContainText('41% 已用')
  await expect(page.locator('.card__refresh-error')).toHaveCount(0)
  await expect(page.locator('.card__footnote')).toHaveCount(0)
  await expect(page.locator('.card__title')).not.toContainText('旧读数')
})

test('balance-only readings show real money without inventing a percentage or resizing',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any;Object.assign(w.testSnapshot.providers[4],{windows:[],creditRemaining:{amount:999999,currency:'CNY'},error:null})
    w.testEmit('usage-updated',w.testSnapshot);w.testEmit('hover-changed','entry:deepseek')
  })
  const item=page.locator('.rail__item').nth(4)
  await expect(item.locator('.rail__percent')).toContainText('999k')
  await expect(item.locator('.rail__percent')).not.toContainText('%')
  await expect(page.locator('.card__balance')).toHaveText('余额CNY 999,999.00')
  await expect(page.locator('.card__bar')).toHaveCount(0)
  await expect(page.locator('.card__message')).toHaveCount(0)
  const count=await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)
  await page.evaluate(()=>{
    const w=window as any;w.testSnapshot.providers[4].creditRemaining.amount=0;w.testEmit('usage-updated',w.testSnapshot)
  })
  await expect(item.locator('.rail__percent')).toContainText('0')
  await expect(page.locator('.card__balance')).toHaveText('余额CNY 0.00')
  expect(await page.evaluate(()=>(window as any).testCommands.filter((c:any)=>c.cmd==='set_panel_metrics').length)).toBe(count)
  await page.evaluate(()=>{const w=window as any;w.testSnapshot.providers[4].error='HTTP 503';w.testSnapshot.providers[4].stale=true;w.testEmit('usage-updated',w.testSnapshot)})
  await expect(item.locator('.rail__percent')).toHaveText('—')
  await expect(page.locator('.card__title')).toContainText('旧读数')
})

test('remaining mode leaves unknown quotas empty and colors both rings by account',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any;w.testPrefs.showsRemaining=true;w.testPrefs.showSecondRing=true
    w.testPrefs.accountAppearance.codex.ringColour='#abcdef'
    Object.assign(w.testSnapshot.providers[0],{windows:[],error:'no reading'})
    w.testSnapshot.providers[1].windows=[{id:'g5',scope:'Gemini',label:'5h',percentUsed:90},{id:'g7',scope:'Gemini',label:'7d',percentUsed:40},{id:'c5',scope:'Claude',label:'5h',percentUsed:80}]
    w.testEmit('preferences-changed',w.testPrefs);w.testEmit('usage-updated',w.testSnapshot)
  })
  await expect(page.locator('.rail__item').first().locator('.ring > circle[stroke-dasharray]')).toHaveCount(0)
  const arcs=page.locator('.rail__item').nth(1).locator('.ring > circle[stroke-dasharray]')
  await expect(arcs).toHaveCount(2)
  expect(await arcs.evaluateAll(nodes=>nodes.map(n=>n.getAttribute('stroke')))).toEqual(['#abcdef','#abcdef'])
  const second=await arcs.nth(1).getAttribute('stroke-dasharray')
  const [painted,rest]=second!.split(' ').map(Number)
  expect(painted/rest).toBeCloseTo(.6,4)
})

test('quota warnings compare time progress on both rings, cards and the collapsed rail',async({page})=>{
  await page.evaluate(()=>{
    const w=window as any,now=Date.now(),quota=(id:string,used:number,elapsed:number,duration:number)=>({id,label:id,percentUsed:used,windowSeconds:duration,resetsAt:new Date(now+duration*(1-elapsed)*1000).toISOString()})
    w.testPrefs.enabledProviders=['codex'];w.testPrefs.showSecondRing=true;w.testPrefs.warningThreshold=.75;w.testPrefs.accountAppearance.codex.ringColour=null
    w.testSnapshot.providers[1].windows=[quota('5h',75,.75,18000),quota('7d',75,.75,604800)]
    w.testEmit('preferences-changed',w.testPrefs);w.testEmit('usage-updated',w.testSnapshot);w.testEmit('hover-changed','entry:codex')
  })
  const arcs=page.locator('.rail__item .ring > circle[stroke-dasharray]')
  await expect(arcs).toHaveCount(2)
  expect(await arcs.evaluateAll(nodes=>nodes.map(n=>n.getAttribute('stroke')))).toEqual(['#00E68C','#00E68C'])
  await expect(page.locator('.card__bar-fill').first()).toHaveCSS('background-color','rgb(0, 230, 140)')
  await page.locator('.card').screenshot({path:'test-results/quota-on-schedule.png'})
  await page.evaluate(()=>{
    const w=window as any;w.testSnapshot.providers[1].windows[0].percentUsed=80
    Object.assign(w.testSnapshot.providers[1].windows[1],{percentUsed:95,resetsAt:new Date(Date.now()+604800*.04*1000).toISOString()})
    w.testEmit('usage-updated',w.testSnapshot)
  })
  await expect.poll(()=>arcs.evaluateAll(nodes=>nodes.map(n=>n.getAttribute('stroke')))).toEqual(['#00E68C','#FF4F42'])
  await expect(page.locator('.card__bar-fill').first()).toHaveCSS('background-color','rgb(255, 79, 66)')
  await expect(page.locator('.card__bar-fill').nth(1)).toHaveCSS('background-color','rgb(0, 230, 140)')
  await page.locator('.card').screenshot({path:'test-results/quota-ahead-of-schedule.png'})
  await page.evaluate(()=>{const w=window as any;w.testEmit('hover-changed',null);w.testEmit('placement-changed',{dockEdge:'right',railPosition:[1000,0]})})
  await expect(page.locator('.rail__items')).toHaveCSS('opacity','0')
  await expect(page.locator('.rail__surface')).toHaveCSS('fill','rgb(255, 79, 66)')
  await page.evaluate(()=>{
    const w=window as any;w.testSnapshot.providers[1].windows.forEach((q:any)=>{q.percentUsed=75;q.resetsAt=new Date(Date.now()+q.windowSeconds*.25*1000).toISOString()});w.testEmit('usage-updated',w.testSnapshot)
  })
  await expect(page.locator('.rail__surface')).toHaveCSS('fill','rgb(0, 0, 0)')
})

test('two Codex accounts keep separate rings, cards and refresh targets',async({page})=>{
  await page.evaluate(()=>{const w=window as any,id='codex--account-ab';
    const base=w.testSnapshot.providers.find((p:any)=>p.id==='codex');
    w.testPrefs.enabledProviders=['codex',id];w.testPrefs.providerOrder=['codex',id];w.testPrefs.accountAppearance={};
    w.testSnapshot.providers=[{...base,name:'个人账号',windows:[{label:'5h',percentUsed:13}]},{...base,id,name:'工作账号',windows:[{label:'5h',percentUsed:68}]}];
    w.testEmit('preferences-changed',w.testPrefs);w.testEmit('usage-updated',w.testSnapshot)
  })
  await expect(page.locator('.rail__item')).toHaveCount(2)
  await expect(page.locator('.rail__item').nth(0)).toContainText('13%');await expect(page.locator('.rail__item').nth(1)).toContainText('68%')
  await page.evaluate(()=>(window as any).testEmit('hover-changed','entry:codex--account-ab'))
  await expect(page.locator('.panel__card')).toContainText('工作账号')
  await page.locator('.rail__item').nth(1).click({position:{x:21,y:18}})
  expect(await page.evaluate(()=>(window as any).testCommands.filter((x:any)=>x.cmd==='refresh_provider').map((x:any)=>x.args.provider))).toEqual(['codex--account-ab'])
  await page.locator('.rail').screenshot({path:'test-results/assistant-multi-account.png'})
})
