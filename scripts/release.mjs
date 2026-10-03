import { readFileSync, writeFileSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

// One command for subsequent builds: bump the patch version, build/sign/publish.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const packageFile = path.join(root, 'package.json')
const pkg = JSON.parse(readFileSync(packageFile, 'utf8'))
if (!/^\d+\.\d+\.\d+$/.test(pkg.version)) throw new Error('Automatic release requires a stable x.y.z version')
const parts = pkg.version.split('.').map(Number); parts[2]++
const version = parts.join('.')
const configFile = path.join(root, 'src-tauri/tauri.conf.json')
const config = JSON.parse(readFileSync(configFile, 'utf8')); config.version = version
const lockFile = path.join(root, 'package-lock.json')
const lock = JSON.parse(readFileSync(lockFile, 'utf8')); lock.version = version; lock.packages[''].version = version
pkg.version = version
const cargoFile = path.join(root, 'src-tauri/Cargo.toml')
const cargo = readFileSync(cargoFile, 'utf8').replace(/^(version\s*=\s*")\d+\.\d+\.\d+(")/m, `$1${version}$2`)
writeFileSync(packageFile, JSON.stringify(pkg, null, 2)+'\n')
writeFileSync(lockFile, JSON.stringify(lock, null, 2)+'\n')
writeFileSync(configFile, JSON.stringify(config, null, 2)+'\n')
writeFileSync(cargoFile, cargo)
const result = spawnSync(process.execPath, [path.join(root, 'scripts/tauri.mjs'), 'build'], { cwd:root,stdio:'inherit' })
if (result.error) throw result.error
process.exitCode = result.status ?? 1
