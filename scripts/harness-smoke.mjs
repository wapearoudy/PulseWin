import { chromium } from '@playwright/test'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { zstdCompressSync } from 'node:zlib'
import assert from 'node:assert/strict'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

// Real release WebView2 / IPC, synthetic native Harness sessions only.
// The independent real-user read-only verification is opt-in in Rust tests.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
await mkdir(path.join(root, 'test-results'), { recursive: true })
const profile = await mkdtemp(path.join(root, 'test-results', 'native-profile-harness-'))
const sessions = path.join(profile, 'harness/sessions/synthetic')
await mkdir(sessions, { recursive: true })
await mkdir(path.join(profile, 'PulseWin'), { recursive: true })
await writeFile(path.join(profile, 'PulseWin/preferences.json'), JSON.stringify({ enabledProviders: [], onboardingCompleted: true, tokenSpendEnabled: false }))
const at = Date.now(), header = version => ({ type: 'session', version, id: 'synthetic-harness', cwd: 'E:/synthetic-harness', isSeeded: false })
const message = seq => ({ type: 'assistant/message', seq, time: at + seq, data: { turn: 1, step: seq,
  message: { id: `synthetic-${seq}`, source: { provider: 'deepseek', model: 'deepseek-chat' }, content: 'PRIVATE_SYNTHETIC_SENTINEL' },
  usage: { inputTokens: 100, outputTokens: 40, cacheReadTokens: 200, cacheWriteTokens: 10, reasoningTokens: 30, totalTokens: 350 } } })
const jsonl = events => events.map(e => JSON.stringify(e) + '\n').join('')
const packed = events => Buffer.concat(events.map(e => zstdCompressSync(Buffer.from(jsonl([e])))))
const plain = path.join(sessions, 'session.jsonl'), compressed = path.join(sessions, 'session.v4.jsonl.zstd')
const plainBytes = Buffer.from(jsonl([header(0), message(1)]))
const compressedBytes = packed([header(4), message(1), message(2)])
await writeFile(plain, plainBytes); await writeFile(compressed, compressedBytes)
const port = Number(process.env.PULSEWIN_HARNESS_CDP_PORT || 19480)
const child = spawn(path.join(root, 'src-tauri/target/release/pulsewin.exe'), ['--settings', '--native-smoke'], {
  cwd: root, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env,
    APPDATA: profile, LOCALAPPDATA: profile, DSH_HOME: path.join(profile, 'harness'), CLAUDE_CONFIG_DIR: path.join(profile, 'empty/claude'),
    CODEX_HOME: path.join(profile, 'empty/codex'), GEMINI_CLI_HOME: path.join(profile, 'empty/gemini'), TOKSCALE_CONFIG_DIR: path.join(profile, 'empty/captures'),
    HINDSIGHT_HOME: path.join(profile, 'empty/hindsight'), XDG_DATA_HOME: path.join(profile, 'empty/databases'),
    WEBVIEW2_USER_DATA_FOLDER: path.join(profile, 'webview'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
  },
})
let browser, page, diagnostic = ''
child.stderr.on('data', chunk => { diagnostic = (diagnostic + chunk).slice(-4000) })
try {
  for (let attempt = 0; attempt < 40; attempt++) {
    if (child.exitCode !== null) throw new Error(`Native app exited: ${child.exitCode}; ${diagnostic}`)
    try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { timeout: 1000 }); break }
    catch { await new Promise(resolve => setTimeout(resolve, 250)) }
  }
  assert(browser)
  for (let attempt = 0; attempt < 40; attempt++) {
    page = browser.contexts().flatMap(c => c.pages()).find(p => p.url().includes('view=settings'))
    if (page) break
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  assert(page)
  const call = (command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args })
  const prefs = await call('get_preferences')
  assert.equal(prefs.tokenSpendEnabled, false); assert.deepEqual(prefs.enabledProviders, [])
  await assert.rejects(call('spend_begin_scan', { enabled: true, force: false }), /disabled/)
  // Public prices may use their existing cache. Offline fixture acceptance must
  // not depend on an external request or write synthetic model prices there.
  await call('save_network_settings', { value: { mode: 'manual', kind: 'http', host: '127.0.0.1', port: 19880 } })
  await page.getByRole('button', { name: 'Token 消耗', exact: true }).click()
  await page.getByRole('switch', { name: '读取本机 Token 用量记录' }).click()
  await page.waitForFunction(() => document.querySelector('[aria-label="读取本机 Token 用量记录"]')?.checked)
  await expectCompleted()
  await page.getByRole('button', { name: 'DeepSeek Harness', exact: false }).click()
  await page.getByRole('heading', { name: 'DeepSeek Harness', exact: true }).waitFor()
  assert((await page.locator('.spend-headline').innerText()).includes('700'))
  await page.locator('.token-spend-pane').evaluate(element => element.scrollIntoView({ block: 'start' }))
  await page.screenshot({ path: path.join(root, 'test-results/native-harness.png') })
  const first = await newestScan()
  assert.equal(first.sourceCount, 12); assert.equal(first.snapshot.notes.length, 0)
  assert.equal(first.snapshot.records.length, 2, 'Only isolated Harness fixtures must be counted')
  assert(first.snapshot.records.every(r => r.agent === 'dsh' && r.tally.output === 40 && r.tally.cacheRead === 200))
  assert.equal(first.snapshot.sources.find(s => s.id === 'dsh').files, 2)
  assert(!JSON.stringify(first.snapshot).includes('PRIVATE_SYNTHETIC_SENTINEL'))
  await page.getByRole('button', { name: '重新扫描', exact: true }).click(); await expectCompleted()
  const second = await newestScan(); assert.equal(second.snapshot.sources.find(s => s.id === 'dsh').cachedFiles, 2)
  const appended = Buffer.concat([compressedBytes, packed([message(3)])]); await writeFile(compressed, appended)
  const third = await newestScan(); assert.equal(third.snapshot.records.length, 3)
  assert.equal(third.snapshot.sources.find(s => s.id === 'dsh').cachedFiles, 1)
  await page.getByRole('button', { name: '重新扫描', exact: true }).click(); await expectCompleted()
  assert((await page.locator('.spend-headline').innerText()).includes('1,050'))
  assert.deepEqual(await readFile(plain), plainBytes); assert.deepEqual(await readFile(compressed), appended)
  await page.getByRole('switch', { name: '读取本机 Token 用量记录' }).click()
  await page.waitForFunction(() => !document.querySelector('[aria-label="读取本机 Token 用量记录"]')?.checked)
  await assert.rejects(call('spend_begin_scan', { enabled: true, force: true }), /disabled/)
  const version = (await call('get_app_info')).version
  await writeFile(path.join(root, 'test-results/harness-smoke.json'), JSON.stringify({ passed: true, version, testedAt: new Date().toISOString(),
    checks: ['real native UI and IPC', 'default-off guard', 'concatenated zstd checkpoints', 'plain and migrated copy deduplication',
      'inclusive reasoning and disjoint cache', 'cached reread', 'appended frame invalidation', 'read-only inputs', 'no transcript content retained', 'opt-out guard'] }, null, 2))
  console.log('PASS: native Harness statistics, compressed frames, deduplication, cache and appended usage.')
  async function newestScan() {
    const id = await call('spend_begin_scan', { enabled: true, force: false })
    for (let attempt = 0; attempt < 200; attempt++) {
      const scan = await call('spend_get_scan', { scanId: id })
      if (scan.status === 'completed') return scan
      assert.equal(scan.status, 'running', scan.error || 'Scan failed')
      await new Promise(resolve => setTimeout(resolve, 200))
    }
    throw new Error('Native Harness scan timed out')
  }
  async function expectCompleted() {
    // UI acceptance uses its own genuine scan, without IPC mocking.
    await page.waitForFunction(() => !document.querySelector('.spend-loading') && !!document.querySelector('.spend-headline'), null, { timeout: 40000 })
  }
} finally {
  await browser?.close().catch(() => undefined)
  child.kill()
}
