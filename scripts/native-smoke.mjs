import { chromium } from '@playwright/test'
import { spawn } from 'node:child_process'
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
  const main = browser.contexts().flatMap(c => c.pages()).find(p => !p.url().includes('view=settings'))
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
    await invoke('plugin:event|emit',{event:'preferences-changed',payload:{...prefs,roundEnds:false,enabledProviders:ids,providerOrder:ids,accountAppearance:Object.fromEntries(ids.map((id,i)=>[id,{animatedMark:true,persona:['calm','eager','steady','curious','sleepy','playful','stoic'][i],body:'blob'}]))}})
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
  await main.locator('.rail').screenshot({path:path.join(root,'test-results/native-assistant.png')})
  await writeFile(path.join(root, 'test-results/native-smoke.json'), JSON.stringify({
    passed: true, checks: ['native startup', 'no automatic collection', 'settings IPC permission', 'preferences persistence', 'shared panel preferences', 'floating rail restoration', 'native capsule geometry', 'floating stays expanded'],
    profile, testedAt: new Date().toISOString(),
  }, null, 2))
  console.log('PASS: native startup, isolated preferences, restored floating rail coordinates, capsule/ring geometry and floating visibility.')
  if(process.env.PULSEWIN_TEST_KEEP_OPEN==='1') {
    await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('plugin:window|hide',{label:'settings'}))
    console.log('NATIVE_READY: isolated assistant available for real input validation')
    await new Promise(resolve=>setTimeout(resolve,180000))
  }
} catch (error) {
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
