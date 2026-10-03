import { spawnSync } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import assert from 'node:assert/strict'

// Exercise the release dispatch, never the personal app profile or a provider probe.
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
await mkdir(path.join(root,'test-results'),{recursive:true})
const profile=await mkdtemp(path.join(root,'test-results','cli-profile-'))
const directory=path.join(profile,'PulseWin')
await mkdir(directory)
const observed=new Date(Date.now()-3600000).toISOString()
const window=(id,percentUsed)=>({id,label:id,kind:'limit',percentUsed,windowSeconds:604800,resetsAt:null,detail:null,isExhausted:false})
const reading=(id,windows)=>({id,name:id,configured:true,windows,fetchedAt:observed,error:null,plan:null,account:null})
const documents={
  'preferences.json':{enabledProviders:['codex'],providerOrder:['claude-code','codex'],pinnedWindows:{codex:'pool-low'}},
  'usage-cache.json':{version:1,readings:{codex:reading('codex',[window('pool-high',99.6),window('pool-low',.25)]),'claude-code':reading('claude-code',[window('disabled',70)])}},
  'claude-code.json':{apiKey:'non-secret-fixture-do-not-migrate'},
}
for(const [name,value] of Object.entries(documents))await writeFile(path.join(directory,name),JSON.stringify(value))
const before=await Promise.all(Object.keys(documents).map(name=>readFile(path.join(directory,name),'utf8')))
const executable=path.join(root,'src-tauri/target/release/pulsewin.exe')
const run=argument=>{
  const result=spawnSync(executable,[argument],{cwd:root,windowsHide:true,encoding:'utf8',timeout:5000,env:{...process.env,APPDATA:profile}})
  assert.equal(result.error,undefined);assert.equal(result.status,0,result.stderr)
  return result.stdout.trim()
}
const report=JSON.parse(run('--json'))
assert.equal(report.accounts.length,1)
assert.equal(report.accounts[0].id,'codex')
assert.equal(report.accounts[0].headline.id,'pool-low')
assert.equal(report.accounts[0].headline.usedPercent,1,'A small nonzero reading must agree with the panel')
assert.equal(report.accounts[0].windows[0].usedPercent,99,'Rounding is not exhaustion')
assert.equal(report.accounts[0].observedAt,observed)
assert(report.accounts[0].ageSeconds>=3600)
assert.equal(run('--statusline'),'Codex 1%')
assert.deepEqual(await Promise.all(Object.keys(documents).map(name=>readFile(path.join(directory,name),'utf8'))),before,'Read-only dispatch must not mutate cache, preferences or plaintext fixture credentials')
await writeFile(path.join(root,'test-results/cli-smoke.json'),JSON.stringify({passed:true,testedAt:new Date().toISOString(),profile,checks:['release dispatch before GUI','enabled accounts only','actual window id pin','panel display rounding','dated observations','read-only files','no credential migration']},null,2))
console.log('PASS: release CLI reads isolated dated cache, respects account/pin selection and leaves every fixture file unchanged.')
