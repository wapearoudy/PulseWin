import { chromium } from '@playwright/test'
import { spawn, spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'

// Real WebView2 / Tauri IPC. Fresh APPDATA ensures the test never changes the
// user's PulseWin preferences and never selects a real account for collection.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
await mkdir(path.join(root, 'test-results'), { recursive: true })
const profile = await mkdtemp(path.join(root, 'test-results', 'native-profile-'))
await mkdir(path.join(profile, 'PulseWin'), {recursive:true})
await writeFile(path.join(profile, 'PulseWin/panel.json'), JSON.stringify({dockEdge:null,railPosition:[720,80],facingEdge:'right'}))
const port = Number(process.env.PULSEWIN_TEST_CDP_PORT || 19473)
const child = spawn(path.join(root, 'src-tauri/target/release/pulsewin.exe'), ['--settings','--native-smoke'], {
  cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
  env: { ...process.env, APPDATA: profile,
    WEBVIEW2_USER_DATA_FOLDER: path.join(root, 'test-results', `WebView2-${path.basename(profile)}`),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  },
})
let diagnostics = ''
child.stderr.on('data', chunk => { diagnostics = (diagnostics + chunk.toString()).slice(-12000) })
let browser
let page
let main
try {
  for (let attempt = 0; attempt < 40; attempt++) {
    if (child.exitCode !== null) throw new Error(`Native app exited: ${child.exitCode}\n${diagnostics}`)
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break }
    catch { await new Promise(resolve => setTimeout(resolve, 250)) }
  }
  assert(browser, 'WebView2 remote debugging endpoint did not start')
  for (let attempt = 0; attempt < 40; attempt++) {
    page = browser.contexts().flatMap(c => c.pages()).find(p => p.url().includes('view=settings'))
    if (page) break
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  assert(page, 'Native settings window was not created')
  await page.getByRole('heading', { name: '选择要监控的服务' }).waitFor()
  assert.equal(await page.getByRole('button', { name: '完成', exact: true }).isDisabled(), true)
  const initial = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('get_snapshot'))
  assert.deepEqual(initial.providers, [], 'First launch must not collect any provider')
  await page.getByRole('button', { name: '暂不设置' }).click()
  await page.getByRole('switch', { name: '圆润端点' }).click()
  await page.waitForFunction(() => document.querySelector('[aria-label="圆润端点"]')?.checked && !document.querySelector('[aria-label="圆润端点"]')?.disabled)
  const prefs = JSON.parse(await readFile(path.join(profile, 'PulseWin/preferences.json'), 'utf8'))
  assert.equal(prefs.roundEnds, true)
  assert.deepEqual(prefs.enabledProviders, [])
  main = browser.contexts().flatMap(c => c.pages()).find(p => !p.url().includes('view=settings'))
  assert(main, 'Native panel webview missing')
  const panelPrefs = await main.evaluate(() => window.__TAURI_INTERNALS__.invoke('get_preferences'))
  assert.equal(panelPrefs.roundEnds, true, 'Panel and settings must use the same persisted preferences')
  assert.equal(await page.getByRole('alert').count(), 0)
  const application=await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('get_application_settings'))
  assert.equal(application.openSettingsShortcut,null)
  assert.equal(application.togglePanelShortcut,null)
  assert.deepEqual(application.unavailable,{})
  assert.equal(application.autostartError,null,'Native startup status must be read without changing it')
  assert.equal(typeof application.autostartEnabled,'boolean')
  // Additional accounts are created disabled, so synthetic secrets never leave
  // this isolated profile or cause a request to a real service.
  const added=await page.evaluate(async()=>{
    const invoke=window.__TAURI_INTERNALS__.invoke;
    await invoke('save_provider_credential',{provider:'codex',apiKey:'native-synthetic-primary',baseUrl:null});
    const first=await invoke('add_account',{provider:'codex',label:'工作账号',credential:JSON.stringify({tokens:{access_token:'native-synthetic-work',account_id:'workspace-work'}}),importLocal:false,enabled:false});
    const second=await invoke('add_account',{provider:'codex',label:'备用账号',credential:'native-synthetic-backup',importLocal:false,enabled:false});
    return {first,second,prefs:await invoke('get_preferences'),rows:await invoke('provider_settings')};
  });
  assert.equal(added.prefs.accounts.length,2);
  assert.deepEqual(added.prefs.enabledProviders,[]);
  const primaryBytes=await readFile(path.join(profile,'PulseWin/codex.json'));
  for(const id of [added.first,added.second]) {
    assert.equal(added.rows.find(row=>row.id===id).providerId,'codex');
    assert.equal(added.rows.find(row=>row.id===id).additional,true);
    const stored=await readFile(path.join(profile,`PulseWin/${id}.json`),'utf8');
    assert.equal(JSON.parse(stored).format,'pulsewin-dpapi');
    assert(!stored.includes('native-synthetic'),'Account credentials must be encrypted');
  }
  await page.evaluate(async({id,prefs})=>{
    const invoke=window.__TAURI_INTERNALS__.invoke;
    // A stale settings window cannot erase metadata created by another window.
    await invoke('save_preferences',{value:{...prefs,accounts:[],accountLabels:{...prefs.accountLabels,[id]:'公司账号'}}});
  },{id:added.first,prefs:added.prefs});
  const renamed=await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('get_preferences'));
  assert.equal(renamed.accounts.length,2);assert.equal(renamed.accountLabels[added.first],'公司账号');
  await page.getByRole('button',{name:'公司账号',exact:true}).click();
  await page.getByRole('heading',{name:'公司账号',exact:true}).waitFor();
  await page.screenshot({path:path.join(root,'test-results/native-multi-account.png')});
  await page.evaluate(async(ids)=>{for(const account of ids)await window.__TAURI_INTERNALS__.invoke('remove_account',{account});},[added.first,added.second]);
  const removed=await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('get_preferences'));
  assert.equal(removed.accounts.length,0);assert.deepEqual(removed.enabledProviders,[]);
  assert.deepEqual(await readFile(path.join(profile,'PulseWin/codex.json')),primaryBytes,'Removing an extra account must preserve primary credentials');
  for(const id of [added.first,added.second]) {
    await assert.rejects(readFile(path.join(profile,`PulseWin/${id}.json`)),{code:'ENOENT'});
  }
  await page.getByRole('button',{name:'外观',exact:true}).click();
  if(process.env.PULSEWIN_TEST_KEEP_OPEN==='1') {
    const hotkey=await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('set_application_shortcut',{action:'openSettings',value:'Control+Alt+Shift+F12'}))
    assert.equal(hotkey.unavailable.openSettings,undefined,'Temporary native test shortcut must register')
  }
  await page.screenshot({ path: path.join(root, 'test-results/native-settings.png') })
  const placement = await main.evaluate(() => window.__TAURI_INTERNALS__.invoke('panel_placement'))
  assert.deepEqual(placement.railPosition, [720,80])
  assert.equal(placement.dockEdge, null)
  // Synthetic usage is emitted only into the isolated test window. It never
  // enables collection or writes credentials/preferences for a real account.
  await main.evaluate(async () => {
    const invoke=window.__TAURI_INTERNALS__.invoke
    const ids=['claude-code','codex','cursor','copilot','deepseek','kiro','glm-coding']
    const prefs=await invoke('get_preferences')
    await invoke('save_preferences',{value:{...prefs,roundEnds:false}})
    await invoke('plugin:event|emit',{event:'preferences-changed',payload:{...prefs,roundEnds:false,enabledProviders:ids,providerOrder:ids,accountAppearance:Object.fromEntries(ids.map(id=>[id,{animatedMark:true,robotFigure:id==='codex'?'cat':null,robotMaterial:id==='codex'?'fluffy':'flat'}]))}})
    await invoke('plugin:event|emit',{event:'usage-updated',payload:{providers:ids.map((id,i)=>({id,name:id,windows:[{label:'5h',percentUsed:[24,13,2,1,0,0,0][i]}],configured:true,error:null})),fetchedAt:new Date().toISOString()}})
    await invoke('plugin:window|show',{label:'main'})
  })
  await main.locator('.rail__item').nth(6).waitFor()
  await main.waitForFunction(() => document.querySelector('.rail')?.getBoundingClientRect().height === 630)
  await main.waitForTimeout(1200)
  assert.equal(await main.locator('.rail__items').evaluate(e=>getComputedStyle(e).opacity),'1')
  const position=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('plugin:window|outer_position',{label:'main'}))
  const rail=await main.locator('.rail').boundingBox()
  const ring=await main.locator('.ring').first().boundingBox()
  assert.equal(ring.width,36,'Clock overlay must not enlarge the ring layout')
  assert.equal(ring.y-rail.y+ring.height/2,40,'First ring must match floating end padding')
  const scale=await main.evaluate(()=>devicePixelRatio)
  assert(Math.abs(position.x+rail.x*scale-720)<=2,'Native rail must restore its X, not window X')
  assert(Math.abs(position.y+rail.y*scale-80)<=2,`Native rail must restore its Y: ${JSON.stringify({position,rail,scale,placement:await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('panel_placement'))})}`)
  // Reproduce the frame shrinking underneath the right-hand rail at 150% DPI.
  // Only the isolated fixture's HWND is resized and its webview stays loaded.
  const shrink = spawnSync('python', ['-c', `
import ctypes as c, sys
from ctypes import wintypes as w
u = c.WinDLL('user32', use_last_error=True)
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
windows = []
def find(hwnd, _):
    owner = w.DWORD()
    u.GetWindowThreadProcessId(hwnd, c.byref(owner))
    title = c.create_unicode_buffer(100)
    u.GetWindowTextW(hwnd, title, 100)
    if owner.value == int(sys.argv[1]) and title.value == 'PulseWin' and u.IsWindowVisible(hwnd):
        windows.append(hwnd)
    return True
u.EnumWindows(callback(find), 0)
assert len(windows) == 1
hwnd = windows[0]
frame = w.RECT()
assert u.GetWindowRect(hwnd, c.byref(frame))
assert u.SetWindowPos(hwnd, 0, 0, 0, int(sys.argv[2]), frame.bottom-frame.top, 0x16), c.get_last_error()
`, String(child.pid), String(Math.floor(rail.x * scale))], { encoding: 'utf8', windowsHide: true })
  assert.equal(shrink.status, 0, shrink.stderr || shrink.error?.message)
  await main.waitForFunction(() => document.querySelector('.rail')?.getBoundingClientRect().right <= innerWidth + 1, undefined, { timeout: 2500 })
  await main.waitForFunction(() => {
    const bots = Array.from(document.querySelectorAll('.rail .robot-avatar'))
    return bots.length === 7 && bots.every(bot => {
      if (bot.getAttribute('data-ready') !== 'true') return false
      const canvas = bot.querySelector('canvas')
      if (!canvas) return Boolean(bot.querySelector('.robot-avatar__drawing svg'))
      const context = canvas.getContext('2d')
      if (!context || !canvas.width || !canvas.height) return false
      return context.getImageData(0, 0, canvas.width, canvas.height).data.some((value, index) => index % 4 === 3 && value > 0)
    })
  }, undefined, { timeout: 10000 })
  const nativeRobotMaterial = await main.locator('.rail .robot-avatar[data-figure="cat"]').first().getAttribute('data-material')
  assert(['flat', 'fluffy'].includes(nativeRobotMaterial), 'Native robot must paint a supported material')
  console.log(`Native WebView2 robot drew material: ${nativeRobotMaterial}`)
  await main.locator('.rail').screenshot({path:path.join(root,'test-results/native-assistant.png')})
  // Real native placement/preferences, with synthetic display-only readings.
  // This exercises IPC, work-area geometry and on-disk persistence without
  // collecting an account or changing the user's installed profile.
  await page.evaluate(async()=>{
    const invoke=window.__TAURI_INTERNALS__.invoke,prefs=await invoke('get_preferences')
    if(prefs.enabledProviders.length)throw new Error('Native fixture must never enable collection')
    // Synthetic events also reach the settings UI. Save only authoritative
    // disabled preferences, never its visual-only fixture state.
    await invoke('save_preferences',{value:{...prefs,panelColour:'light',enabledProviders:[]}})
  })
  await main.waitForFunction(()=>document.querySelector('.panel-root')?.getAttribute('data-colour')==='light')
  assert.equal(JSON.parse(await readFile(path.join(profile,'PulseWin/preferences.json'),'utf8')).panelColour,'light')
  assert.equal(await main.locator('.rail__surface').evaluate(e=>getComputedStyle(e).fill),'rgb(247, 247, 247)')
  await main.evaluate(async()=>{
    const invoke=window.__TAURI_INTERNALS__.invoke,prefs=await invoke('get_preferences')
    const ids=['claude-code','codex','cursor','copilot','deepseek','kiro','glm-coding']
    await invoke('plugin:event|emit',{event:'preferences-changed',payload:{...prefs,enabledProviders:ids,providerOrder:ids,accountAppearance:Object.fromEntries(ids.map((id,i)=>[id,{animatedMark:true,persona:['calm','eager','steady','curious','sleepy','playful','stoic'][i],body:'blob'}]))}})
    await invoke('plugin:event|emit',{event:'usage-updated',payload:{providers:ids.map((id,i)=>({id,name:id,windows:[{label:'5h',percentUsed:[24,13,2,1,0,0,0][i]}],configured:true,error:null})),fetchedAt:new Date().toISOString()}})
  })
  await main.locator('.rail__item').nth(6).waitFor()
  await main.evaluate(async()=>{
    window.placementTrace=[]
    const api=window.__TAURI_INTERNALS__
    for(const event of ['edge-changed','placement-changed']){
      const handler=api.transformCallback(e=>window.placementTrace.push(e))
      await api.invoke('plugin:event|listen',{event,target:{kind:'Any'},handler})
    }
  })
  const bottom=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('set_panel_position',{position:'bottom'}))
  assert.equal(bottom.dockEdge,'bottom')
  await main.waitForFunction(()=>document.querySelector('.rail')?.getBoundingClientRect().height===64&&document.querySelector('.rail')?.getBoundingClientRect().width===678,{},{timeout:10000})
  const docked=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('panel_placement'))
  assert.equal(docked.dockEdge,'bottom')
  await assert.rejects(main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('set_panel_metrics',{width:342,height:630,railX:278,railY:0,railWidth:64,railHeight:630,expectedEdge:'right'})),/orientation changed/)
  assert.equal((await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('panel_placement'))).dockEdge,'bottom','Obsolete metrics must not undo the selected orientation')
  const area=docked.railSpace.workArea
  const waitForBottomRail = () => main.waitForFunction(async ({ bottom, scale }) => {
    const rail = document.querySelector('.rail')?.getBoundingClientRect()
    if (!rail || Math.abs(rail.bottom - innerHeight) > 2) return false
    const position = await window.__TAURI_INTERNALS__.invoke('plugin:window|outer_position', { label: 'main' })
    return Math.abs(position.y + rail.bottom * scale - bottom) <= 2
  }, { bottom: area.bottom, scale })
  await waitForBottomRail()
  const bottomBox=await main.locator('.rail').boundingBox()
  const bottomWindow=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('plugin:window|outer_position',{label:'main'}))
  assert(Math.abs(bottomWindow.y+(bottomBox.y+bottomBox.height)*scale-area.bottom)<=2,'Bottom rail must attach to the Windows work area')
  await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('plugin:event|emit',{event:'hover-changed',payload:'entry:codex'}))
  await main.locator('.card').waitFor()
  await main.waitForFunction(()=>Number(getComputedStyle(document.querySelector('.rail__items')).opacity)===1)
  const card=await main.locator('.card').boundingBox()
  assert(card.y+card.height<bottomBox.y && bottomBox.y-card.y-card.height<=12,'Bottom card tail must point down next to the rail')
  await main.screenshot({path:path.join(root,'test-results/native-assistant-bottom-light.png')})
  // Fill more rows than this monitor can show. Keep collection disabled and
  // exercise the actual work-area metrics, clip path and WebView2 scrolling.
  const rowCount = Math.ceil((area.bottom - area.top) / scale / 60) + 8
  await main.evaluate(async count => {
    const timestamp = new Date().toISOString()
    const ids = ['claude-code', 'codex', 'cursor', 'copilot', 'deepseek', 'kiro', 'glm-coding']
    await window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {
      event: 'usage-updated',
      payload: { fetchedAt: timestamp, providers: ids.map(id => ({
        id, name: id, configured: true, fetchedAt: timestamp,
        windows: Array.from({ length: id === 'codex' ? count : 1 }, (_, index) => ({
          id: String(index), label: `Synthetic quota ${index + 1}`, percentUsed: 50,
        })),
      })) },
    })
  }, rowCount)
  await main.waitForFunction(count => document.querySelectorAll('.card__row').length === count, rowCount)
  await waitForBottomRail()
  await main.locator('.panel__card').evaluate(async element => {
    await Promise.allSettled(element.getAnimations().map(animation => animation.finished))
  })
  const longCard = await main.locator('.card').boundingBox()
  const longWindow = await main.evaluate(() => window.__TAURI_INTERNALS__.invoke('plugin:window|outer_position', { label: 'main' }))
  assert(longWindow.y + longCard.y * scale >= area.top - 2, 'Long card must start inside the native work area')
  assert(longWindow.y + (longCard.y + longCard.height) * scale <= area.bottom + 2, 'Long card must end inside the native work area')
  const content = main.locator('.card__content')
  assert(await content.evaluate(element => element.scrollHeight > element.clientHeight), 'Long card must scroll inside its clipped surface')
  await content.hover({ position: { x: 80, y: 70 } })
  await main.mouse.wheel(0, 240)
  await main.waitForFunction(() => document.querySelector('.card__content')?.scrollTop > 0)
  const horizontal=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('set_panel_position',{position:'free-horizontal'}))
  assert.equal(horizontal.dockEdge,null);assert.equal(horizontal.facingEdge,'bottom')
  await main.waitForFunction(()=>document.querySelector('.rail')?.getBoundingClientRect().height===64&&document.querySelector('.rail')?.getBoundingClientRect().width===630)
  const savedPlacement=JSON.parse(await readFile(path.join(profile,'PulseWin/panel.json'),'utf8'))
  assert.equal(savedPlacement.facingEdge,'bottom');assert.equal(savedPlacement.dockEdge,null)
  await main.screenshot({path:path.join(root,'test-results/native-assistant-horizontal-light.png')})
  // Reloading the native webview must recover the horizontal facing edge.
  await main.reload();await main.waitForFunction(()=>document.querySelector('.panel-root')?.getAttribute('data-colour')==='light')
  assert.equal(await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('panel_edge')),'bottom')
  assert.deepEqual((await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('get_preferences'))).enabledProviders,[])
  await writeFile(path.join(root, 'test-results/native-smoke.json'), JSON.stringify({
    passed: true, checks: ['native startup', 'no automatic collection', 'settings IPC permission', 'preferences persistence', 'shared panel preferences', 'floating rail restoration', 'DPI-aware recovery when the native frame clips the rail', 'native capsule geometry', 'floating stays expanded', 'disabled additional accounts never collect', 'per-account encrypted credentials', 'stale window preserves account metadata', 'account rename persists', 'removal preserves primary credentials','light appearance persistence','bottom work-area dock and card','long card scrolls inside the native work area','obsolete layout metrics rejected','horizontal floating persistence after reload'],
    profile, testedAt: new Date().toISOString(),
  }, null, 2))
  console.log('PASS: native frame recovery, light appearance, bottom docking, long-card scrolling, horizontal restoration and obsolete-layout rejection. Test accounts stayed isolated.')
  if(process.env.PULSEWIN_TEST_KEEP_OPEN==='1') {
    await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('plugin:window|hide',{label:'settings'}))
    console.log('NATIVE_READY: isolated assistant available for real input validation')
    await new Promise(resolve=>setTimeout(resolve,180000))
  }
} catch (error) {
  if(main){
    await main.screenshot({path:path.join(root,'test-results/native-assistant-failure.png')}).catch(()=>undefined)
    console.error(await main.evaluate(async()=>({rail:document.querySelector('.rail')?.getBoundingClientRect().toJSON(),items:document.querySelectorAll('.rail__item').length,colour:document.querySelector('.panel-root')?.getAttribute('data-colour'),trace:window.placementTrace,placement:await window.__TAURI_INTERNALS__.invoke('panel_placement')})).catch(()=>null))
  }
  if (page) {
    await page.screenshot({ path: path.join(root, 'test-results/native-failure.png') }).catch(() => undefined)
    console.error(await page.locator('body').innerText().catch(() => 'No readable page'))
  }
  throw error
} finally {
  await page?.evaluate(()=>window.__TAURI_INTERNALS__.invoke('set_application_shortcut',{action:'openSettings',value:null})).catch(()=>undefined)
  await browser?.close().catch(() => undefined)
  child.kill()
}
