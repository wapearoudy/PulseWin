import { chromium } from '@playwright/test'
import { spawn } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'

// Only synthetic, credentialless additional accounts. A provider request
// cannot resolve the user's CLI login; cached figures remain explicitly stale.
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
await mkdir(path.join(root,'test-results'),{recursive:true})
const profile=await mkdtemp(path.join(root,'test-results/native-profile-tray-'))
const dir=path.join(profile,'PulseWin');await mkdir(dir,{recursive:true})
const ids=['codex--account-aabb','codex--account-ccdd'],time=new Date().toISOString()
const prefs={accounts:ids.map(id=>({id,provider:'codex'})),accountLabels:{[ids[0]]:'工作账号',[ids[1]]:'备用账号'},enabledProviders:ids,providerOrder:ids,panelVisible:false,trayShowsUsage:true,trayStyle:'ring',trayAccount:ids[0]}
await writeFile(path.join(dir,'preferences.json'),JSON.stringify(prefs))
await writeFile(path.join(dir,'usage-cache.json'),JSON.stringify({version:1,readings:Object.fromEntries(ids.map((id,i)=>[id,{id,name:prefs.accountLabels[id],plan:'Pro',account:null,configured:true,stale:false,error:null,creditRemaining:null,source:'synthetic offline test',fetchedAt:time,windows:[{id:'five-hours',label:'5 小时',percentUsed:i?12:76,resetsAt:new Date(Date.now()+3600000).toISOString(),detail:null,windowSeconds:18000,scope:null,kind:'limit',isExhausted:false}]}]))}))
const port=19477;let child,browser,settings,diagnostics=''
const delay=ms=>new Promise(r=>setTimeout(r,ms))
async function start(){
  child=spawn(path.join(root,'src-tauri/target/release/pulsewin.exe'),['--settings','--native-smoke'],{cwd:root,windowsHide:true,stdio:['ignore','pipe','pipe'],env:{...process.env,APPDATA:profile,WEBVIEW2_USER_DATA_FOLDER:path.join(profile,'webview'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`}})
  child.stderr.on('data',b=>{diagnostics=(diagnostics+b.toString()).slice(-12000)})
  for(let i=0;i<50;i++){if(child.exitCode!==null)throw Error(`App exited ${child.exitCode}: ${diagnostics}`);try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`,{timeout:500});break}catch{await delay(200)}}
  assert(browser,'Native WebView2 did not start')
  for(let i=0;i<50;i++){settings=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('view=settings'));if(settings)break;await delay(200)}
  assert(settings);await settings.waitForFunction(()=>!!window.__TAURI_INTERNALS__)
}
const call=(cmd,args={},page=settings)=>Promise.race([page.evaluate(({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args}),new Promise((_,reject)=>{const timer=setTimeout(()=>reject(Error(`IPC timed out: ${cmd}`)),20000);timer.unref()})])
async function stop(){
  await browser?.close().catch(()=>{});browser=null
  const running=child;if(running&&running.exitCode===null){const exit=new Promise(r=>running.once('exit',r));running.kill();await Promise.race([exit,delay(1000)])}
}
try{
  await start()
  assert.equal(await call('plugin:window|is_visible',{label:'main'}),false,'Tray-only preferences must hide the assistant on first startup')
  const initial=await call('get_snapshot');assert.equal(initial.providers.length,2)
  await call('show_usage_dashboard')
  let usage
  for(let i=0;i<50;i++){usage=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('view=usage'));if(usage)break;await delay(200)}
  assert(usage,'The real usage window must be created')
  await usage.getByRole('button',{name:'查看工作账号用量'}).waitFor()
  assert.equal(await call('plugin:window|is_visible',{label:'usage'},usage),true)
  await usage.getByRole('tab',{name:'工作账号',exact:true}).click()
  await usage.getByRole('heading',{name:'工作账号',exact:true}).waitFor()
  assert.equal(await usage.getByRole('progressbar',{name:'5 小时'}).getAttribute('aria-valuenow'),'76')
  await usage.getByText(/此账号缺少独立登录信息/).waitFor()
  await usage.waitForFunction(()=>{
    const node=document.querySelector('.tray-content-inner'),frame=document.querySelector('.tray-dashboard');
    const target=Math.max(220,Math.min(560,Math.ceil(node.getBoundingClientRect().height+Array.from(frame.querySelectorAll('.tray-header,.tray-tabs,.tray-footer')).reduce((sum,e)=>sum+e.getBoundingClientRect().height,0)+26)));
    return Math.abs(innerHeight-target)<=2;
  })
  const layout=await usage.locator('main').evaluate(e=>({scrollHeight:e.scrollHeight,clientHeight:e.clientHeight,content:document.querySelector('.tray-content-inner').getBoundingClientRect().height,header:document.querySelector('.tray-header').getBoundingClientRect().height,tabs:document.querySelector('.tray-tabs').getBoundingClientRect().height,footer:document.querySelector('.tray-footer').getBoundingClientRect().height,viewport:innerHeight,root:document.getElementById('root').getBoundingClientRect().height,main:e.getBoundingClientRect().height}));assert(layout.scrollHeight<=layout.clientHeight+1,`Short detail layout: ${JSON.stringify(layout)}`)
  const geometry=await usage.evaluate(async()=>{const invoke=window.__TAURI_INTERNALS__.invoke;return {position:await invoke('plugin:window|outer_position',{label:'usage'}),size:await invoke('plugin:window|inner_size',{label:'usage'}),monitor:await invoke('plugin:window|current_monitor',{label:'usage'}),scale:devicePixelRatio,viewport:[innerWidth,innerHeight]}})
  assert(Math.abs(geometry.viewport[0]-320)<=1,'Dashboard width should be 320 logical pixels')
  const area=geometry.monitor.workArea
  assert(geometry.position.x>=area.position.x&&geometry.position.y>=area.position.y)
  assert(geometry.position.x+geometry.size.width<=area.position.x+area.size.width+2)
  assert(geometry.position.y+geometry.size.height<=area.position.y+area.size.height+2)
  await usage.screenshot({path:path.join(root,'test-results/native-tray-account.png')})
  await usage.getByRole('tab',{name:'概览',exact:true}).click()
  await usage.screenshot({path:path.join(root,'test-results/native-tray-overview.png')})
  // Opt-in source modes are saved through the actual Settings UI and IPC.
  await call('show_settings',{provider:null})
  await settings.getByRole('button',{name:'通用',exact:true}).click()
  await settings.getByLabel('托盘用量样式').selectOption('split')
  await settings.waitForFunction(()=>document.querySelector('[aria-label="托盘用量样式"]')?.value==='split'&&!document.querySelector('[aria-label="托盘用量样式"]')?.disabled)
  const saved=JSON.parse(await readFile(path.join(dir,'preferences.json'),'utf8'))
  assert.equal(saved.trayStyle,'split');assert.deepEqual(saved.enabledProviders,ids);assert.equal(saved.panelVisible,false)
  await call('set_panel_visible',{visible:true})
  assert.equal(await call('plugin:window|is_visible',{label:'main'}),true)
  await call('set_panel_visible',{visible:false})
  assert.equal(await call('plugin:window|is_visible',{label:'main'}),false)
  assert.deepEqual((await call('get_preferences')).enabledProviders,ids,'Hiding must not disable collection')
  assert.equal((await call('get_snapshot')).providers.length,2)
  await call('show_usage_dashboard')
  await usage.keyboard.press('Escape')
  await delay(100);assert.equal(await call('plugin:window|is_visible',{label:'usage'}),false,'Escape hides the real native window')
  assert.equal(await usage.getByRole('alert').count(),0)
  await stop();await start()
  assert.equal(await call('plugin:window|is_visible',{label:'main'}),false,'The assistant stays hidden after a real application restart')
  assert.equal((await call('get_preferences')).trayStyle,'split')
  assert.equal((await call('get_snapshot')).providers.length,2)
  await writeFile(path.join(root,'test-results/tray-smoke.json'),JSON.stringify({passed:true,profile,testedAt:new Date().toISOString(),geometry,checks:['native popup IPC and permissions','320 logical pixel width','content height adapts without resizing assistant','clamped to monitor work area','independent additional-account tabs','real settings style persistence','tray-only monitoring remains enabled','assistant visibility persists after process restart','Escape hides popup'],notTested:['physical taskbar mouse click','mixed-DPI displays','visual sharpness of OS-scaled tray icon']},null,2))
  console.log('PASS: native tray dashboard, exact account tabs, compact work-area geometry, saved style, persistent tray-only startup and Escape hide.')
}catch(error){console.error(diagnostics);await settings?.screenshot({path:path.join(root,'test-results/tray-smoke-failure.png')}).catch(()=>{});throw error}
finally{await stop()}
