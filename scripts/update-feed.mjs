import { copyFileSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

export function publishLocalUpdates(root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')) {
  const config = JSON.parse(readFileSync(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'))
  const { version } = config
  if (!/^\d+\.\d+\.\d+(?:-[\da-z.-]+)?$/i.test(version)) throw new Error('Invalid update version')
  const file = `PulseWin_${version}_x64-setup.exe`
  const installer = path.join(root, 'src-tauri/target/release/bundle/nsis', file)
  if (!existsSync(`${installer}.sig`)) throw new Error('Signed NSIS update artifact is missing')
  const signature = readFileSync(`${installer}.sig`, 'utf8').trim()
  if (!signature) throw new Error('Update signature is empty')
  const folder = path.join(root, 'updates')
  mkdirSync(folder, { recursive: true })
  // Publish the immutable version file before replacing the channel pointer.
  const staged = path.join(folder, `${file}.${process.pid}.tmp`)
  copyFileSync(installer, staged)
  renameSync(staged, path.join(folder, file))
  copyFileSync(`${installer}.sig`, path.join(folder, `${file}.sig`))
  const notes = existsSync(path.join(root, 'release-notes.txt'))
    ? readFileSync(path.join(root, 'release-notes.txt'), 'utf8').trim() : `PulseWin ${version}`
  const manifest = { version, notes, pub_date: new Date().toISOString(), file, signature }
  const temporary = path.join(folder, `latest.${process.pid}.tmp`)
  writeFileSync(temporary, JSON.stringify(manifest, null, 2))
  renameSync(temporary, path.join(folder, 'latest.json'))
  console.log(`Published signed local update ${version} to ${folder}`)
}
