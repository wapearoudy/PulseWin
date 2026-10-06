# Codex quota freshness — 0.1.13

The reported difference was PulseWin 37% versus Codex 41%. The user confirmed
that PulseWin marked its value as an old reading. The stored successful reading
was fetched on 2026-10-06 at 11:15 China Standard Time; the installed process
started at 11:21. At the diagnostic checkpoint the same account's live usage
endpoint, Codex app limits reader and a fresh native PulseWin process returned
41%. The CLI login matched the app account; no separate saved Codex login or
environment override was present. This observation establishes stale data,
rather than a 37-to-41 percentage conversion error.

The original installed process did not have a debugging endpoint, so its exact
request failure was not recovered. We independently reproduced a relevant
long-running connection defect: clients captured the WinINET proxy at startup,
and external changes did not update them unless an application preference was
saved. A deterministic test switches two local proxy servers with identical
application preferences, exercises real HTTP requests and verifies the old
client is cancelled, the new route is used, and an unchanged policy preserves
the connection pool. No personal Windows registry settings are changed by it.
The WinINET settings key was last written at 11:23, after the installed process
started, supporting the route-change hypothesis without proving which registry
value changed.

Refresh now compares the captured effective proxy policy before collecting
accounts. Both normal and redirect-disabled clients use the same captured
policy. Manual precedence, loopback bypass and tolerance of malformed external
proxy values are preserved. A concurrent preference save still cancels obsolete
requests and rejects an obsolete result through the existing generation guard.

Cached values retain their successful observation time. Their assistant cards
now show the request failure and a scoped retry action, in addition to the old
reading label. Card frame and hit-region budgets include these controls. A
browser regression checks the 37% cached state, retry target, control bounds,
and transition to a fresh 41% state without a lingering warning. Browser
fixtures use synthetic values; the private live native verification is recorded
separately in the release evidence.
The final release native test retained the real dated 37% cache under a forced
connection failure, used its retry button, restored the saved system route in
the same process and read a fresh 41% at 20:27. The failure UI disappeared and a
subsequent scoped refresh remained successful. Detailed-card pricing footnotes
are independent of the old-reading footnote and remain visible normally.

A separate parser regression showed `used_percent: 0.5` being interpreted as
50% and `used_percent: 1` as 100%. Codex percentage points now remain percentage
points at all values, for both five-hour and weekly windows. The shared fraction
normalizer used by other providers was not changed.

Quota is fetched on a schedule or on request; future values can differ if the
account is used between observations. Authentication refusals still require
repairing the login. This patch does not claim automatic token renewal or full
parity with all original Pulse functionality.

Read-only live checks do not persist access tokens, account identifiers, email
addresses or transcripts in committed evidence. They isolate APPDATA and compare
personal credential, preferences and quota-cache bytes before and after.
