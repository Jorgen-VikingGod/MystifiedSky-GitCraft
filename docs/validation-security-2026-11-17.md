# Validation security and session-index fixes — 2026-11-17

Based on app commit `31f0632`. Six open static-analysis findings are addressed:

| Finding | Rule | Change |
| --- | --- | --- |
| AaHsofocUqRQE0uOeM0N | githubactions:S7637 | Pin browser setup to the full commit SHA behind the annotated v1 tag, `c785b87e244131f27c9f19c1a33e2ead956ab7ce`. Both tag and peeled commit were checked against the upstream remote. |
| AaHsofoUUqRQE0uOeM0L | jssecurity:S8707 | Validate report tags before filesystem work; reject separators, traversal, drives, shell/control characters and overlong names. Canonical containment also rejects target/artifact junctions outside the checkout before creating verification files. |
| AaHsofoUUqRQE0uOeM0M | jssecurity:S8701 | Replace arbitrary executable CLI paths with a fixed selection of checkout-local release/debug builds. Canonical containment rejects builds linked outside the checkout. |
| AaHsofoUUqRQE0uOeM0J | javascript:S4036 | Resolve Git from explicit Program Files locations and use its full path for fixture Git and native runtime settings. The app's search path contains only the selected Git directory and the Windows system directory. |
| AaHsofoUUqRQE0uOeM0K | javascript:S4036 | Invoke the Windows system PowerShell by absolute path. The cancellation helper also uses an explicit system ping path. |
| AaHsofmgUqRQE0uOeM0I | typescript:S6671 | Reject the unreadable-history fixture with an Error object. Its actual component error/retry/empty-state regression passes. |

## macOS CI failure

[Release run 44228362581](https://github.com/MystifiedSky/GitCraft/actions/runs/44228362581)
failed in `agents::tests::catalog_index_avoids_idle_probes_and_tracks_append_create_remove_and_overflow`
at the first modified-file wait, `src/agents.rs:336`. The job reported 186 passed,
one failed and three ignored tests.

The catalog watched and indexed the caller's lexical root, then rejected event
paths outside that prefix. Native macOS notifications use resolved paths; `/var`
and `/private/var` can identify the same directory with different prefixes. This
explains an ignored update followed by the five-second test timeout.

The index now resolves its root before both registration and discovery. Create,
modify and delete paths therefore use the same identity. Delete events do not
require canonicalizing a file that no longer exists. Missing directories retain
the existing registration retry behavior. The recovery scan and idle-probe
budgets remain unchanged.

A deterministic regression starts with an aliased root and delivers resolved
modify/delete paths through the production event-path handler. Both updates
apply without a recovery scan. The native event fixture also gives each stage
its own bounded wait and a descriptive failure message, rather than sharing one
five-second deadline across all operations. No platform-specific skip was added.

## Verification

- Session-index native tests: 5 passed, including the new canonical-identity regression and the original 4,200-file native watcher test. Idle refreshes still perform zero probes.
- Script tests: 17 passed, including executable injection/traversal rejection, trailing newlines, sibling-prefix containment, outside junctions and a poisoned Windows PATH.
- Browser component suite: 57 passed, zero captured renderer exceptions.
- Windows desktop harness: all 9 checks passed using the existing release executable, including terminal sessions, watchers, cancellation, editor workers, CSP and actual local/UNC Explorer selection. This validates the hardened script; the new Rust catalog code is exercised by the native tests above.
- TypeScript and production renderer build passed. Existing editor chunk-size warnings remain.
- `git diff --check` passed.

Logs and desktop reports are under `target/verification/` with `sonar-fix` in
their names. The path-boundary script suite now runs in the three-platform CI
matrix. A fresh macOS runner execution and hosted analysis are still needed to
confirm the remote jobs after publication. These are the local verification
results recorded before the commit; remote findings were not manually closed.
