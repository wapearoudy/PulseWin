import { chromium } from '@playwright/test'
import { spawn, spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { homedir } from 'node:os'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
const current=JSON.parse(await readFile(path.join(root,'package.json'),'utf8')).version
const parts=current.split('.').map(Number);parts[2]++;const next=parts.join('.')
await mkdir(path.join(root,'test-results'),{recursive:true})
const profile=await mkdtemp(path.join(root,'test-results/native-profile-update-'))
const channel=path.join(profile,'channel');await mkdir(channel,{recursive:true})
const file=path.join(channel,'package.exe'),original=Buffer.from('PulseWin signed update smoke fixture: never executed')
await writeFile(file,original)
const signer=spawnSync(process.execPath,[path.join(root,'node_modules/@tauri-apps/cli/tauri.js'),'signer','sign','--private-key-path',path.join(homedir(),'.pulsewin-signing/updater.key'),'--app-version',next,file],{cwd:root,encoding:'utf8',env:{...process.env,TAURI_SIGNING_PRIVATE_KEY_PASSWORD:''}})
if(signer.status!==0)throw new Error('Fixture signing failed: '+signer.stderr)
const signature=(await readFile(file+'.sig','utf8')).trim()
const manifest={version:next,file:'package.exe',signature,notes:'Native signature acceptance fixture'}
await writeFile(path.join(channel,'latest.json'),JSON.stringify(manifest))
const port=19475
const child=spawn(path.join(root,'src-tauri/target/release/pulsewin.exe'),['--settings','--native-smoke'],{cwd:root,windowsHide:true,stdio:'ignore',env:{...process.env,APPDATA:profile,WEBVIEW2_USER_DATA_FOLDER:path.join(profile,'webview'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`}})
let browser
try{
  for(let i=0;i<40;i++){try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`,{timeout:1000});break}catch{await new Promise(r=>setTimeout(r,250))}}
  assert(browser,'Native update WebView did not start')
  let page
  for(let i=0;i<40;i++){page=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('view=settings'));if(page)break;await new Promise(r=>setTimeout(r,250))}
  assert(page,'Settings not available')
  await page.waitForFunction(()=>!!window.__TAURI_INTERNALS__)
  const call=(cmd,args={})=>page.evaluate(({cmd,args})=>window.__TAURI_INTERNALS__.invoke(cmd,args),{cmd,args})
  const prefs=JSON.stringify(await call('get_preferences'))
  await call('save_update_settings',{settings:{source:channel,automatic:false}})
  const available=await call('check_app_update');assert.equal(available.phase,'available');assert.equal(available.version,next)
  await call('verify_app_update_smoke',{version:next})
  assert.equal((await call('get_update_status')).downloaded,original.length)
  await writeFile(file,Buffer.from('tampered package'))
  await call('check_app_update')
  await assert.rejects(call('verify_app_update_smoke',{version:next}),/签名校验失败/)
  assert.equal((await call('get_update_status')).phase,'error')
  await writeFile(file,original)
  parts[2]++;const forged=parts.join('.')
  await writeFile(path.join(channel,'latest.json'),JSON.stringify({...manifest,version:forged}))
  await call('check_app_update')
  await assert.rejects(call('verify_app_update_smoke',{version:forged}),/签名校验失败/)
  await writeFile(path.join(channel,'latest.json'),JSON.stringify({...manifest,version:current}))
  assert.equal((await call('check_app_update')).phase,'current')
  await assert.rejects(call('install_app_update',{version:next}),/不允许安装/)
  assert.equal(JSON.stringify(await call('get_preferences')),prefs)
  await call('show_settings',{provider:null})
  await page.goto(page.url().split('?')[0]+'?view=settings&account=general')
  // A fresh profile intentionally starts with explicit provider selection.
  // Enter General without selecting any account, just as a user would.
  await page.getByRole('button',{name:'通用',exact:true}).click()
  await page.getByRole('button',{name:'检查更新',exact:true}).waitFor()
  await page.screenshot({path:path.join(root,'test-results/native-updates.png')})
  const checks=['signed local download with real Tauri plugin','tampered bytes rejected','forged version rejected','same version skipped','native test install blocked','account preferences unchanged']
  await writeFile(path.join(root,'test-results/update-smoke.json'),JSON.stringify({passed:true,testedAt:new Date().toISOString(),version:current,checks},null,2))
  console.log('PASS: signed local update download, tamper/version rejection, same-version skip and preferences preservation. No installer executed.')
}finally{await browser?.close().catch(()=>{});child.kill()}
