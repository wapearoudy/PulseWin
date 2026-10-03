import { defineConfig } from '@playwright/test'
export default defineConfig({
  testDir: './e2e',
  use: { baseURL: 'http://127.0.0.1:5183', viewport: { width: 960, height: 720 }, channel: 'msedge', trace:'retain-on-failure', screenshot:'only-on-failure' },
  webServer: { command: 'npm run dev -- --host 127.0.0.1', url: 'http://127.0.0.1:5183', reuseExistingServer: true },
})
