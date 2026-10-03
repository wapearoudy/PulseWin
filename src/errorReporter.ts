/**
 * Frontend diagnostics channel.
 *
 * A Windows webview has no console. A JavaScript exception therefore shows up
 * as nothing at all — the panel renders empty and there is no output anywhere
 * to read. This forwards everything that escapes to the Rust side, which
 * appends it to `%TEMP%\pulsewin-errors.log`.
 *
 * This lives in a module rather than an inline `<script>` because the app's CSP
 * is `default-src 'self'`, and `script-src` falls back to it: an inline script
 * is silently blocked. Silently, which is the whole problem this file exists to
 * solve.
 */

import { invoke } from '@tauri-apps/api/core'

function report(message: string) {
  try {
    void invoke('report_error', { message }).catch(() => undefined)
  } catch {
    /* the bridge itself is what failed */
  }
}

let installed = false

export function installErrorReporter() {
  if (installed) return
  installed = true

  window.addEventListener('error', (event) => {
    const where = event.filename ? ` @ ${event.filename}:${event.lineno}:${event.colno}` : ''
    report(`error: ${event.message || String(event)}${where}`)
  })

  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason as { stack?: string } | undefined
    report(`unhandledrejection: ${String(reason?.stack ?? event.reason)}`)
  })

  report('boot: reporter installed')
}

/** A plain beacon, so "nothing rendered" can be told apart from "nothing ran". */
export function beacon(message: string) {
  report(message)
}
