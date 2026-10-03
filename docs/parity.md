# Pulse Windows parity work

Reference: the checked-in `../../Pulse-original` source and its authoritative `Docs/` topics.
Target: equivalent product behavior on Windows, and faithful layout and interaction. A registered provider, passing parser fixture, or successful build is not proof of product parity.

## Acceptance inventory

All unchecked items remain incomplete or unverified. No overall completion percentage is claimed.

- [ ] First-run explicit provider selection, dismissal, upgrade offers, restoration
- [ ] Provider enable/disable gates every timer/manual/credential refresh
- [ ] Account order, per-account refresh, extra accounts and identity isolation
- [ ] Settings sidebar and all Appearance, Behavior, Application, account panes
- [ ] Provider-specific authentication, source selection, diagnostics and recovery
- [ ] All 77 provider routes audited against Swift; real-account verification recorded separately
- [ ] Usage semantics: authoritative exhaustion, estimates, balance basis, split/pinned windows
- [ ] Last-good cache, stale labels, expiry and route/account scoped reconciliation
- [ ] Adaptive refresh, activity, wake recovery and superseded-pass rejection
- [ ] History, ledger, burn rate, forecasts and charts
- [ ] Token spend, agent/source readers and model drill-down
- [ ] Notifications and reset/window starter behavior
- [ ] Panel geometry, glass, ring/clock direction, grouping and animations
- [ ] Pointer, drag, context menu, multi-monitor/DPI, full-screen, focus and click-through
- [ ] BotMark complete animation/state behavior
- [ ] Shortcuts, startup, visibility, proxy and update settings
- [ ] Extensions, JSON export, deep links and developer integrations
- [ ] Localization and keyboard/accessibility behavior
- [ ] Installer plus end-to-end acceptance with screenshots and actual native interaction

## Verification baseline

Current Token Spend readers: 12 of 54. DeepSeek Harness is implemented in
0.1.12; 42 sources remain unsupported. Four implemented readers require prepared
exports/captures. Historical milestone counts below describe their release date.

0.1.12 Harness acceptance: ordinary/versioned JSONL and concatenated Zstandard
frames, fork inheritance, failed-attempt settlements and retries, compaction
usage, inclusive reasoning, copy deduplication, bounded/cancellable streaming,
cache hits and appended-frame invalidation. Local compressed user logs were
read without modifying any input and independently checked against Python's
Zstandard decoder; aggregate counters matched with no reading warnings. Private
logs, paths and totals were not published. Real release WebView2/IPC used only
synthetic session fixtures. See `harness-token-spend.md`, `release-0.1.12.json`
and `screenshots/native-harness.png`. Validation: 883 offline Rust, 182 frontend
and 59 browser tests; native Harness, assistant, tray, CLI, network and signed
update checks pass. Actual installation/overwrite/restart was not executed.

2026-10-02: existing frontend build passes; 102 frontend tests pass. These cover pure rules and geometry, not end-to-end parity.

### Implemented milestone — selection, preferences and settings

- Explicit initial selection; empty selection starts no collection. Dismissing keeps Settings reachable.
- Selection gates collection before provider credential discovery; targeted refresh checks enablement.
- Preferences persist separately from dock placement and credentials. Disabled accounts are removed immediately; outdated in-flight results cannot reintroduce them.
- Account ordering changes the panel without fetching. Appearance changes update through a cross-window event.
- Sidebar settings, provider detail, visible save/load errors and account-scoped refresh.
- Original size (0.82/1/1.22), spacing (0.6/1/1.4) and warning threshold choices (default 75%).
- Remaining usage, round ends, labels, auto-collapse, manual cadence, pinned windows.
- Reset clock only when a reported duration exists; Codex adapter preserves it. Other adapters still need auditing.
- Explicit restriction flag in the model. Visual saturation at 100% remains consistent with original `UsageTint.swift`; that is distinct from a provider-authoritative exhausted flag.
- Saved credential edits preserve omitted values. CLI token resolution now consumes the saved credential path.
- Second-instance `--settings` opens Settings; showing the panel no longer explicitly steals focus.

Verification: 685 Rust tests, 106 frontend tests, and 2 Playwright flows pass. Browser tests mock IPC and do not prove native interaction or real provider connectivity. Screenshots: `../test-results/settings-appearance.png`, `settings-account.png`, `settings-small.png`.

Native smoke also passes against the release executable using real WebView2/Tauri IPC: startup, empty-selection gate, settings event permissions, preference persistence and shared panel state. Run `npm run test:native` after building. Evidence: `../test-results/native-settings.png` and `native-smoke.json`. Test profiles live under ignored `test-results/`; the native test process could not write the host temporary directory (WebView2 creation failed with 0x8000FFFF, preference writes with access denied). Moving only the isolated test profile into the writable workspace resolves both. No machine permissions or installed runtime were changed. Native drag/focus and cross-monitor DPI transitions remain unverified; floating startup position is covered by the later assistant milestone.

Still open within this milestone: original subscription/API grouping, complete access descriptions, upgrade offers, multi-account slots, complete settings/localization, actual multi-monitor interaction and provider-specific credential flows. This milestone does not check off a whole parity category prematurely.

## Platform adaptations

macOS-only APIs (AppKit/Keychain/Spaces/SMAppService/Liquid Glass) require Windows implementations with equivalent user-facing behavior. Unsupported or unverified adaptations stay in this inventory rather than being counted as complete.


### Desktop assistant focus — screenshot reference

- Free floating rail, circular capsule ends, solid black surface, original floating end padding (22pt), 64pt width and 88pt ring pitch. Seven entries occupy 630pt.
- Fixed SVG capsule arcs that previously expanded outside the declared bounds. Fixed ring SVG layout incorrectly reserving clock overflow in the slot; first floating ring center is now 40pt from the top.
- Floating rail stays expanded; docked rail collapses after 320ms. Background between slots is interactive and draggable. Only circle clicks refresh; movement of 5px suppresses refresh.
- Rail coordinates and facing side persist independently from the wider card window. Drop near left/right/top docks; other drops float. Placement checks available displays and clamps to the selected display.
- Native pointer coordinates feed BotMark gaze without restarting its animation. Idle phases differ between characters.
- Added browser regression tests for capsule bounds, ring centers, floating/collapsed states, click/drag distinction, and card hover frame stability. Native smoke additionally checks restored floating coordinates and screenshots the assistant using isolated synthetic readings.

Remaining: docking morph during the drag (currently on release), full original BotMark choreography/particles/body transformations, assistant context menu, complete multi-display/DPI/hotplug acceptance, rounded hit testing, and all previously unchecked items. Screenshot fixtures are not real account results; this is not a 100% parity claim.


### README-driven assistant parity, 2026-10-03

Full feature/UI inventory: [readme-coverage.md](readme-coverage.md). This is the authoritative scope checklist; totals from unit tests are not feature coverage.

Implemented: original provider SVG assets; account-level animated mark switch, eight persona playlists copied from Swift, eighteen body shapes, body/ring colors; persistent animation engine with springs, expression history, face fitting/clipping and solid yaw; independently configurable second ring, side/top figures, activity animation and collapsed warning color; native rail context menu with a hold while open; settings position selector; scoped refresh events with a 650ms minimum; nonzero/nonfull percentage boundaries. Animation still has the explicitly listed gaps in the feature inventory.


Further assistant work in the same milestone: local lifecycle scanners (Claude Code, Codex, Kiro, ZCode), witnessed per-persona completion beats, original single-reading forecast with evidence gates, fullscreen avoidance, optional active-display following and monitor-change observation, docking while dragging with quarter-turn/grab compensation, detail-card path winding/tail-width/error-budget repairs, minimum six-row frame budget and separate Rings and figures settings. The latest status and verification limits supersede the earlier milestone notes above; see readme-coverage.md.

Verification: 698 Rust unit tests and 116 frontend tests. Native smoke passes with isolated configuration and synthetic usage, verifying the real Tauri IPC and WebView2 renderer. Fullscreen and mixed-DPI follow rules have unit coverage, not native app-by-app acceptance. The OS automation helper cannot target the click-through collapsed rail reliably without an existing cursor hover, so complete native drag acceptance remains open.

### 0.1.2 milestone, 2026-10-03

The coverage inventory supersedes the older counts and open items above. Added current-user DPAPI
credential migration, last-good/dated cache, evidence-based notifications and reset stamps, read-only
JSON/status-line output, global shortcut recording/registration, actual login-start status, and
per-provider one-shot adaptive refresh with Windows power/recovery signals.

BotMark now includes the original effects, front/back ribbons, random gestures, shape continuity
and full reset celebration. The standard mark canvas is 28pt. Native placement now records stable
display identities and per-display preferences, preserves an absolute anchor on same-screen size
changes, and recalls proportions on display/work-area/DPI changes. This fixes the reproduced native
empty-to-seven-entry Y=80→45 regression rather than relaxing its acceptance assertion.

The assistant claims clicks using its actual SVG silhouette while retaining original hover
forgiveness. Money-only accounts show reported credit, not an unavailable placeholder. Missing
percentages remain empty in remaining mode; second rings prefer the headline's real model pool
where known. Claude's full limits payload, warning-versus-block semantics and literal sub-one-percent
readings are preserved.

Token Spend has eight actual readers: Claude, Codex, Qwen, Gemini plus four explicit import/capture
formats. Forty-six sources remain unsupported, including native OpenCode/Antigravity databases.
Public price estimates, unknown-price handling, cancellation, file caching, model drill-down,
daily/hour charts, project/session sorting and pagination are available. Aggregated exports cannot
prove hourly usage and the UI says so.

Validation: 808 complete Rust regressions followed by 41 reader regressions after the final
metadata repair, 177 frontend unit tests, and 27 browser flows (one SVG assertion corrected and
rerun). Production and installer/native verification are recorded in the coverage inventory. See
`design-qa.md` for visual evidence and remaining acceptance boundaries. Multi-account authentication,
extensions, updates, glass and localization are still incomplete; this milestone does not close the
full-replica goal.

Final 0.1.2 release validation: both NSIS/MSI packages built and all three generated binaries have
Medium integrity. Real WebView2 smoke passes with DPI 1.5 and [720,80] preserved after the rail
grows from empty to seven entries. Actual OS shortcut registration and read-only startup status
queries pass, but Computer Use app approval timed out before native key/mouse input; those checks
remain unverified. Release CLI smoke passes with an isolated cache, no credential migration and
byte-for-byte unchanged input files. All 27 browser flows pass in the final complete run. Evidence
screenshots are copied into `docs/screenshots` so later test runs cannot erase them.

### 0.1.3 milestone, 2026-10-03

Added per-account opt-in detailed cards: original dimensions and history figures, 31-day bars, dominant model, measured cache-hit rate and guarded API-price window estimates. Both account appearance and Token Spend permission gate history reads. History is provider-scoped, cached for five minutes, cancelled on opt-out, and never reuses a prior account's numbers. Fixed legacy dashboard CSS leaking 11px padding, borders and ring track colors into the assistant; a real browser assertion reproduces and guards this defect.

Read-only SQLite readers now cover OpenCode, Kilo CLI and MiMo Code. WAL/journal fingerprints invalidate cached records; fixture tests exercise live WAL commits, read-only byte preservation, malformed/partial counts, cancellation and mirrored-message deduplication. Vendor-specific plan prices stay scoped to their agents, with first-party model prices preferred. Eleven of fifty-four sources are implemented; four still require prepared import files. Forty-three sources remain unsupported.

OpenCode Go now preserves window identity/duration/restriction metadata, actual credential source and XDG login lookup; invalid keys, subscription-entitlement errors and throttling are distinct. An error card opens the matching account pane. The real CLI key on this machine produced HTTP 403 EntitlementError from the official usage endpoint. No successful online quota reading is claimed. The user must supply the key belonging to the subscribed account/workspace through local settings.

Validation: 824 offline Rust regressions, 181 frontend tests, 30 browser flows; the separate ignored live Go test also passed with the expected actionable entitlement refusal. Detailed-card screenshots are retained under docs/screenshots. This milestone leaves multi-account login, extensions, updates, glass, localization and remaining readers/real-provider acceptance open.
Final 0.1.3 release validation: both installers and the application built with bundled rusqlite MIT notice and Medium integrity. Native WebView2 smoke and read-only CLI smoke passed. Computer Use successfully sent the temporary global shortcut and observed the isolated settings window opening. Background drag was blocked by the tool's target-window check even after fresh activation/capture; native drag/click-through acceptance remains open. The installer was only launched hidden and stopped, without installation or a welcome-page capture. Hashes and explicit validation boundaries are in release-0.1.3.json.

### 0.1.4 milestone, 2026-10-03

Signed in-app updates now support local build folders and public GitHub Releases. General settings and assistant/tray menus expose checks, update notes, progress, errors and explicit installation/restart. Automatic checks run after startup and every thirty minutes. Signed versions prevent a forged manifest from presenting an old package as a newer build; the same Tauri verifier handles both sources. Account preferences and credentials retain their existing storage locations.

Gemini OAuth client constants are no longer bundled. Refresh uses a complete client pair from explicit environment settings, credential JSON, or the installed Gemini CLI's static configuration. An unexpired access token does not need these values. Missing refresh configuration reports how to renew the CLI login rather than inventing credentials.

Validation: 832 offline Rust regressions (one online test ignored), 181 frontend tests and 35 browser flows. Real native signed-download verification rejects tampered bytes and mismatched versions; native and read-only CLI smoke also pass. Installer execution and restart still require separate acceptance. This milestone does not establish full Pulse parity; remaining gaps above still apply. Evidence: release-0.1.4.json and screenshots/native-updates.png.

### 0.1.5 maintenance release, 2026-10-03

Online acceptance caught update configuration using the Windows Known Folder independently of the application's APPDATA profile. Updates now share the existing settings directory. Native regressions assert fresh default source, persisted isolated configuration and byte-for-byte unchanged personal update settings. The 0.1.4 signature checks remain valid, but configuration isolation was not established by those earlier tests. The current release evidence is release-0.1.5.json.

Public release acceptance passed: five anonymous asset downloads match local SHA-256; the current native client checks its default GitHub feed, and a real 0.1.4 client detects, downloads and verifies the actual published 0.1.5 package without executing installation. Exit, overwrite and restart acceptance remains open.

### 0.1.9 — tray usage dashboard and persistent tray-only mode

The tray's left click opens a separate compact overview with per-account tabs; right click retains the application menu. Account detail reads the published quota snapshot, never requests quota on opening, supports explicit scoped refresh, and reports every real limit, reset, balance, stale state and error. Known official page links are copied from the upstream UsagePages table; unknown pages have no guessed link. Primary local history reuses the existing opt-in readers and five-minute card cache; additional accounts cannot use the primary ledger.

General settings select the tightest pinned quota automatically or a named enabled account, with figure/ring/split styles. Windows uses a 32px RGBA notification-area icon and a UTF-16-limited tooltip because it cannot place text beside a tray icon. Split requires two distinct provider-evidenced account-wide durations, excluding model scopes. Missing percentages never become zero. Warning severity uses consumed quota even in remaining mode.

Panel visibility is persisted independently from enabled accounts and restored on process startup. Hiding the assistant does not disable collection. Native dashboard geometry uses the clicked monitor's physical work area and DPI, stays inside it, and adapts its own content height without changing the assistant frame. Escape and lost focus hide the dashboard.

Verification: 849 Rust, 181 frontend, 49 full-suite browser tests, followed by 13 targeted dashboard tests after the native window-thread and pending-close corrections. Native IPC, restart and release evidence: `docs/release-0.1.9.json`, `docs/tray-dashboard.md`. Physical taskbar clicks and mixed-DPI transitions remain unverified; this milestone is not a declaration of complete parity.


### 0.1.10 milestone — network and About

Added system / manual HTTP CONNECT / SOCKS5 settings, atomic validated endpoints, shared provider policy and proxy reconfiguration cancellation. Proxy changes retry only enabled accounts; generic stale preference writes cannot replace the authoritative network configuration. Kiro / Alibaba helper environments and models.dev price fetches consume the saved policy; gateway redirects remain disabled, editor loopback remains direct and updates retain system proxy policy. Network refresh cadence moved to its own page. About exposes compiled version, existing signed updates, allowlisted project / attribution links and bundled licenses.

Verification: 861 Rust tests plus one ignored, 181 frontend tests, 58 browser flows and 13 final targeted flows. Real native isolated-profile smoke tests proxy replacement and restart persistence without an upstream connection; CONNECT fake servers observe FIN by draining their socket. Details and remaining PAC / auth / real-account limits are in [networking.md](networking.md).


### 0.1.11 — malformed system proxy compatibility

System proxy configuration is external to validated application preferences. Restored the previous tolerance for malformed WinINET URL values so users can still launch Settings and choose a valid manual endpoint. Added client-initialization and real system bypass-list regressions; 863 Rust tests pass plus one ignored. The unchanged frontend retains the 0.1.10 browser and unit evidence. Final binary and public release checks are recorded in [release-0.1.11.json](release-0.1.11.json).

Public 0.1.12 acceptance: all five anonymously downloaded release assets match
local SHA-256; the current app checks the default GitHub feed, and a real 0.1.4
client downloads and verifies the actual published signed 0.1.12 package. No
installer was executed. Detailed evidence is in `release-0.1.12.json`.
