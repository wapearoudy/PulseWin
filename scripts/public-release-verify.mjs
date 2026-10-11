import { chromium } from '@playwright/test'
import { spawn, execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { createHash } from 'node:crypto'
import { readFile, writeFile, mkdir, mkdtemp } from 'node:fs/promises'
import path from 'node:path'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'

// Anonymous asset acceptance plus real updater downloads; never installs.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const version = JSON.parse(await readFile(path.join(root, 'package.json'), 'utf8')).version
const { repository, source } = JSON.parse(await readFile(path.join(root, 'release-channel.json'), 'utf8'))
const release = `https://github.com/${repository}/releases/tag/v${version}`
const folder = path.join(root, 'updates/github')
const names = ['latest.json', `PulseWin_${version}_x64-setup.exe`, `PulseWin_${version}_x64-setup.exe.sig`, `PulseWin_${version}_x64_en-US.msi`, `PulseWin_${version}_x64_en-US.msi.sig`]
const digest = bytes => createHash('sha256').update(bytes).digest('hex')
const execute = promisify(execFile)
async function anonymousDownload(url) {
  // curl honors the session's HTTPS proxy; Node fetch does not by default.
  const { stdout } = await execute('curl.exe', ['--fail', '--location', '--silent', '--show-error', '--connect-timeout', '15', '--max-time', '60', url], { encoding: 'buffer', maxBuffer: 32 * 1024 * 1024, windowsHide: true })
  return stdout
}
const anonymousAssets = await Promise.all(names.map(async name => {
  const local = await readFile(path.join(folder, name))
  const downloaded = await anonymousDownload(`https://github.com/${repository}/releases/download/v${version}/${name}`)
  assert.equal(digest(downloaded), digest(local), `${name}: published bytes differ`)
  return { name, bytes: downloaded.length, sha256: digest(downloaded) }
}))
assert.equal(JSON.parse((await anonymousDownload(source)).toString('utf8')).version, version)
await mkdir(path.join(root, 'test-results'), { recursive: true })

async function native(executable, port, upgrade) {
  const profile = await mkdtemp(path.join(root, 'test-results/native-profile-public-'))
  const directory = path.join(profile, 'PulseWin')
  await mkdir(directory)
  await writeFile(path.join(directory, 'preferences.json'), JSON.stringify({ enabledProviders: [], tokenSpendEnabled: false }))
  const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith('PULSEWIN_')))
  Object.assign(environment, { APPDATA: profile, LOCALAPPDATA: profile, WEBVIEW2_USER_DATA_FOLDER: path.join(profile, 'webview'), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1` })
  const child = spawn(executable, ['--settings', '--native-smoke'], { cwd: root, env: environment, windowsHide: true, stdio: 'ignore' })
  let browser
  try {
    for (let i = 0; i < 80; i++) {
      if (child.exitCode !== null) throw Error('Verification app exited')
      try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break } catch { await new Promise(resolve => setTimeout(resolve, 250)) }
    }
    assert(browser, 'Native verification endpoint unavailable')
    let page
    for (let i = 0; i < 80; i++) {
      page = browser.contexts().flatMap(context => context.pages()).find(candidate => candidate.url().includes('view=settings'))
      if (page) break
      await new Promise(resolve => setTimeout(resolve, 250))
    }
    assert(page, 'Native Settings page unavailable')
    const call = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args })
    const prefs = JSON.stringify(await call('get_preferences'))
    const status = await call('get_update_status')
    await call('save_update_settings', { settings: { ...status.settings, source, automatic: false } })
    const check = await call('check_app_update')
    assert.equal(check.phase, upgrade ? 'available' : 'current', check.error ?? 'Unexpected update state')
    if (upgrade) {
      assert.equal(check.version, version)
      await call('verify_app_update_smoke', { version })
      const verified = await call('get_update_status')
      assert.equal(verified.downloaded, anonymousAssets.find(asset => asset.name.endsWith('-setup.exe')).bytes)
    } else assert.equal(check.currentVersion, version)
    assert.equal(JSON.stringify(await call('get_preferences')), prefs)
    return upgrade ? { from: status.currentVersion, to: version, passed: true, actualPublishedPackage: true } : { phase: check.phase, source, currentVersion: check.currentVersion }
  } finally { await browser?.close().catch(() => {}); child.kill() }
}

const nativeCheck = await native(path.join(root, 'src-tauri/target/release/pulsewin.exe'), 19476, false)
const signedUpgradeDownload = await native(path.join(root, '.test-data/pulsewin-0.1.4.exe'), 19477, true)
const evidence = { passed: true, testedAt: new Date().toISOString(), repository, release, version, anonymousAssets, nativeCheck, installationExecuted: false, signedUpgradeDownload }
await writeFile(path.join(root, 'test-results/public-release-verification.json'), JSON.stringify(evidence, null, 2))
console.log(`PASS: ${release}; five anonymous assets match SHA-256, latest pointer matches, and ${signedUpgradeDownload.from} verifies the actual signed upgrade. No installer executed.`)
