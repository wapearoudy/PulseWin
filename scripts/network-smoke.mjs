import { chromium } from '@playwright/test'
import { spawn } from 'node:child_process'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import path from 'node:path'
import assert from 'node:assert/strict'
import { fileURLToPath } from 'node:url'

// Isolated profile and a synthetic DeepSeek key. The local fake proxies never
// tunnel to an upstream server. A pending CONNECT proves actual cancellation.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
await mkdir(path.join(root, 'test-results'), { recursive: true })
const profile = await mkdtemp(path.join(root, 'test-results/native-profile-network-'))
const dir = path.join(profile, 'PulseWin'); await mkdir(dir, { recursive: true })
await writeFile(path.join(dir, 'preferences.json'), JSON.stringify({ enabledProviders: [], panelVisible: false }))
const personal = path.join(process.env.APPDATA, 'PulseWin/preferences.json')
const readPersonal = () => readFile(personal).catch(e => { if (e.code === 'ENOENT') return null; throw e })
const personalBefore = await readPersonal(), sockets = new Set(), requests = [[], []]
let firstSeen, firstClosed, secondSeen
const connected = new Promise(r => { firstSeen = r }), cancelled = new Promise(r => { firstClosed = r }), replaced = new Promise(r => { secondSeen = r })
const servers = [0, 1].map(index => {
  const server = createServer((_, res) => { res.writeHead(502); res.end() })
  server.on('connect', (req, socket) => {
    // CONNECT sockets are handed to us; drain them to observe the client's FIN.
    socket.resume(); socket.on('end', () => socket.end())
    sockets.add(socket); socket.on('close', () => { sockets.delete(socket); if (!index) firstClosed() })
    requests[index].push(req.url)
    if (req.url !== 'api.deepseek.com:443') { socket.destroy(); return }
    if (!index) firstSeen()
    else { secondSeen(); socket.end('HTTP/1.1 502 Synthetic proxy response\r\nContent-Length: 0\r\nConnection: close\r\n\r\n') }
  })
  return server
})
for (const server of servers) await new Promise(r => server.listen(0, '127.0.0.1', r))
const ports = servers.map(server => server.address().port), debugPort = 19478, version = JSON.parse(await readFile(path.join(root, 'package.json'), 'utf8')).version
let child, browser, page, diagnostics = ''
const delay = ms => new Promise(r => setTimeout(r, ms))
const bounded = (promise, label) => Promise.race([promise, new Promise((_, reject) => { const timer = setTimeout(() => reject(Error(`Timed out: ${label}`)), 15000); timer.unref() })])
async function start() {
  const env = { ...process.env }
  for (const key of Object.keys(env)) if (/^PULSEWIN_/i.test(key)) delete env[key]
  child = spawn(path.join(root, 'src-tauri/target/release/pulsewin.exe'), ['--settings', '--native-smoke'], { cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: { ...env, APPDATA: profile, PULSEWIN_DEEPSEEK_KEY: 'synthetic-network-only', WEBVIEW2_USER_DATA_FOLDER: path.join(profile, 'webview'), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort} --remote-debugging-address=127.0.0.1` } })
  child.stderr.on('data', b => { diagnostics = (diagnostics + b.toString()).slice(-8000) })
  for (let i = 0; i < 50; i++) { if (child.exitCode !== null) throw Error(`App exited ${child.exitCode}: ${diagnostics}`); try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`, { timeout: 500 }); break } catch { await delay(200) } }
  assert(browser)
  for (let i = 0; i < 50; i++) { page = browser.contexts().flatMap(c => c.pages()).find(p => p.url().includes('view=settings')); if (page) break; await delay(200) }
  assert(page); await page.waitForFunction(() => !!window.__TAURI_INTERNALS__)
}
const call = (cmd, args = {}) => bounded(page.evaluate(({ cmd, args }) => window.__TAURI_INTERNALS__.invoke(cmd, args), { cmd, args }), cmd)
async function stop() {
  await browser?.close().catch(() => {}); browser = null
  if (child && child.exitCode === null) { const exited = new Promise(r => child.once('exit', r)); child.kill(); await Promise.race([exited, delay(1500)]) }
}
try {
  await start()
  await page.getByRole('button', { name: '网络与刷新', exact: true }).click()
  await page.getByLabel('代理方式').selectOption('manual')
  await page.getByLabel('代理主机').fill('127.0.0.1'); await page.getByLabel('代理端口').fill(String(ports[0])); await page.getByLabel('代理端口').press('Enter')
  await page.waitForFunction(port => !document.querySelector('[aria-label="代理主机"]').disabled && document.querySelector('[aria-label="代理端口"]').value === String(port), ports[0])
  assert.equal((await call('get_network_status')).settings.port, ports[0])
  assert.equal((await call('get_snapshot')).providers.length, 0); assert.deepEqual(requests, [[], []])
  const initial = await call('get_preferences')
  await call('save_preferences', { value: { ...initial, enabledProviders: ['deepseek'] } })
  await bounded(connected, 'first local proxy CONNECT')
  const switchedAt = Date.now()
  await call('save_network_settings', { value: { mode: 'manual', kind: 'http', host: '127.0.0.1', port: ports[1] } })
  await bounded(cancelled, 'superseded CONNECT cancellation'); await bounded(replaced, 'new proxy retry')
  const cancellationMs = Date.now() - switchedAt; assert(cancellationMs < 5000, 'Do not wait for the old 20-second request timeout')
  const reading = await bounded((async () => { for (;;) { const value = (await call('get_snapshot')).providers.find(p => p.id === 'deepseek' && p.error); if (value) return value; await delay(50) } })(), 'replacement error snapshot')
  assert.equal(reading.windows.length, 0, 'A proxy error must not turn into a zero-percent reading')
  const current = await call('get_preferences')
  // A stale generic preference save must not restore the old proxy.
  await call('save_preferences', { value: { ...current, networkProxy: initial.networkProxy, enabledProviders: [] } })
  assert.equal((await call('get_network_status')).settings.port, ports[1])
  for (const value of [ { mode: 'manual', kind: 'http', host: 'http://bad/path', port: 80 }, { mode: 'manual', kind: 'http', host: '127.0.0.1', port: 65536 }, { mode: 'manual', kind: 'http', host: '', port: null } ]) {
    await assert.rejects(call('save_network_settings', { value })); assert.equal((await call('get_network_status')).settings.port, ports[1])
  }
  await page.getByRole('button', { name: '网络与刷新', exact: true }).click()
  await page.getByLabel('代理类型').selectOption('socks5'); await page.getByLabel('检查间隔').selectOption('600')
  await page.waitForFunction(async () => (await window.__TAURI_INTERNALS__.invoke('get_preferences')).refreshSeconds === 600)
  await page.screenshot({ path: path.join(root, 'test-results/native-network.png') })
  await page.getByRole('button', { name: '关于', exact: true }).click(); await page.getByText(`版本 ${version}`, { exact: true }).waitFor()
  assert.equal((await call('get_app_info')).licenses.length, 5)
  await page.screenshot({ path: path.join(root, 'test-results/native-about.png') })
  await assert.rejects(call('open_about_link', { key: 'https://invalid.example' }))
  const saved = JSON.parse(await readFile(path.join(dir, 'preferences.json'), 'utf8'))
  assert.equal(saved.networkProxy.kind, 'socks5'); assert.equal(saved.networkProxy.port, ports[1]); assert.deepEqual(saved.enabledProviders, [])
  await stop(); await start()
  await page.getByRole('button', { name: '网络与刷新', exact: true }).click(); await page.getByLabel('代理类型').waitFor()
  assert.equal(await page.getByLabel('代理类型').inputValue(), 'socks5'); assert.equal(await page.getByLabel('代理端口').inputValue(), String(ports[1])); assert.equal(await page.getByLabel('检查间隔').inputValue(), '600')
  assert.equal(await call('plugin:window|is_visible', { label: 'main' }), false)
  assert.deepEqual(await readPersonal(), personalBefore, 'The installed profile must remain unchanged')
  await writeFile(path.join(root, 'test-results/network-smoke.json'), JSON.stringify({ passed: true, version, testedAt: new Date().toISOString(), profile, cancellationMs, requests, checks: ['real native UI and IPC', 'atomic manual endpoint save', 'no provider collection on opening', 'pending CONNECT cancelled', 'immediate retry through new proxy', 'errors do not become zero quota', 'stale generic saves preserve network', 'invalid endpoints preserve valid proxy', 'SOCKS5 and cadence restart persistence', 'native metadata and bundled licenses', 'unknown link rejected', 'installed profile byte equality'] }, null, 2))
  console.log(`PASS: native network settings, proxy replacement/cancellation (${cancellationMs}ms), restart persistence and About metadata.`)
} catch (e) {
  await page?.screenshot({ path: path.join(root, 'test-results/network-native-failure.png') }).catch(() => {})
  throw e
} finally {
  await stop(); for (const socket of sockets) socket.destroy(); for (const server of servers) await new Promise(r => server.close(r))
}
