import { chromium } from '@playwright/test'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'

// Real WebView2 UI/IPC, synthetic readings, and no enabled backend accounts.
// Actual alert decisions are exercised separately in alerts.rs, with fixed time.
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms))
await mkdir(path.join(root,'test-results'),{recursive:true})
const profile=await mkdtemp(path.join(root,'test-results/native-profile-pacing-'))
const dir=path.join(profile,'PulseWin');await mkdir(dir,{recursive:true})
await writeFile(path.join(dir,'preferences.json'),JSON.stringify({enabledProviders:[],panelVisible:false,tokenSpendEnabled:false}))
const personal=path.join(process.env.APPDATA,'PulseWin/preferences.json')
const readPersonal=()=>readFile(personal).catch(error=>{if(error.code==='ENOENT')return null;throw error})
const before=await readPersonal(),port=19483
const env={...process.env,APPDATA:profile,WEBVIEW2_USER_DATA_FOLDER:path.join(profile,'webview'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`}
for(const key of Object.keys(env))if(/^PULSEWIN_/i.test(key))delete env[key]
const child=spawn(path.join(root,'src-tauri/target/release/pulsewin.exe'),['--settings','--native-smoke'],{cwd:root,env,windowsHide:true,stdio:'ignore'})
let browser,page
try{
  for(let i=0;i<50;i++){try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`,{timeout:1000});break}catch{await delay(200)}}
  assert(browser,'Native WebView2 must start')
  for(let i=0;i<40;i++){page=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('view=settings'));if(page)break;await delay(200)}
  assert(page)
  const call=(cmd,args={},target=page)=>target.evaluate(({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args})
  await page.getByRole('button',{name:'暂不设置',exact:true}).click()
  await page.getByRole('button',{name:'通知',exact:true}).click()
  await page.getByText('达到门槛且用量进度高于时间进度时提醒。',{exact:true}).waitFor()
  await page.getByLabel('额度提醒阈值').selectOption('75')
  for(let i=0;i<30;i++){if((await call('get_preferences')).alerts.threshold===75)break;await delay(100)}
  const saved=await call('get_preferences');assert.equal(saved.alerts.threshold,75);assert.deepEqual(saved.enabledProviders,[])
  assert.equal(JSON.parse(await readFile(path.join(dir,'preferences.json'),'utf8')).alerts.threshold,75)
  await page.getByRole('button',{name:'通用',exact:true}).click();await page.getByRole('button',{name:'通知',exact:true}).click()
  assert.equal(await page.getByLabel('额度提醒阈值').inputValue(),'75')
  await page.screenshot({path:path.join(root,'test-results/native-pacing-settings.png')})
  const main=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().includes('view='));assert(main)
  const show=async used=>{
    await main.evaluate(async({saved,used})=>{
      const invoke=window.__TAURI_INTERNALS__.invoke,now=Date.now()
      await invoke('plugin:event|emit',{event:'preferences-changed',payload:{...saved,enabledProviders:['codex'],providerOrder:['codex'],showSecondRing:true,warningThreshold:.75,accountAppearance:{codex:{animatedMark:false,detailedCard:false,ringColour:null}}}})
      await invoke('plugin:event|emit',{event:'usage-updated',payload:{providers:[{id:'codex',name:'Codex',configured:true,error:null,stale:false,fetchedAt:new Date(now).toISOString(),windows:[{id:'five',label:'5 小时',percentUsed:used,windowSeconds:18000,resetsAt:new Date(now+4500000).toISOString()},{id:'week',label:'每周',percentUsed:75,windowSeconds:604800,resetsAt:new Date(now+151200000).toISOString()}]}],fetchedAt:new Date(now).toISOString()}})
      await invoke('plugin:event|emit',{event:'hover-changed',payload:'entry:codex'})
      await invoke('plugin:window|show',{label:'main'})
    },{saved,used})
    await main.locator('.card__bar-fill').first().waitFor()
    const expected=used===75?'rgb(0, 230, 140)':'rgb(255, 79, 66)'
    for(let i=0;i<30;i++){if(await main.locator('.card__bar-fill').first().evaluate(e=>getComputedStyle(e).backgroundColor)===expected)break;await delay(100)}
    assert.equal(await main.locator('.card__bar-fill').first().evaluate(e=>getComputedStyle(e).backgroundColor),expected)
  }
  await show(75)
  assert.deepEqual(await main.locator('.rail__item .ring > circle[stroke-dasharray]').evaluateAll(nodes=>nodes.map(n=>n.getAttribute('stroke'))),['#00E68C','#00E68C'])
  await main.locator('.card').screenshot({path:path.join(root,'test-results/native-pacing-equal.png')})
  await show(80)
  assert.deepEqual(await main.locator('.rail__item .ring > circle[stroke-dasharray]').evaluateAll(nodes=>nodes.map(n=>n.getAttribute('stroke'))),['#FF4F42','#00E68C'])
  await main.locator('.card').screenshot({path:path.join(root,'test-results/native-pacing-ahead.png')})
  assert.deepEqual((await call('get_preferences')).enabledProviders,[])
  assert.deepEqual((await call('get_snapshot')).providers,[])
  assert.deepEqual(await readPersonal(),before)
  const result={passed:true,version:JSON.parse(await readFile(path.join(root,'package.json'),'utf8')).version,testedAt:new Date().toISOString(),readings:'synthetic UI-only snapshots',actualNotificationsDelivered:false,checks:['native settings copy and minimum threshold persistence','real WebView2 and event IPC','75% usage / 75% elapsed remains normal','80% usage / 75% elapsed warns','five-hour and weekly ring colours stay independent','UI fixture never enables backend collection','personal preferences unchanged']}
  await writeFile(path.join(root,'test-results/pacing-smoke.json'),JSON.stringify(result,null,2))
  console.log('PASS: native pacing UI, saved minimum threshold, independent cycles and isolated backend. No real notifications sent.')
}finally{await browser?.close().catch(()=>{});child.kill()}
