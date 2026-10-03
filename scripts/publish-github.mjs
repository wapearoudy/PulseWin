import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
const {repository}=JSON.parse(readFileSync(path.join(root,'release-channel.json'),'utf8'))
if(!/^[\w.-]+\/[\w.-]+$/.test(repository))throw new Error('Invalid release repository')
const git=(args)=>{const result=spawnSync('git',args,{cwd:root,encoding:'utf8',env:{...process.env,GIT_TERMINAL_PROMPT:'0',GCM_INTERACTIVE:'never'}});if(result.status!==0)throw new Error('Git command failed: '+result.stderr);return result.stdout.trim()}
if(git(['status','--porcelain']))throw new Error('Commit source changes before publishing a release')
const version=JSON.parse(readFileSync(path.join(root,'package.json'),'utf8')).version
const tag=`v${version}`
if(!process.env.GH_TOKEN&&!process.env.GITHUB_TOKEN){
  const result=spawnSync('git',['-c','credential.interactive=false','credential','fill'],{cwd:root,input:`protocol=https\nhost=github.com\nusername=${repository.split('/')[0]}\n\n`,encoding:'utf8',env:{...process.env,GIT_TERMINAL_PROMPT:'0',GCM_INTERACTIVE:'never'}})
  if(result.status!==0)throw new Error('GitHub login required; complete it with Git Credential Manager')
  const token=result.stdout.split(/\r?\n/).find(line=>line.startsWith('password='))?.slice(9)
  if(!token)throw new Error('GitHub credential unavailable')
  process.env.GH_TOKEN=token
}
const token=process.env.GH_TOKEN||process.env.GITHUB_TOKEN
async function api(route,options={}){
  const response=await fetch(route.startsWith('https://')?route:`https://api.github.com${route}`,{...options,headers:{Authorization:`Bearer ${token}`,Accept:'application/vnd.github+json','X-GitHub-Api-Version':'2022-11-28','User-Agent':'PulseWin-Release',...options.headers}})
  if(!response.ok)throw new Error(`GitHub request failed (${response.status}): ${route.split('?')[0]}`)
  return response.status===204?null:response.json()
}
const user=await api('/user');if(user.login.toLowerCase()!==repository.split('/')[0].toLowerCase())throw new Error('Release repository does not belong to the authenticated account')
const prep=spawnSync(process.execPath,[path.join(root,'scripts/github-update.mjs'),repository],{cwd:root,stdio:'inherit'});if(prep.status!==0)throw new Error('Release preparation failed')
git(['push','origin','main'])
const sha=git(['rev-parse','HEAD'])
let release
try{release=await api(`/repos/${repository}/releases/tags/${tag}`)}catch(error){if(!String(error).includes('(404)'))throw error}
if(!release)release=await api(`/repos/${repository}/releases`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({tag_name:tag,target_commitish:sha,name:`PulseWin ${version}`,body:readFileSync(path.join(root,'release-notes.txt'),'utf8'),draft:true,prerelease:false})})
if(!release.draft)throw new Error('This version is already published; bump the version before releasing new bytes')
const folder=path.join(root,'updates/github')
for(const file of [`PulseWin_${version}_x64-setup.exe`,`PulseWin_${version}_x64-setup.exe.sig`,`PulseWin_${version}_x64_en-US.msi`,`PulseWin_${version}_x64_en-US.msi.sig`,'latest.json']){
  if(release.assets.some(asset=>asset.name===file))throw new Error(`Draft already has ${file}; review it before retrying`)
  const bytes=readFileSync(path.join(folder,file))
  await api(`${release.upload_url.split('{')[0]}?name=${encodeURIComponent(file)}`,{method:'POST',headers:{'Content-Type':'application/octet-stream'},body:bytes})
  console.log(`Uploaded ${file}`)
}
release=await api(`/repos/${repository}/releases/${release.id}`,{method:'PATCH',headers:{'Content-Type':'application/json'},body:JSON.stringify({draft:false,make_latest:'true'})})
console.log(`Published ${release.html_url}`)
