# PulseWin

> **Parity work in progress (2026-10-03).** This is not yet a complete port of Pulse.
> The desktop assistant now includes original provider artwork, per-account bots with eight
> personalities/eighteen bodies, lifecycle activity, hover cards, scoped refresh, forecast,
> drag docking and persisted preferences. The precise implemented/partial/missing status is in
> [the official README feature/UI comparison](docs/readme-coverage.md).
> All 77 provider adapters are registered; registration is not real-account validation.
> Token Spend now has eleven actual readers, notifications have evidence-based alert
> rules, and the original bot animation engine/effects are ported. Multi-account authentication,
> the remaining Token Spend readers, extensions, localization and full
> native/multiple-display acceptance remain open.
> Existing installations explicitly select the services to monitor; stored credentials are retained.

**Download:** [GitHub Releases](https://github.com/wapearoudy/PulseWin/releases). Install 0.1.6 once if upgrading from 0.1.3 or earlier; 0.1.4 and 0.1.5 can update directly. Subsequent releases can be installed with **Settings → General → Software updates → Update and restart**; preferences and credentials stay in place. Both GitHub Releases and local build directories are supported. See [update and release instructions](docs/updates.md).


A **Windows** port of [Pulse](https://github.com/qunqin24/Pulse) — a tray monitor that shows how
much quota is left across the AI coding tools installed on the machine.

The original is a macOS menu-bar app written in Swift. PulseWin uses React and Rust/Tauri for Windows. See the acceptance inventory for the distinction between implemented, tested and unverified behavior.

## Status

Target platform: Windows 11 Pro (x64). The last release build in this tree produced:

| Artifact | Path | Size |
| --- | --- | --- |
| Application | `src-tauri/target/release/pulsewin.exe` | ~7.06 MiB |
| Installer (MSI) | `src-tauri/target/release/bundle/msi/PulseWin_0.1.6_x64_en-US.msi` | ~4.38 MiB |
| Installer (NSIS) | `src-tauri/target/release/bundle/nsis/PulseWin_0.1.6_x64-setup.exe` | ~2.74 MiB |

What is covered by tests, and what is not, is set out under [Tests](#tests).

---

## The panel

A rail capsule fused to one screen edge — **left, right or top**. Every explicitly enabled service gets a ring; credential discovery does not enable monitoring. A missing reading is shown as unavailable, never a fabricated zero.

- **Hovering a ring opens a bubble card** beside it, listing that tool's limits, each with its own
  bar, percentage, detail line and reset. **The window does not resize while a card is open.** The
  frame is sized in advance for the largest card any provider could open and the card is drawn as an
  overlay inside it, which is why every length in `cardLayout.ts` is a budget rather than a
  measurement.
- **Clicking a ring refreshes now.** A side rail prints the percentage under each ring; a top rail
  hides the labels by default, because a second line of type under the top of the screen turns a
  compact pill into a banner.
- **Drag the rail by a ring or its background.** It docks at the left/right/top while moving,
  floats away from the edges, and keeps the grab position when the dock padding changes. A turn
  onto the top axis centres under the pointer. Rail position is written to
  `%APPDATA%\PulseWin\panel.json` and restored on the next launch.
- **Unattended, it winds down to a 6pt sliver.** That sliver is the rail's own silhouette at
  `openness` 0 rather than a second shape, so the flare and the corners give way together. It opens
  on any contact and closes 320 ms after the pointer leaves, which is long enough to walk from one
  ring to the next without shutting it.
- **The empty part of the window is handed back to the desktop.** The window is several times wider
  than the rail so a card can open inside it, and a transparent window still eats clicks: a pointer
  watcher in `lib.rs` samples the cursor every 60 ms and flips the window between click-through and
  interactive as the pointer crosses the regions the frontend claims. Without it, the invisible
  margin would swallow every click meant for the window underneath.
- **The tray icon**: left click shows or hides the panel, right click offers Show PulseWin /
  Settings… / Refresh now / Quit, and the tooltip names the tightest quota on the rail.
- Automatic refresh uses a per-provider 2/5/15/30-minute schedule based on activity, inspection,
  quota changes and panel visibility; a fixed interval can also be selected. A fetch that fails still draws its ring —
  a ring that has gone quiet is the one being looked for — and its card says what went wrong and
  how to open its connection settings.

## BotMark

`src/panel/botmark/`. The mascot that stands where a provider's logo stood.

- **18 bodies and 25 eye expressions**, ported verbatim from the original's own `bot-data.json`
  (39 states in all). The file is copied unchanged: transcribing a drawing by hand would have been a
  paraphrase of it.
- **Five moods** — idle, working, fetching, spent, unavailable — resolved in `mood.ts` from facts
  the rail already has, each mapped onto one of the original's states. Busy outranks spent: a spent
  limit will still be true in a minute, a CLI turning is happening now. This is the only place a
  Pulse fact becomes a bot state — if the mark is doing something, a reading somewhere says why.
- **Idle and working animation**: breath, rock, blink, a change of expression and a gaze that
  follows the pointer, written straight onto the SVG nodes from one `requestAnimationFrame` loop per
  mark rather than through React state. This runs inside a ring that is already being repainted, and
  a re-render per frame per ring would be the panel's whole budget.
- **Colour is identity, never status.** `botmark/tint.ts` is a literal port of the original's brand
  table, and providers whose logo is monochrome by design are dealt a colour from a wheel so that no
  two neighbouring marks look alike. How close a limit is stays the ring's job (`panel/tint.ts`).
- Original per-persona playlists, spring state, face fitting, solid yaw, task morphs, front/back
  particle ribbons, random gestures and full reset celebrations are implemented. Completion beats
  require witnessed work ending, and reset celebrations require a fresh reset event. Reduced-motion
  mode shows a settled frame. Formula and timing tests do not prove frame-by-frame visual identity.

## The settings window

Tray → **Settings…**, `pulsewin.exe --settings`, or the Settings… button on a card that has no
reading. It is a second window rather than a view inside the panel, because the panel's frame is
worked out from the rail's length and nothing may resize it while a card is open.

It lists every registered provider, whether it was found on this machine, and — under "Where it
looks" — the paths that provider reads plus the file a pasted credential would be saved to. Reading
the list makes no network call, so opening the window cannot fire fifty requests at services you do
not use.

Each row takes a **pasted API key / token / cookie** and, for the self-hosted gateways, a **server
address**. Saving encrypts the complete credential document with Windows current-user DPAPI in
`%APPDATA%\PulseWin\<id>.json`. Existing plaintext files migrate when read; failed decryption or
migration remains visible and never falls back to writing plaintext. Saving preserves omitted fields
and refreshes that enabled account.

Notifications are off by default. Settings offers quota thresholds, witnessed reset alerts,
three-failure protection and a monetary balance threshold where an adapter reports a real balance.
Alert memory persists so re-opening the application does not repeatedly announce the same window.
Actual notification banners and click routing still need native acceptance.

Token Spend is also off by default. Current readers cover Claude Code, Codex, Qwen Code and Gemini
CLI local logs, plus Cursor exports, Antigravity synchronized cache, Hindsight mirrored usage and
MCode captured stream-json. These four import formats require files prepared beforehand; PulseWin
does not connect or sync them. Readers use bounded streaming, replay deduplication, file caching and cooperative cancellation.
The page has today/7-day/30-day ranges, model and agent details, daily/hourly charts, project and
session sorting/pagination, and public models.dev price estimates. Unknown prices stay unknown.
Native read-only SQLite readers now cover OpenCode, Kilo CLI and MiMo Code, including active WAL files. The original's other 43 sources, including native Antigravity CLI/IDE stores,
are explicitly unsupported until their readers are implemented. Aggregate Cursor exports do not
generate a fabricated hour chart.

General settings offers two optional global shortcuts and reads the actual Windows login-startup
state. `pulsewin.exe --json` and `--statusline` read the saved quota cache without opening the GUI,
accessing credentials or fetching providers; observations carry their age. Full original JSON and
deep-link integration compatibility remains incomplete.

---

## Requirements

| Tool | Version used | Why |
| --- | --- | --- |
| Node.js | 24.15.0 | frontend build |
| Rust | 1.98.1 stable (`x86_64-pc-windows-msvc`) | Tauri backend |
| VS 2022 Build Tools | 17.14 with "Desktop development with C++" | MSVC linker; Windows SDK 10.0.26100 |
| WebView2 Runtime | 150.0.4078.83 (preinstalled on Win 10 21H2+ / Win 11) | renders the panel |

> **Use `npm run tauri build`, not `cargo build --release`.** Building the Rust crate on its own
> produces a binary that starts but renders a blank white window, because the frontend assets are
> only embedded correctly when the Tauri CLI drives the build. This cost an hour to diagnose; do
> not repeat it.

---

## Build

```powershell
cd PulseWin
npm install

# One-time: generate the icon set the bundle expects.
npx tauri icon assets/pulse-mark.svg    # writes src-tauri/icons/*

npm run tauri dev                       # live-reloading dev build
npm run tauri build                     # release binary + NSIS + MSI installers
```

On Windows, the npm Tauri wrapper also assigns the normal Medium integrity label to the
generated application and current-version installers. This prevents an inherited **Low Mandatory
Level** label from the build directory. A low-labelled installer cannot write to normal `%TEMP%` and NSIS fails before
its welcome page with `Error writing temporary file. Make sure your temp folder is valid.`
This does not change system temp settings, workspace permissions, or require running the app
as administrator. Use `npm run repair:artifacts` after packaging with `npx tauri` directly.
The helper targets the
default `src-tauri/target/release` x64 outputs; custom target directories need a corresponding check.

The Rust side also needs the MSVC environment. If `cargo` cannot find `cl.exe`:

```powershell
& "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
```

### Behind a slow network

If rustup stalls downloading toolchains, or crates.io times out, point both at a domestic mirror:

```powershell
$env:RUSTUP_DIST_SERVER = "https://mirrors.ustc.edu.cn/rust-static"
$env:RUSTUP_UPDATE_ROOT = "https://mirrors.ustc.edu.cn/rust-static/rustup"
```

and in `~/.cargo/config.toml`:

```toml
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
```

---

## Proxy support

reqwest only honours the `HTTP_PROXY` / `HTTPS_PROXY` environment variables. Most Windows users
behind a local proxy (Clash, v2ray, a corporate MITM) configure it in Settings → Network → Proxy,
which lands in the registry and never reaches the environment.

`src-tauri/src/proxy.rs` reads `ProxyEnable` / `ProxyServer` / `ProxyOverride` from
`HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings` and configures the HTTP client
accordingly. Without it, **every** provider call times out with `client error (Connect) <-
operation timed out`. Environment variables still take precedence when set.

---

## Where credentials come from

A provider is fetched only if something on the machine says the tool is there. Three routes, in the
order a provider tries them:

1. **The environment.** `PULSEWIN_<ID>_KEY`, `PULSEWIN_<ID>_TOKEN` or `PULSEWIN_<ID>_COOKIE`, with
   the id upper-cased and `-` written as `_` — `PULSEWIN_CLAUDE_CODE_TOKEN`. The generic
   `PULSEWIN_KEY` and `PULSEWIN_TOKEN` are a fallback for any provider. An exported variable wins,
   because someone who exported one meant that one to be used.
2. **`%APPDATA%\PulseWin\<id>.json`** — the current-user encrypted credential envelope written by
   Settings. Legacy plaintext files are migrated when read.
3. **The tool's own login file**, where the tool keeps one. PulseWin reads those files and never
   writes to them. It never asks for a password.

| Tool | What PulseWin reads |
| --- | --- |
| Claude Code | `%USERPROFILE%\.claude\.credentials.json` |
| Codex | `%USERPROFILE%\.codex\auth.json` |
| Gemini CLI | `%USERPROFILE%\.gemini\oauth_creds.json` |
| GitHub Copilot | `%APPDATA%\GitHub Copilot\hosts.json`, VS Code's global storage, or the `gh` CLI's `hosts.yml` |
| Cursor | a session cookie you paste — see below |

For an expired Gemini token, PulseWin reads the OAuth client pair from the installed Gemini CLI
(standard npm installation), or `client_id` / `client_secret` in its credential JSON. You can also
set both `PULSEWIN_GEMINI_CLIENT_ID` and `PULSEWIN_GEMINI_CLIENT_SECRET`. No OAuth client secrets
are bundled in this repository; without a client pair, run Gemini CLI again to renew its login.
An existing unexpired access token continues to work without this configuration.

Every other provider documents its own rule at the top of its own file in
`src-tauri/src/providers/`, because the rule differs: some read a key, some a token, some a cookie,
and the self-hosted gateways need an address beside the key.

### Cursor needs one step by hand

Cursor keeps its session in `%APPDATA%\Cursor\User\globalStorage\state.vscdb` (SQLite), which
PulseWin cannot read. Supply the session cookie once — through Settings, or

```powershell
$env:PULSEWIN_CURSOR_COOKIE = "WorkosCursorSessionToken=user_xxx%3A%3Ajwt..."
```

or create a legacy `%APPDATA%\PulseWin\cursor.json` once (it is encrypted when read):

```json
{ "cookie": "WorkosCursorSessionToken=user_xxx%3A%3Ajwt..." }
```

### Registered providers (77)

The registry is in `src-tauri/src/providers/mod.rs`. Browser-session adapters generally require a pasted credential; native account helpers and real-account behavior must be checked individually. The previous list of 23 unported adapters is obsolete, but full authentication parity remains unfinished.

## Debugging a provider

The unit tests parse fixed fixtures and never touch the network. For live checks use the probe,
which uses the credentials actually on the machine:

```powershell
cd src-tauri
cargo run --example probe                  # every provider
cargo run --example probe -- codex         # one provider
$env:PULSEWIN_DEBUG=1; cargo run --example probe -- codex
```

`cargo run --example probe -- <unknown-id>` lists every registered id and exits without making a
request, which is the quickest way to see what this build supports.

---

## Architecture

```
PulseWin/
├─ docs/                          the screenshots above
├─ src/                           the webview — both windows, one bundle
│  ├─ main.tsx                    lazy-loads the panel or settings from ?view=
│  ├─ App.tsx                     panel shell: rail, card, hit regions, frame size
│  ├─ Settings.tsx                the settings window
│  ├─ api.ts                      typed wrappers over Tauri invoke/listen
│  ├─ types.ts                    mirrors the Rust model, incl. severity thresholds
│  ├─ errorReporter.ts            JS errors to %TEMP%\pulsewin-errors.log
│  ├─ styles.css, settings.css
│  └─ panel/
│     ├─ Rail.tsx                 the docked column of rings
│     ├─ UsageRing.tsx            one ring gauge, with the mark on its disc
│     ├─ UsageCard.tsx            the hover bubble
│     ├─ berthShape.ts            the rail's silhouette and its dock layout
│     ├─ bubbleShape.ts           the card's outline and tail
│     ├─ cardLayout.ts            the card's size budgets
│     ├─ ringGeometry.ts          arc, dash and clock geometry
│     ├─ tint.ts                  how close a limit is, and nothing else
│     ├─ panel.css
│     └─ botmark/                 BotMark.tsx, mood.ts, data.ts, geometry.ts,
│                                 tint.ts, bot-data.json
└─ src-tauri/
   ├─ examples/probe.rs           command-line provider probe
   └─ src/
      ├─ main.rs                  windows_subsystem, then run()
      ├─ lib.rs                   tray, window placement, pointer watcher, IPC commands
      ├─ settings.rs              panel.json and credential persistence
      ├─ credential_store.rs      current-user DPAPI and legacy migration
      ├─ adaptive_refresh.rs      per-provider one-shot schedule
      ├─ usage_cache.rs           last-good observations and expiry
      ├─ alerts.rs                persistent evidence-based alert decisions
      ├─ application.rs           Windows shortcuts and login startup
      ├─ token_spend.rs           local log readers and scan sessions
      ├─ usage_report.rs          read-only JSON/status-line cache output
      ├─ model.rs                 UsageWindow / ProviderUsage / Snapshot
      ├─ credentials.rs           credential discovery + JSON path helpers
      ├─ proxy.rs                 Windows system-proxy discovery
      └─ providers/
         ├─ mod.rs                Provider trait, registry, scheduler, shared helpers
         └─ …                     provider adapters and shared helpers
```

Adding a provider is one file: implement `Provider`, add it to `registry()` in `providers/mod.rs`.
The scheduler runs every provider concurrently with a 20-second timeout each, so one dead endpoint
degrades to a single error card instead of freezing the panel. The self-hosted gateways get a second
HTTP client with redirects turned **off**: the key on those requests is set by hand, and a hand-set
header rides a redirect to whatever host it names, so a redirect has to arrive as a refusal rather
than be followed.

---

## Tests

```powershell
cd src-tauri
cargo test --lib
cd ..
npx vitest run
npm run test:e2e
# After a release build, with an isolated profile and no real accounts enabled:
npm run test:native
npm run test:cli
```

`cargo test` needs the MSVC environment from [Build](#build) in the shell that runs it; without it
the linker never starts.

Unit suites run offline against fixed fixtures. Rust tests must use an isolated application/profile
directory; see the native test harness for profile isolation. Do not exercise credential migration
against personal credentials during tests. The final run counts and native acceptance boundaries
are recorded in [the coverage inventory](docs/readme-coverage.md).

- **The Rust side** parses each registered provider's reply — JSON, a string envelope, a
  console's prose, a CLI's own output — and checks the rules each one carries: a limit of zero is
  not a denominator, a percentage nobody reported is never invented, an unreported balance is not
  zero, a spend past its ceiling clamps to a full ring rather than past it. It also covers the
  shared machinery: proxy parsing, the JSON path helpers that read a credential file, the dock edge
  round-tripping through `panel.json`, and the gateway address checks.
- **The frontend** covers the geometry the panel is built from — the berth
  shape, the bubble outline, the card budgets, the ring's arcs — the tint tables, the model's
  severity thresholds, and BotMark's data: that all 18 bodies resolve to a ring, and that every
  state names expressions that exist. It also exercises animation timing and lifecycle evidence,
  shortcuts, Token Spend filtering and unknown-cost behavior. Browser E2E uses mocked IPC to
  check complete settings and assistant flows; native smoke separately checks real WebView2 IPC.

Offline suites do not talk to real services. A separate ignored OpenCode Go live check was explicitly run for 0.1.3 and confirmed the entitlement refusal; it did not validate successful quota retrieval. No fixture test can say whether a provider's
endpoint or field names are still what the service sends this week. The probe above is that check.

---

## Licence

The original Pulse is Apache 2.0. This port follows the same licence; see the upstream repository
for the original copyright notice. The bot mark's character data is the original's own asset and
carries its notice alongside it in `src/panel/botmark/LICENSE-Pulse.txt`.


官方中文 README 对照与功能缺口：见 [功能与 UI 覆盖表](docs/readme-coverage.md)。

### 0.1.3 account details and OpenCode Go

Per-account detailed cards are opt-in and require Token Spend permission before reading local history. They show today/7-day/31-day totals, daily bars, model share and measured cache-hit rate where available. Public API-price estimates are not subscription billing; partial or untimed records do not produce a window budget. Account histories never share cached readings.

OpenCode Go now identifies the actual credential source and distinguishes invalid-key, subscription-entitlement and rate-limit responses. The connection button opens that account's settings. A read-only live request with this machine's CLI key returned **403 EntitlementError: OpenCode Go subscription required**. Use the key associated with the subscribed account/workspace; successful live quota retrieval remains unverified. Fixtures alone do not establish service availability.
