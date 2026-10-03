# Windows tray usage dashboard

Authoritative original behavior: `Pulse-original/Sources/Pulse/App/MenuDashboard.swift`, `MenuBarReading.swift`, and `Providers/UsagePages.swift`. The Windows tray differs from the macOS menu bar: the notification area exposes one fixed-size image, not arbitrary text next to it.

## User behavior

- Left click opens the compact overview; right click opens the application menu. General settings also has an Open overview button.
- Every enabled account appears in rail order. The overview has each headline quota (including its pin), reset and stale state. A tab shows every quota, plan, observation age, error, real balance, and a known official usage-page action.
- Missing quota remains a dash and empty track. A balance is money, not zero percent. Remaining mode keeps an exhausted quota's warning track full while its figure says zero left.
- Opening, resizing and changing tabs do not refresh quota. Manual refresh asks the selected account, or all enabled accounts in Overview. A slow refresh does not prevent closing the popup.
- Local spend is opt-in, reads only supported primary accounts, uses the existing five-minute retained card history, and labels API cost estimates as estimates. Additional accounts never borrow the primary account's ledger. The dashboard shows today, last 31 days, busiest day and all available records plus the 31-day token chart. Reader coverage is unchanged by this milestone.
- Selected account survives window hide/reopen; a removed or disabled selection falls back to Overview. Arrow keys/Home/End navigate tabs; many-account tabs scroll horizontally with icon-only labels and accessible names.
- General settings selects automatic tightest pinned quota or an enabled account. The tray uses figure, mini-ring or two evidenced account-wide timed quotas. Unknown/scoped windows are excluded from split; fewer than two eligible windows falls back to figure. Warning color is based on used quota regardless of remaining display.
- The tooltip holds full labels within Windows' 127 UTF-16 unit limit. Only known upstream official links are permitted; no endpoint is guessed as a human-facing page.
- Desktop assistant visibility is an independent persisted preference. Hiding the assistant retains enabled accounts and collection. Startup restores tray-only mode; fullscreen recovery cannot reveal an intentionally hidden assistant.

## Native window implementation and verification

Window `usage` is a separate 320 logical pixel WebView2 with no taskbar button. The physical work area and clicked monitor DPI bound its position and height; content height is capped at 560 logical pixels. Resizing the popup never resizes the assistant. Lost focus and Escape hide it. All dynamic window creation runs off the Windows UI thread and is serialized: synchronous WebView2 construction from a command or tray callback can deadlock.

`node scripts/tray-smoke.mjs` uses isolated APPDATA, instance identity and WebView2 storage. It seeds two synthetic, credentialless additional Codex accounts; the collection route cannot fall back to the user's CLI login. Real native IPC verifies account tabs, compact geometry at 150% scaling, saved tray style, panel visibility, preserved enabled-account collection, Escape and a complete process restart. Original assistant geometry/account isolation also passes the existing native smoke.

Full regression: 849 Rust (1 ignored), 181 frontend, 49 browser tests; 13 targeted dashboard browser tests after the final window-thread/closing changes. Browser screenshots cover light/dark; native screenshots and release evidence are kept in `docs/screenshots/` and `docs/release-0.1.9.json`.

Unverified: physical taskbar mouse event, mixed-DPI transfer, Windows taskbar scaling/visual sharpness of dynamic tray images, and installer execution/restart. No claim of macOS pixel identity or complete product parity is made.
