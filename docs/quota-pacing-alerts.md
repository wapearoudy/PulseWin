# Quota alerts compared with cycle time — 0.1.14

The user requested that a quota used at the same rate as elapsed time should
not warn, and chose to retain the configured usage percentage as a minimum
notification threshold. The rule for an approaching notification is therefore:

`used >= minimum && used > 100 * (1 - seconds_until_reset / cycle_seconds)`

Both conditions must hold. With a 75% minimum, 75% used at 75% elapsed stays
quiet; 80% used at 75% elapsed warns. Fast consumption at 74.9% remains below
that notification minimum. Five-hour, weekly and model-scoped windows are
evaluated independently, with independent account and window identities.

Only a finite positive duration and a valid future reset inside that duration
are evidence of a cycle. Duration is never inferred from a display label.
Balance and purchased top-up percentages are excluded. Unknown cycle metadata
does not fall back to a percentage-only approaching notification. Explicit
restrictions and genuine exhaustion remain distinct urgent states. Currency
low-balance and repeated-fetch-failure notifications retain their own rules.

Fresh observations only can prove quota alerts. Cached, failed, future-dated
and over-age readings do not prove fast consumption. An expired cycle is not
announced. Within one cycle, threshold/exhaustion notifications remain deduped;
a newly witnessed reset can rearm them. Saved legacy fixed-percentage warnings
do not suppress the first genuinely paced warning, and do not by themselves
prove that a reset notification is wanted. Legacy exhaustion stays deduped.

The default ring/row colours and native tray warning colour compare the same
cycle progress. On-schedule values remain normal. Red still requires the
configured visual threshold; amber is earlier visual caution for fast
consumption above 50%, not a notification. Chosen account colours retain their
existing role; exhaustion still wins. A collapsed rail checks every window,
including a faster secondary window when the fullest main window is on pace.
Displayed percentages and ring fill still show actual usage or the user's
remaining-mode choice. New observations refresh the display clock immediately
so a stale UI clock cannot turn an equal-progress example into a warning.

The notification page explains the two conditions and preserves saved minimum
settings. Ring settings also explain the visual warning rule. Windows toast
delivery itself is unchanged.

Acceptance includes fixed-time Rust alert tests, legacy memory round-trips,
actual tray pixel colours, frontend cycle tests and browser checks for both
rings, hover cards, the collapsed rail, tray overview and per-account detail.
The native `scripts/pacing-smoke.mjs` checks real settings persistence and
WebView2/IPC with explicitly synthetic UI-only snapshots. It keeps backend
accounts disabled and sends no actual OS notifications. Real toast delivery,
physical pointer and multi-DPI acceptance are not claimed by this test.
