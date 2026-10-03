# DeepSeek Harness Token Spend

The `dsh` reader is part of the existing opt-in Token Spend scan. It discovers
only `$DSH_HOME/sessions`, falling back to `~/.dsh/sessions`. Current Harness
desktop and CLI share the same home. It never accesses credentials, attachments,
profiles, backups or stored conversation titles.

Supported physical files: `session.jsonl`, `session.v<N>.jsonl`, and their
`.zstd` counterparts. Logical formats 0–4 are supported; later formats report
an explicit unsupported-version error. Compression is detected by magic bytes,
not the filename. The application bundles Zstandard; no Python, command-line
decoder or local DLL installation is needed.

Provider-reported input is uncached input. Cache read/write are separate buckets;
reasoning is already included in output. Missing cache fields mean zero;
input/output are required. Invalid counters, inconsistent totals, missing call
coordinates, model or event timestamp are reported as incomplete reads.
Context occupancy, shadowed-token estimates and message lengths are not spend.

The last usage sample embedded in an assistant attempt is its settlement. A later
settlement for the same turn/step replaces the prior sample. A matching
`llm/retry-started` closes that replacement slot so a billed failed attempt and
its retry remain separate. Compaction summaries with measured usage count their
own model call. Model resolution prefers the provider response model, message
source, explicit compaction model, then the current request header.

Old `seedLength` and the last tagged `session/end-seed` inherited marker exclude
fork history. Ordinary restore markers do not erase earlier local calls. A
seeded log without a valid inherited cut is rejected. Shared message/compaction
identities and call timestamps deduplicate plain/compressed/migrated copies,
including on cache hits. A growing file invalidates its normalized cache.

Limits: 64 MiB physical file, 512 MiB streamed decoded data, 8 MiB per record,
64 MiB decoder window, and the existing global file/record limits. Oversized
records are skipped with a warning. Torn trailing frames preserve complete
records with an incomplete-read notice; corrupted files are rejected. Warned or
concurrently changed files are never cached. Every streaming chunk checks
cancellation. Only normalized counters and metadata enter the cache.

Prices use the existing public model table and are estimates, not subscription
charges or account balances. Unknown models remain unpriced. Harness calls are
not attributed to a specific API credential or quota account.

Reference implementations, audited at Harness commit
`5badb15009ae1756c3afe0ae0cef1faafc290ccc`:

- [Token usage settlement fold](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/llm/token-meter/src/usage-projection.ts)
- [Session format and inherited markers](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/core/session/src/types.ts)
- [Shared home resolution](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/packages/util/home-paths/src/index.ts)
- [Desktop shared home](https://github.com/deepseek-ai/deepseek-harness/blob/5badb15009ae1756c3afe0ae0cef1faafc290ccc/apps/desktop/src/paths.ts)
- Pulse `Sources/Pulse/Usage/Readers/DSHUsageReader.swift` and `DSHZstdDecoder.swift`.

Validation includes synthetic reader regressions, summary/drilldown browser
checks, a real release WebView2/IPC smoke with synthetic compressed sessions,
and an explicitly opted-in read-only check of local compressed Harness logs.
Independent Python/Zstandard and Rust totals matched, all input hashes remained
unchanged, and the second scan reused every file cache. Real session contents,
paths and usage totals are not published.

Run synthetic native acceptance with `node scripts/harness-smoke.mjs` after
building the release binary. Local-user verification is intentionally ignored
in normal tests; it requires explicit `PULSEWIN_DSH_VERIFY_HOME` and private
`PULSEWIN_DSH_VERIFY_RESULT` paths, then
`cargo test --manifest-path src-tauri/Cargo.toml verify_local_harness_read_only --lib -- --ignored`.
