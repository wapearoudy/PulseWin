import {chromium} from '@playwright/test'
import {spawn,spawnSync} from 'node:child_process'
import {mkdtemp,mkdir,writeFile,readFile} from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'
import {fileURLToPath} from 'node:url'
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
await mkdir(path.join(root,'test-results'),{recursive:true})
const profile=await mkdtemp(path.join(root,'test-results/native-profile-'))
const data=path.join(profile,'PulseWin'),home=path.join(data,'smoke-home'),cache=path.join(data,'smoke-cache/PulseWin/token-spend')
const jsonLines=rows=>rows.map(x=>JSON.stringify(x)).join('\n')
const put=async(relative,body)=>{const file=path.join(home,relative);await mkdir(path.dirname(file),{recursive:true});await writeFile(file,body)}
const now=Date.now(),at=s=>new Date(now+s*1000).toISOString()
await put('.codex/sessions/rollout-fixture.jsonl',jsonLines([
 {type:'session_meta',timestamp:at(-10),payload:{id:'native-fixture',cli_version:'0.146.0',cwd:'synthetic-project'}},
 {type:'event_msg',timestamp:at(-9),payload:{type:'thread_settings_applied',thread_settings:{model:'gpt-5.6',reasoning_effort:'high'}}},
 {type:'event_msg',timestamp:at(-8),payload:{type:'task_started',turn_id:'native-turn',root_turn_id:'native-turn'}},
 {type:'turn_context',timestamp:at(-8),payload:{model:'gpt-5.6',effort:'high'}},
 {type:'response_item',timestamp:at(-8),payload:{type:'message',role:'user',content:[{type:'input_text',text:'synthetic cache session'}]}},
 {type:'response_item',timestamp:at(-4),payload:{type:'message',role:'assistant'}},
 {type:'event_msg',timestamp:at(-3),payload:{type:'token_count',info:{model_context_window:272000,total_token_usage:{input_tokens:100,output_tokens:200,cached_input_tokens:40,total_tokens:300},last_token_usage:{input_tokens:100,output_tokens:200,cached_input_tokens:40,reasoning_output_tokens:516}}}},
 {type:'event_msg',timestamp:at(-2),payload:{type:'task_complete',turn_id:'native-turn',time_to_first_token_ms:1500}}
]))
await put('.claude/projects/synthetic/session.jsonl',jsonLines([
 {type:'user',uuid:'u',timestamp:at(-8),message:{content:'synthetic Claude session'}},
 {type:'assistant',uuid:'a',parentUuid:'u',timestamp:at(-4),message:{id:'native-claude',model:'claude-sonnet-4.5',usage:{input_tokens:100,output_tokens:200,cache_creation_input_tokens:80,cache_read_input_tokens:40,cache_creation:{ephemeral_1h_input_tokens:80}}}}
]))
for(const [dir,id]of[['.pi/agent/sessions','pi'],['.omp/agent/sessions','omp'],['.config/kimchi/harness/sessions','kimchi']])await put(`${dir}/fixture.jsonl`,jsonLines([{type:'session',id:`native-${id}`},{type:'message',id:'reply',timestamp:at(-3),message:{role:'assistant',model:'gpt-5.6',usage:{input:100,output:20,cacheRead:30,cacheWrite:5,totalTokens:155}}}]))
await put('.kiro/sessions/cli/native.json',JSON.stringify({session_id:'native-kiro',session_state:{rts_model_state:{model_info:{model_id:'gpt-5.6'}},conversation_metadata:{user_turn_metadatas:[{input_token_count:100,output_token_count:20,end_timestamp:Math.floor(now/1000)-3}]}}}))
await put('.kimi/sessions/native/wire.jsonl',jsonLines([{timestamp:Math.floor(now/1000)-3,message:{payload:{token_usage:{input_other:100,output:20,input_cache_read:30,input_cache_creation:5}}}}]))
await put('.local/share/amp/threads/T-native.json',JSON.stringify({id:'T-native',created:now-3000,messages:[],usageLedger:{events:[{model:'gpt-5.6',timestamp:at(-3),tokens:{input:100,output:20}}]}}))
await put('.factory/sessions/native.settings.json',JSON.stringify({model:'gpt-5.6',providerLockTimestamp:at(-3),tokenUsage:{inputTokens:100,outputTokens:20,totalTokens:120}}))
await put('.openclaw/agents/native/sessions/fixture.jsonl',jsonLines([{type:'message',id:'native-openclaw',timestamp:at(-3),message:{role:'assistant',model:'gpt-5.6',usage:{input:100,output:20,cacheRead:30,cacheWrite:5}}}]))
const fixtureDatabases=spawnSync('python',['-c',`
import sqlite3,sys,json,time
from pathlib import Path
home=Path(sys.argv[1])
fixtures=[('.local/share/goose/sessions/sessions.db',"CREATE TABLE sessions(id TEXT,model_config_json TEXT,created_at TEXT,input_tokens INTEGER,output_tokens INTEGER)","INSERT INTO sessions VALUES(?,?,?,?,?)",('goose',json.dumps({'model_name':'gpt-5.6'}),time.strftime('%Y-%m-%d %H:%M:%S',time.gmtime()),100,20)),('.hermes/state.db',"CREATE TABLE sessions(id TEXT,model TEXT,started_at INTEGER,input_tokens INTEGER,output_tokens INTEGER)","INSERT INTO sessions VALUES(?,?,?,?,?)",('hermes','gpt-5.6',int(time.time()),100,20)),('.local/share/zed/threads/threads.db',"CREATE TABLE threads(id TEXT,updated_at INTEGER,data_type TEXT,data BLOB)","INSERT INTO threads VALUES(?,?,?,?)",('zed',int(time.time()),'json',json.dumps({'model':{'provider':'zed.dev','model':'gpt-5.6'},'request_token_usage':[{'input_tokens':100,'output_tokens':20,'cache_read_input_tokens':30}]}).encode()))]
for relative,schema,insert,values in fixtures:
 file=home/relative;file.parent.mkdir(parents=True,exist_ok=True)
 with sqlite3.connect(file) as db:db.execute(schema);db.execute(insert,values)
`,home],{windowsHide:true,encoding:'utf8'})
assert.equal(fixtureDatabases.status,0,fixtureDatabases.stderr)
await mkdir(cache,{recursive:true})
const prices=Object.fromEntries(['gpt-5.6','claude-sonnet-4.5'].map(model=>[model,{input:2,output:8,cache_read:.2,cache_write:2.5,name:model,tiers:[]}]))
await writeFile(path.join(cache,'prices-1.json'),JSON.stringify({version:3,fetched_at:Math.floor(now/1000),prices}))
const environment=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('PULSEWIN_')))
Object.assign(environment,{CRUSH_GLOBAL_DATA:path.join(home,'.crush'),HERMES_HOME:path.join(home,'.hermes'),GOOSE_PATH_ROOT:path.join(home,'.goose'),XDG_DATA_HOME:path.join(home,'.local/share'),DSH_HOME:path.join(home,'.dsh'),APPDATA:profile,LOCALAPPDATA:path.join(data,'smoke-local'),CODEX_HOME:path.join(home,'.codex'),CLAUDE_CONFIG_DIR:path.join(home,'.claude'),GEMINI_CLI_HOME:path.join(home,'.gemini'),KIMCHI_CODING_AGENT_DIR:path.join(home,'.config/kimchi/harness'),TOKSCALE_CONFIG_DIR:path.join(home,'.config/tokscale'),HINDSIGHT_HOME:path.join(home,'.hindsight'),WEBVIEW2_USER_DATA_FOLDER:path.join(profile,'WebView2'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19474 --remote-debugging-address=127.0.0.1'})
const child=spawn(path.join(root,'src-tauri/target/release/pulsewin.exe'),['--settings','--native-smoke'],{cwd:root,windowsHide:true,stdio:['ignore','pipe','pipe'],env:environment})
let browser,page,stderr='';child.stderr.on('data',b=>stderr=(stderr+b.toString()).slice(-4000))
try{
 for(let i=0;i<60;i++){if(child.exitCode!==null)throw Error(`Native startup failed ${stderr}`);try{browser=await chromium.connectOverCDP('http://127.0.0.1:19474',{timeout:1000});break}catch{await new Promise(r=>setTimeout(r,250))}}
 assert(browser,'Native WebView2 endpoint unavailable')
 for(let i=0;i<60;i++){page=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('view=settings'));if(page)break;await new Promise(r=>setTimeout(r,250))}assert(page)
 await page.getByRole('button',{name:'暂不设置'}).click()
 const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args})
 const nav=page.getByRole('complementary',{name:'设置导航'})
 for(const name of ['Claude Code','Codex','Claude Code','Grok','DeepSeek','Grok']){
  await nav.getByRole('button',{name,exact:true}).click()
  assert.equal(await page.getByLabel('账号显示名称',{exact:true}).count(),1,`Native ${name}: exactly one account identity`)
  assert.equal(await page.getByLabel('账号显示名称',{exact:true}).inputValue(),name)
 }
 await page.screenshot({path:path.join(root,'test-results/native-account-switch.png')})
 assert.deepEqual((await invoke('get_preferences')).enabledProviders,[])
 assert.equal((await invoke('get_opencode_console_connection')).connected,false);await assert.rejects(invoke('get_opencode_console_history'),/启用 OpenCode Go/);await assert.rejects(invoke('finish_opencode_console_login'),/先打开控制台/);
 assert.equal((await invoke('get_deepseek_console_connection')).connected,false);await assert.rejects(invoke('get_deepseek_console_history'),/启用 DeepSeek/);await assert.rejects(invoke('finish_deepseek_console_login'),/先打开控制台/)
 await assert.rejects(invoke('get_codex_signals',{span:'month'}),/本机 Token/)
 await assert.rejects(invoke('get_prompt_cache',{provider:'codex'}),/未开启/)
 await page.getByRole('button',{name:'Token 消耗',exact:true}).click();await page.getByRole('switch',{name:'读取本机 Token 用量记录'}).click();await page.waitForFunction(()=>document.querySelector('[aria-label="读取本机 Token 用量记录"]')?.checked)
 await page.getByRole('button',{name:'月度 / 年度回顾'}).waitFor({timeout:15000})
 assert.equal(await page.locator('.spend-sources').getByText('尚未移植',{exact:false}).count(),0)
 assert.equal(await page.locator('.spend-sources').getByText('Grok Build',{exact:false}).count(),0)
 const scan=await page.evaluate(async()=>{const id=await window.__TAURI_INTERNALS__.invoke('spend_begin_scan',{enabled:true,force:false});for(let i=0;i<100;i++){const r=await window.__TAURI_INTERNALS__.invoke('spend_get_scan',{scanId:id});if(r.status==='completed')return r;if(r.status==='error')throw Error(r.error);await new Promise(r=>setTimeout(r,100))}throw Error('scan timeout')})
 assert.equal(scan.snapshot.sources.length,41);assert(['roocode','kilocode','cline','cherrystudio','commandcode','opencodereview','zcode','unsloth','mux','codebuff','jcode','augment','gjc','junie','fx','lmstudio','reasonix'].every(id=>scan.snapshot.sources.some(source=>source.id===id)));assert.equal(scan.snapshot.records.length,13);assert(scan.snapshot.records.every(r=>r.sourceTimestamp>1e9&&r.sourceTimestamp<1e10));assert.deepEqual(scan.snapshot.notes,[]);assert.equal((await invoke('spend_kept_snapshot')).records.length,13)
 const codex=scan.snapshot.records.find(r=>r.agent==='codex');assert.equal(codex.replyTiming.seconds,4);assert.equal(codex.replyTiming.firstTokenSeconds,1.5)
 const signals=await invoke('get_codex_signals',{span:'month'});assert.equal(signals.sessions,1);assert.equal(signals.judgedSessions,1);assert.equal(signals.models[0].hits,1);assert.equal(signals.models[0].suspicious,false)
 const codexCache=await invoke('get_prompt_cache',{provider:'codex'}),claudeCache=await invoke('get_prompt_cache',{provider:'claude-code'});assert.equal(codexCache.live.length,1);assert.equal(codexCache.live[0].lifetime,1800);assert.equal(claudeCache.live[0].lifetime,3600)
 await page.getByRole('complementary',{name:'设置导航'}).getByRole('button',{name:'Codex',exact:true}).click();await page.getByRole('region',{name:'Codex 本地分析'}).getByText('样本不足',{exact:true}).waitFor();await page.screenshot({path:path.join(root,'test-results/native-local-evidence.png')})
 await page.getByRole('button',{name:'Token 消耗',exact:true}).click();await page.getByRole('button',{name:'月度 / 年度回顾'}).click();const recap=page.getByRole('region',{name:'月度与年度回顾'});await recap.getByRole('img').waitFor()
 const png=await recap.getByRole('img').getAttribute('src');const exported=await invoke('recap_export',{cards:[{name:'pulse-recap-native-poster.png',png:png.split(',')[1]}],action:'smoke'});assert.equal(exported,'saved')
 const bytes=await readFile(path.join(data,'pulse-recap-native-poster.png'));assert.equal(bytes.readUInt32BE(16),1080);assert.equal(bytes.readUInt32BE(20),1920);await writeFile(path.join(root,'test-results/native-recap-poster.png'),bytes)
 const prefs=await invoke('get_preferences');await invoke('save_preferences',{value:{...prefs,recapMonthlyPrice:20,recapHidesProjects:true,spendActivityView:'weekly'}})
 const persisted=JSON.parse(await readFile(path.join(data,'preferences.json'),'utf8'));assert.equal(persisted.recapMonthlyPrice,20);assert.equal(persisted.recapHidesProjects,true);assert.equal(persisted.spendActivityView,'weekly')
 const update=await invoke('get_update_status');await invoke('save_update_settings',{settings:{...update.settings,automatic:false,backupAutomatic:false,notesLanguage:'ja'}});const saved=JSON.parse(await readFile(path.join(data,'updates.json'),'utf8'));assert.equal(saved.backupAutomatic,false);assert.equal(saved.notesLanguage,'ja')
 await invoke('save_preferences',{value:{...prefs,tokenSpendEnabled:false}});assert.equal(await invoke('spend_kept_snapshot'),null);await assert.rejects(invoke('get_codex_signals',{span:'month'}),/本机 Token/);await assert.rejects(invoke('get_prompt_cache',{provider:'codex'}),/未开启/)
 const result={passed:true,profile,testedAt:new Date().toISOString(),sources:scan.snapshot.sources.map(s=>({id:s.id,records:s.records})),checks:['account switches remove prior identities and drafts','isolated fixture-only home and cache','opt-in and opt-out native guards','forty-one registered token sources with thirteen fixture-backed reader records including Kiro/Kimi and four SQLite/legacy stores','counter timestamps in seconds','Codex measured speed and TTFT','Codex evidence and insufficient-sample threshold','Claude and Codex cache TTL','full-size real canvas PNG export through native decoder and atomic writer','recap and activity preference persistence','backup and release-note language persistence'],notRun:['real account billing','clipboard and interactive file dialogs','official status outage delivery','full original recap visual parity','online backup assets blocked by public API rate limit']}
 await writeFile(path.join(root,'test-results/native-features-smoke.json'),JSON.stringify(result,null,2));console.log('PASS: native feature IPC, fixture counters, opt-out, PNG export and settings persistence')
}catch(e){await page?.screenshot({path:path.join(root,'test-results/native-features-failure.png')}).catch(()=>undefined);throw e}finally{await browser?.close().catch(()=>undefined);child.kill()}
