import { test, expect, type Page } from '@playwright/test'

// Native registration and the OS login-start state are fixture responses here;
// the test never registers a shortcut or changes Windows startup settings.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as any
    let sequence = 0
    const callbacks: Record<number, Function> = {}, listeners: Record<number, any> = {}
    const prefs = { enabledProviders: ['claude-code'], providerOrder: ['claude-code', 'codex'], accountAppearance: {}, pinnedWindows: {}, openSettingsShortcut: 'Control+KeyZ', togglePanelShortcut: 'Alt+KeyZ' }
    w.applicationState = { openSettingsShortcut: null, togglePanelShortcut: null, unavailable: {}, autostartEnabled: false, autostartError: null }
    w.applicationCommands = []; w.autostartFail = false; w.shortcutFail = false; w.applicationReadFail = false; w.acceptAutostartChange = true
    const emit = (event: string, payload: unknown) => Object.values(listeners).filter(x => x.event === event).forEach(x => callbacks[x.handler]?.({ event, payload }))
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_: string, id: number) => delete listeners[id] }
    w.__TAURI_INTERNALS__ = {
      transformCallback: (fn: Function) => { callbacks[++sequence] = fn; return sequence },
      invoke: async (cmd: string, args: any = {}) => {
        w.applicationCommands.push({ cmd, args })
        if (cmd === 'plugin:event|listen') { listeners[++sequence] = args; return sequence }
        if (cmd === 'plugin:event|unlisten') return
        if (cmd === 'get_preferences') return structuredClone(prefs)
        if (cmd === 'save_preferences') { Object.assign(prefs, args.value); emit('preferences-changed', structuredClone(prefs)); return }
        if (cmd === 'provider_settings') return ['claude-code', 'codex'].map(id => ({ id, name: id === 'codex' ? 'Codex' : 'Claude Code', configured: true, stored: false, credentialPath: null, hints: [] }))
        if (cmd === 'panel_placement') return { dockEdge: 'right', railPosition: null }
        if (cmd === 'get_snapshot') return { providers: [], fetchedAt: new Date().toISOString() }
        if (cmd === 'get_application_settings') {
          if (w.applicationReadFail) throw new Error('模拟系统设置读取失败')
          return structuredClone(w.applicationState)
        }
        if (cmd === 'set_application_shortcut') {
          if (w.shortcutFail) throw new Error('模拟快捷键保存失败')
          const state = w.applicationState, field = args.action === 'openSettings' ? 'openSettingsShortcut' : 'togglePanelShortcut'
          state[field] = args.value
          delete state.unavailable[args.action]
          if (args.value && state.openSettingsShortcut === state.togglePanelShortcut) state.unavailable[args.action] = '此快捷键已被另一个操作或其他应用占用。请选择不同的组合。'
          return structuredClone(state)
        }
        if (cmd === 'set_autostart') {
          if (w.autostartFail) throw new Error('Windows 拒绝修改登录启动设置')
          if (w.acceptAutostartChange) w.applicationState.autostartEnabled = args.enabled
          return structuredClone(w.applicationState)
        }
      },
    }
  })
  await page.goto('/?view=settings')
  await page.getByRole('button', { name: '通用', exact: true }).click()
  await expect(page.getByRole('heading', { name: '通用', exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: '录入打开设置快捷键' })).toBeEnabled()
})

const openShortcut = (page: Page) => page.getByRole('button', { name: '录入打开设置快捷键', exact: true })
const panelShortcut = (page: Page) => page.getByRole('button', { name: '录入显示或隐藏面板快捷键', exact: true })
const mutations = (page: Page) => page.evaluate(() => (window as any).applicationCommands.filter((x: any) => ['set_application_shortcut', 'set_autostart', 'save_preferences', 'refresh_provider', 'refresh_now'].includes(x.cmd)))

test('defaults read the OS and remain unset without startup or provider mutations', async ({ page }) => {
  await expect(openShortcut(page)).toHaveText('未设置')
  await expect(panelShortcut(page)).toHaveText('未设置')
  await expect(page.getByRole('switch', { name: '登录时启动 Pulse' })).not.toBeChecked()
  await page.keyboard.press('Control+Alt+KeyP')
  expect(await mutations(page)).toEqual([])
  await page.screenshot({ path: 'test-results/application-general.png', fullPage: true })
  await page.setViewportSize({ width: 720, height: 480 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

test('local recording swallows invalid keys and commits one physical-key combination', async ({ page }) => {
  await page.evaluate(() => { (window as any).recordingBubbleKeys = []; document.addEventListener('keydown', event => (window as any).recordingBubbleKeys.push(event.code)) })
  await openShortcut(page).click()
  await page.keyboard.press('KeyP')
  await expect(openShortcut(page)).toHaveText('请加 Ctrl、Alt 或 Win')
  await page.keyboard.press('Shift+KeyP')
  expect(await mutations(page)).toEqual([])
  await page.keyboard.down('Control')
  await expect(openShortcut(page)).toHaveText('Ctrl')
  await page.keyboard.down('Alt')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt')
  expect(await page.evaluate(() => (window as any).recordingBubbleKeys)).toEqual([])
  await page.keyboard.press('KeyP')
  await page.keyboard.up('Alt'); await page.keyboard.up('Control')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  expect(await mutations(page)).toEqual([{ cmd: 'set_application_shortcut', args: { action: 'openSettings', value: 'Control+Alt+KeyP' } }])
})

test('escape cancels, delete clears, and leaving the pane ends local capture', async ({ page }) => {
  await openShortcut(page).click(); await page.keyboard.press('Control+Alt+KeyP')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  await openShortcut(page).click(); await page.keyboard.press('Escape')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  expect(await mutations(page)).toHaveLength(1)
  await openShortcut(page).click(); await page.keyboard.press('Backspace')
  await expect(openShortcut(page)).toHaveText('未设置')
  await panelShortcut(page).click(); await page.keyboard.press('Alt+KeyH')
  await expect(panelShortcut(page)).toHaveText('Alt + H')
  await panelShortcut(page).click(); await page.keyboard.press('Delete')
  await expect(panelShortcut(page)).toHaveText('未设置')
  await panelShortcut(page).click()
  await page.getByRole('button', { name: '外观', exact: true }).click()
  await page.keyboard.press('Control+Alt+KeyM')
  expect(await mutations(page)).toHaveLength(4)
  await page.getByRole('button', { name: '通用', exact: true }).click()
  await expect(panelShortcut(page)).toHaveText('未设置')
  await panelShortcut(page).click(); await page.keyboard.press('Alt+KeyH')
  await expect(panelShortcut(page)).toHaveText('Alt + H')
  await page.getByRole('button', { name: '清除显示或隐藏面板快捷键' }).click()
  await expect(panelShortcut(page)).toHaveText('未设置')
  expect((await mutations(page)).map((x: any) => x.args.value)).toEqual(['Control+Alt+KeyP', null, 'Alt+KeyH', null, 'Alt+KeyH', null])
})

test('registration conflicts replace the affected subtitle and removal clears them', async ({ page }) => {
  await openShortcut(page).click(); await page.keyboard.press('Control+Alt+KeyP')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  await panelShortcut(page).click(); await page.keyboard.press('Control+Alt+KeyP')
  await expect(panelShortcut(page)).toHaveText('Ctrl + Alt + P')
  const row = page.locator('.setting-row').filter({ has: panelShortcut(page) })
  await expect(row.getByRole('alert')).toHaveText('此快捷键已被另一个操作或其他应用占用。请选择不同的组合。')
  await expect(row.getByText('显示用量面板，或将它隐藏。', { exact: true })).toHaveCount(0)
  await expect(page.locator('.setting-row').filter({ has: openShortcut(page) }).getByRole('alert')).toHaveCount(0)
  await page.screenshot({ path: 'test-results/application-shortcut-conflict.png', fullPage: true })
  await page.getByRole('button', { name: '清除显示或隐藏面板快捷键' }).click()
  await expect(row.getByRole('alert')).toHaveCount(0)
  await expect(row.getByText('显示用量面板，或将它隐藏。', { exact: true })).toBeVisible()
})

test('login-start state follows the OS response and visible failures keep that state', async ({ page }) => {
  const startup = page.getByRole('switch', { name: '登录时启动 Pulse' })
  await startup.check()
  await expect(startup).toBeChecked()
  expect(await mutations(page)).toEqual([{ cmd: 'set_autostart', args: { enabled: true } }])
  await page.evaluate(() => { (window as any).acceptAutostartChange = false })
  await startup.click()
  await expect(startup).toBeChecked()
  await page.evaluate(() => { (window as any).autostartFail = true })
  await startup.click()
  await expect(page.getByRole('alert')).toContainText('Windows 拒绝修改登录启动设置')
  await expect(startup).toBeChecked()
  expect((await mutations(page)).map((x: any) => x.args.enabled)).toEqual([true, false, false])
  await page.evaluate(() => { const w = window as any; w.applicationState.autostartEnabled = null; w.applicationState.autostartError = '无法读取 Windows 登录启动状态' })
  await page.getByRole('button', { name: '外观', exact: true }).click()
  await page.getByRole('button', { name: '通用', exact: true }).click()
  await expect(startup).toBeDisabled()
  await expect(page.getByRole('alert')).toHaveText('无法读取 Windows 登录启动状态')
  expect(await mutations(page)).toHaveLength(3)
})

test('read and write failures are recoverable without changing a saved shortcut', async ({ page }) => {
  await openShortcut(page).click(); await page.keyboard.press('Control+Alt+KeyP')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  await page.evaluate(() => { (window as any).shortcutFail = true })
  await openShortcut(page).click(); await page.keyboard.press('Alt+KeyH')
  await expect(page.getByRole('alert')).toContainText('模拟快捷键保存失败')
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  await page.evaluate(() => { (window as any).applicationReadFail = true })
  await page.getByRole('button', { name: '外观', exact: true }).click()
  await page.getByRole('button', { name: '通用', exact: true }).click()
  await expect(page.getByRole('alert')).toContainText('模拟系统设置读取失败')
  await expect(openShortcut(page)).toBeDisabled()
  await page.evaluate(() => { (window as any).applicationReadFail = false })
  await page.getByRole('button', { name: '重新读取', exact: true }).click()
  await expect(openShortcut(page)).toBeEnabled()
  await expect(openShortcut(page)).toHaveText('Ctrl + Alt + P')
  expect(await mutations(page)).toHaveLength(2)
})
