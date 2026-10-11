import {chromium} from '@playwright/test'
import {spawn} from 'node:child_process'
import {mkdtemp,mkdir,writeFile,readFile} from 'node:fs/promises'
import path from 'node:path'
import {fileURLToPath} from 'node:url'
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
await mkdir(path.join(root,'test-results'),{recursive:true})
const supplied=process.argv[2]?path.resolve(process.argv[2]):null
if(supplied&&(path.dirname(supplied)!==path.join(root,'test-results')||!path.basename(supplied).startsWith('native-profile-login-')))throw Error('Reuse requires an isolated native-profile-login-* directly under workspace test-results')
const profile=supplied??await mkdtemp(path.join(root,'test-results/native-profile-login-'))
const data=path.join(profile,'PulseWin'),home=path.join(data,'smoke-home')
await mkdir(data,{recursive:true})
let networkProxy
try{networkProxy=JSON.parse(await readFile(path.join(process.env.APPDATA,'PulseWin/preferences.json'),'utf8')).networkProxy}catch{}
if(!supplied)await writeFile(path.join(data,'preferences.json'),JSON.stringify({enabledProviders:['deepseek'],tokenSpendEnabled:false,...(networkProxy?{networkProxy}: {})}))
const environment=Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('PULSEWIN_')))
Object.assign(environment,{APPDATA:profile,LOCALAPPDATA:path.join(data,'smoke-local'),XDG_DATA_HOME:path.join(home,'.local/share'),DSH_HOME:path.join(home,'.dsh'),CODEX_HOME:path.join(home,'.codex'),CLAUDE_CONFIG_DIR:path.join(home,'.claude'),GEMINI_CLI_HOME:path.join(home,'.gemini'),HERMES_HOME:path.join(home,'.hermes'),CRUSH_GLOBAL_DATA:path.join(home,'.crush'),GOOSE_PATH_ROOT:path.join(home,'.goose'),KIMCHI_CODING_AGENT_DIR:path.join(home,'.kimchi'),TOKSCALE_CONFIG_DIR:path.join(home,'.tokscale'),HINDSIGHT_HOME:path.join(home,'.hindsight'),WEBVIEW2_USER_DATA_FOLDER:path.join(profile,'WebView2'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19475 --remote-debugging-address=127.0.0.1'})
const child=spawn(path.join(root,'src-tauri/target/release/pulsewin.exe'),['--settings','--native-smoke'],{cwd:root,windowsHide:true,stdio:['ignore','pipe','pipe'],env:environment})
let browser,page,quitting=false,checking=false,phase='deepseek',evidence={startedAt:new Date().toISOString(),profile,providers:{}},opened=new Set()
const cleanup=async()=>{quitting=true;await browser?.close().catch(()=>{});child.kill()}
process.on('SIGINT',async()=>{await cleanup();process.exit(0)})
const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args})
try{
 for(let i=0;i<100;i++){if(child.exitCode!==null)throw Error('Verification application exited');try{browser=await chromium.connectOverCDP('http://127.0.0.1:19475',{timeout:1000});break}catch{await new Promise(r=>setTimeout(r,250))}}
 if(!browser)throw Error('Verification endpoint unavailable')
 for(let i=0;i<100;i++){page=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('view=settings'));if(page)break;await new Promise(r=>setTimeout(r,250))}
 if(!page)throw Error('Settings page unavailable')
 const dismiss=page.getByRole('button',{name:'暂不设置'});if(await dismiss.isVisible())await dismiss.click()
 async function open(provider){const command=provider==='deepseek'?'deepseek':'opencode';if((await invoke(`get_${command}_console_connection`)).connected){console.log(`${provider}: reusing isolated encrypted connection.`);return;}const label=provider==='deepseek'?'DeepSeek':'OpenCode Go';await page.getByRole('complementary',{name:'设置导航'}).getByRole('button',{name:label,exact:true}).click();const group=page.getByRole('region',{name:provider==='deepseek'?'DeepSeek 控制台账单':'OpenCode Go 控制台账单'});await group.getByRole('button',{name:'登录控制台',exact:true}).click();opened.add(provider);console.log(`${label}: official login window opened. Complete login, return to Settings and click 已登录，连接控制台.`)}
 await open('deepseek')
 console.log('Private verification profile created; installed settings and credentials are untouched.')
 async function check(){if(quitting||checking||phase==='done')return;checking=true;try{const provider=phase,command=provider==='deepseek'?'deepseek':'opencode';const connection=await invoke(`get_${command}_console_connection`);if(!connection.connected)return;let result
 try{const history=await invoke(`get_${command}_console_history`);result={connected:true,billingRead:true,days:history.days.length,partial:history.partial??false,notes:history.notes??[],checkedAt:history.checkedAt};console.log(`${provider}: connected; billing read ${history.days.length} days; partial=${result.partial}. No credentials are printed.`)}catch(e){result={connected:true,billingRead:false,error:String(e)};console.log(`${provider}: connected, billing verification failed: ${String(e)}`)}
 evidence.providers[provider]=result;await writeFile(path.join(profile,'verification.json'),JSON.stringify(evidence,null,2));
 if(provider==='deepseek'){phase='opencode';const prefs=await invoke('get_preferences');await invoke('save_preferences',{value:{...prefs,enabledProviders:['deepseek','opencode-go']}});await open('opencode')}else{phase='done';console.log('Both login flows processed; verification window remains open for review. Send close to finish.')}
 }finally{checking=false}}
 const timer=setInterval(()=>void check().catch(e=>console.log(`Verification check: ${String(e)}`)),5000)
 process.stdin.setEncoding('utf8');process.stdin.on('data',async text=>{if(text.trim()==='check')await check();if(text.trim()==='close'){clearInterval(timer);await cleanup();process.exit(0)}})
 await new Promise(()=>{})
}catch(e){console.error(String(e));await cleanup();process.exit(1)}
