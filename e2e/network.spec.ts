import { test, expect, type Page } from '@playwright/test'

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any, callbacks: Record<number, Function> = {}, listeners: Record<number, any> = {}; let sequence = 0
    const prefs = { enabledProviders: [], providerOrder: ['codex'], networkProxy: { mode: 'system', kind: 'http', host: '', port: null }, refreshAutomatic: true, refreshSeconds: 120 }
    w.networkPrefs = prefs; w.networkCommands = []; w.networkFail = false; w.networkReadFail = false; w.aboutOpenFail = false
    const emit = (event: string, payload: unknown) => Object.values(listeners).filter(v => v.event === event).forEach(v => callbacks[v.handler]?.({ event, payload }))
    w.networkEmit = emit
    const status = () => ({ settings: structuredClone(prefs.networkProxy), source: prefs.networkProxy.mode === 'manual' && prefs.networkProxy.port ? '手动代理' : '系统未设置代理', endpoint: prefs.networkProxy.mode === 'manual' && prefs.networkProxy.port ? `${prefs.networkProxy.kind}://127.0.0.1:${prefs.networkProxy.port}` : null, manualReady: prefs.networkProxy.mode === 'manual' && !!prefs.networkProxy.port })
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_: string, id: number) => delete listeners[id] }
    w.__TAURI_INTERNALS__ = {
      transformCallback: (fn: Function) => { callbacks[++sequence] = fn; return sequence },
      invoke: async (cmd: string, args: any = {}) => {
        w.networkCommands.push({ cmd, args })
        if (cmd === 'plugin:event|listen') { listeners[++sequence] = args; return sequence }
        if (cmd === 'plugin:event|unlisten') return
        if (cmd === 'get_preferences') return structuredClone(prefs)
        if (cmd === 'save_preferences') { Object.assign(prefs, args.value); emit('preferences-changed', structuredClone(prefs)); return }
        if (cmd === 'provider_settings') return [{ id: 'codex', name: 'Codex', configured: false, stored: false, hints: [] }]
        if (cmd === 'panel_placement') return { dockEdge: 'right', railPosition: null }
        if (cmd === 'get_snapshot') return { providers: [], fetchedAt: new Date().toISOString() }
        if (cmd === 'get_network_status') { if (w.networkReadFail) throw Error('无法读取代理设置'); return status() }
        if (cmd === 'save_network_settings') {
          if (w.networkFail) throw Error('代理保存失败，已保留原设置')
          if (args.value.host.includes('://')) throw Error('主机不包含协议、路径或登录信息')
          prefs.networkProxy = structuredClone(args.value); emit('preferences-changed', structuredClone(prefs)); return status()
        }
        if (cmd === 'get_app_info') return { version: '9.8.7', licenses: [{ title: 'Pulse 版权声明', text: 'Pulse by qunqin24 and contributors.' }, { title: 'Pulse · Apache License 2.0', text: 'Apache License Version 2.0' }] }
        if (cmd === 'open_about_link') { if (w.aboutOpenFail) throw Error('无法打开浏览器'); return }
        if (cmd === 'get_update_status') return { currentVersion: '9.8.7', settings: { source: 'https://github.com/wapearoudy/PulseWin/releases/latest/download/latest.json', automatic: true }, phase: 'current', version: null, error: null, notes: null, checkedAt: null }
      },
    }
  })
  await page.goto('/?view=settings')
  await page.getByRole('button', { name: '网络与刷新', exact: true }).click()
  await expect(page.getByLabel('代理方式')).toBeEnabled()
})
const commands = (page: Page, cmd: string) => page.evaluate(cmd => (window as any).networkCommands.filter((v: any) => v.cmd === cmd), cmd)
const manual = async (page: Page) => { await page.getByLabel('代理方式').selectOption('manual'); await expect(page.getByLabel('代理主机')).toBeEnabled() }
const endpoint = async (page: Page) => { await page.getByLabel('代理主机').fill('127.0.0.1'); await page.getByLabel('代理端口').fill('1080'); await page.getByLabel('代理端口').press('Enter'); await expect(page.getByRole('button', { name: '保存代理', exact: true })).toBeDisabled() }

test('opening network and about never enables accounts or starts provider requests', async ({ page }) => {
  await expect(page.getByLabel('代理方式')).toHaveValue('system')
  await expect(page.getByLabel('检查间隔')).toHaveValue('automatic')
  await expect(page.getByLabel('代理主机')).toHaveCount(0)
  await page.getByRole('button', { name: '关于', exact: true }).click()
  await expect(page.getByText('版本 9.8.7', { exact: true })).toBeVisible()
  for (const cmd of ['save_preferences', 'save_network_settings', 'refresh_now', 'refresh_provider', 'set_autostart', 'open_about_link']) expect(await commands(page, cmd)).toEqual([])
})

test('manual endpoint is committed atomically with enter and survives pane changes', async ({ page }) => {
  await manual(page)
  await expect(page.getByRole('status').filter({ hasText: '暂时沿用系统设置' })).toBeVisible()
  await page.getByLabel('代理主机').fill('127.0.0.1')
  await page.getByLabel('代理端口').focus()
  expect(await commands(page, 'save_network_settings')).toHaveLength(1)
  await page.getByLabel('代理端口').fill('1080'); await page.getByLabel('代理端口').press('Enter')
  await expect(page.getByRole('button', { name: '保存代理', exact: true })).toBeDisabled()
  expect((await commands(page, 'save_network_settings'))[1].args.value).toEqual({ mode: 'manual', kind: 'http', host: '127.0.0.1', port: 1080 })
  await page.getByRole('button', { name: '外观', exact: true }).click(); await page.getByRole('button', { name: '网络与刷新', exact: true }).click()
  await expect(page.getByLabel('代理端口')).toHaveValue('1080')
  expect(await commands(page, 'save_network_settings')).toHaveLength(2)
})

test('invalid port or host drafts never overwrite a valid endpoint and can be corrected', async ({ page }) => {
  await manual(page); await endpoint(page)
  for (const invalid of ['', '0', '65536', '1.5', '1e3', 'abc']) {
    await page.getByLabel('代理端口').fill(invalid); await page.getByLabel('代理端口').press('Enter')
    await expect(page.getByRole('alert')).toContainText('原设置已保留')
  }
  expect(await commands(page, 'save_network_settings')).toHaveLength(2)
  await page.getByLabel('代理端口').fill('1081'); await page.getByLabel('代理主机').fill('http://127.0.0.1'); await page.getByLabel('代理主机').press('Enter')
  await expect(page.getByRole('alert')).toContainText('不包含协议')
  expect(await page.evaluate(() => (window as any).networkPrefs.networkProxy.port)).toBe(1080)
  await page.getByLabel('代理主机').fill('127.0.0.1'); await page.getByRole('button', { name: '保存代理', exact: true }).click()
  await expect(page.getByRole('button', { name: '保存代理', exact: true })).toBeDisabled()
  expect(await page.evaluate(() => (window as any).networkPrefs.networkProxy.port)).toBe(1081)
})

test('leaving both inputs saves once and type or system changes use committed endpoints', async ({ page }) => {
  await manual(page); await page.getByLabel('代理主机').fill('127.0.0.1'); await page.getByLabel('代理端口').fill('1080')
  await page.getByRole('heading', { name: '网络与刷新', exact: true }).click()
  await expect(page.getByRole('button', { name: '保存代理', exact: true })).toBeDisabled()
  expect(await commands(page, 'save_network_settings')).toHaveLength(2)
  await page.getByLabel('代理类型').selectOption('socks5'); await expect(page.getByLabel('代理类型')).toBeEnabled()
  await expect(page.getByRole('status').filter({ hasText: '当前来源' })).toContainText('socks5://127.0.0.1:1080')
  await page.getByLabel('代理方式').selectOption('system'); await expect(page.getByLabel('代理主机')).toHaveCount(0)
  await manual(page); await expect(page.getByLabel('代理端口')).toHaveValue('1080'); await expect(page.getByLabel('代理类型')).toHaveValue('socks5')
})

test('failed reads and saves are visible and retry retains entered values', async ({ page }) => {
  await manual(page)
  await page.evaluate(() => { (window as any).networkFail = true })
  await page.getByLabel('代理主机').fill('127.0.0.1'); await page.getByLabel('代理端口').fill('1080'); await page.getByLabel('代理端口').press('Enter')
  await expect(page.getByRole('alert')).toContainText('保存失败')
  await expect(page.getByLabel('代理端口')).toHaveValue('1080')
  await page.evaluate(() => { (window as any).networkFail = false })
  await page.getByRole('button', { name: '保存代理', exact: true }).click(); await expect(page.getByRole('alert')).toHaveCount(0)
  await page.getByRole('button', { name: '外观', exact: true }).click()
  await page.evaluate(() => { (window as any).networkReadFail = true })
  await page.getByRole('button', { name: '网络与刷新', exact: true }).click(); await expect(page.getByRole('alert')).toContainText('无法读取')
  await page.evaluate(() => { (window as any).networkReadFail = false })
  await page.getByRole('button', { name: '重新读取', exact: true }).click(); await expect(page.getByLabel('代理方式')).toHaveValue('manual')
})

test('cadence changes preserve network and account selection and cross-window settings synchronize', async ({ page }) => {
  await manual(page); await endpoint(page)
  await page.getByLabel('检查间隔').selectOption('600')
  expect(await page.evaluate(() => (window as any).networkPrefs)).toMatchObject({ enabledProviders: [], refreshAutomatic: false, refreshSeconds: 600, networkProxy: { port: 1080 } })
  await page.evaluate(() => { const w = window as any; w.networkPrefs.networkProxy = { mode: 'manual', kind: 'socks5', host: '127.0.0.1', port: 1082 }; w.networkEmit('preferences-changed', structuredClone(w.networkPrefs)) })
  await expect(page.getByLabel('代理端口')).toHaveValue('1082'); await expect(page.getByLabel('代理类型')).toHaveValue('socks5')
  await page.getByLabel('检查间隔').selectOption('automatic'); expect(await page.evaluate(() => (window as any).networkPrefs.refreshAutomatic)).toBe(true)
})

test('about uses native version and named links with visible failures and local licenses', async ({ page }) => {
  await page.getByRole('button', { name: '关于', exact: true }).click()
  await expect(page.getByText('版本 9.8.7', { exact: true })).toBeVisible()
  await page.getByText('Pulse · Apache License 2.0', { exact: true }).click(); await expect(page.getByText('Apache License Version 2.0', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: '打开PulseWin 源代码', exact: true }).click()
  expect((await commands(page, 'open_about_link'))[0].args).toEqual({ key: 'source' })
  await page.evaluate(() => { (window as any).aboutOpenFail = true }); await page.getByRole('button', { name: '打开Pulse 原版', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('无法打开浏览器')
})

test('network and about remain readable in light, dark and small windows', async ({ page }) => {
  await manual(page); await endpoint(page)
  for (const colorScheme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme }); await page.screenshot({ path: `test-results/network-${colorScheme}.png` })
    await page.getByRole('button', { name: '关于', exact: true }).click(); await page.screenshot({ path: `test-results/about-${colorScheme}.png` })
    await page.getByRole('button', { name: '网络与刷新', exact: true }).click()
  }
  await page.setViewportSize({ width: 720, height: 480 }); await expect(page.getByLabel('代理主机')).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.getByLabel('搜索设置与账号').fill('SOCKS5'); await expect(page.locator('nav').getByRole('button')).toHaveCount(1)
  await expect(page.locator('nav').getByRole('button', { name: '网络与刷新', exact: true })).toBeVisible()
})
