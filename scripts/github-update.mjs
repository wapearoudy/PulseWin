import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

// Prepares reviewable GitHub Release assets. Does not upload or create a repo.
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
const repo=process.argv[2]
if(!repo || !/^[\w.-]+\/[\w.-]+$/.test(repo))throw new Error('Usage: npm run prepare:github -- owner/repo')
const local=JSON.parse(readFileSync(path.join(root,'updates/latest.json'),'utf8'))
const folder=path.join(root,'updates/github');mkdirSync(folder,{recursive:true})
const release=`https://github.com/${repo}/releases/download/v${local.version}`
const msi=`PulseWin_${local.version}_x64_en-US.msi`
copyFileSync(path.join(root,'updates',local.file),path.join(folder,local.file))
copyFileSync(path.join(root,'updates',`${local.file}.sig`),path.join(folder,`${local.file}.sig`))
const msiPath=path.join(root,'src-tauri/target/release/bundle/msi',msi)
const msiSignature=readFileSync(`${msiPath}.sig`,'utf8').trim()
copyFileSync(msiPath,path.join(folder,msi));copyFileSync(`${msiPath}.sig`,path.join(folder,`${msi}.sig`))
const nsis={url:`${release}/${local.file}`,signature:local.signature}
const manifest={version:local.version,notes:local.notes,pub_date:local.pub_date,platforms:{
  'windows-x86_64':nsis,'windows-x86_64-nsis':nsis,
  'windows-x86_64-msi':{url:`${release}/${msi}`,signature:msiSignature}
}}
writeFileSync(path.join(folder,'latest.json'),JSON.stringify(manifest,null,2))
console.log(`GitHub Release v${local.version} assets ready in ${folder}`)
console.log(`Application update source: https://github.com/${repo}/releases/latest/download/latest.json`)
