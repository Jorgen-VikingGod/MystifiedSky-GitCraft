# GitCraft code map

GitCraft is a Tauri desktop app. React renders the interface, Tauri connects the renderer to the Rust backend, and Rust invokes the configured Git executable and handles native platform integration. The Rust backend lives in [crates/gitcraft-core](../crates/gitcraft-core), which has no Tauri dependency; [src/main.rs](../src/main.rs) is the Tauri shell around it.

| Area | Start here | Where to verify it |
| --- | --- | --- |
| App startup, windows, and command registration | [src/main.rs](../src/main.rs), [ui/src/App.tsx](../ui/src/App.tsx), [capabilities/default.json](../capabilities/default.json) | [Packaged desktop checks](../TESTING.md#desktop-smoke-test) |
| Repository reads and changes | [crates/gitcraft-core/src/git.rs](../crates/gitcraft-core/src/git.rs), [crates/gitcraft-core/src/actions.rs](../crates/gitcraft-core/src/actions.rs), [crates/gitcraft-core/src/files.rs](../crates/gitcraft-core/src/files.rs), [crates/gitcraft-core/src/read_jobs.rs](../crates/gitcraft-core/src/read_jobs.rs) | [Native regression fixtures](../crates/gitcraft-core/src/regressions.rs) |
| Commit graph and selection | [crates/gitcraft-core/src/graph.rs](../crates/gitcraft-core/src/graph.rs), [ui/src/components/GraphView.tsx](../ui/src/components/GraphView.tsx), [ui/src/lib/graphnav.ts](../ui/src/lib/graphnav.ts) | [Native layout tests](../crates/gitcraft-core/src/graph.rs), [renderer navigation tests](../ui/src/lib/graphnav.test.ts), [browser checks](../ui/tests/browser/verify.tsx) |
| Renderer commands, state, and settings | [ui/src/lib/api.ts](../ui/src/lib/api.ts), [ui/src/lib/repo.ts](../ui/src/lib/repo.ts), [ui/src/lib/store.ts](../ui/src/lib/store.ts), [crates/gitcraft-core/src/settings_file.rs](../crates/gitcraft-core/src/settings_file.rs) | [Repository tests](../ui/src/lib/repo.test.ts), [persistence tests](../ui/src/lib/persistence.test.ts) |
| Repository watching, terminals, and native launchers | [crates/gitcraft-core/src/watch.rs](../crates/gitcraft-core/src/watch.rs), [crates/gitcraft-core/src/terminal.rs](../crates/gitcraft-core/src/terminal.rs), [crates/gitcraft-core/src/external.rs](../crates/gitcraft-core/src/external.rs) | [Native watcher and terminal tests](../TESTING.md#build-and-run-the-checks), [desktop checks](../TESTING.md#desktop-smoke-test) |
| Hosting and review | [crates/gitcraft-core/src/hosting.rs](../crates/gitcraft-core/src/hosting.rs), [crates/gitcraft-core/src/hosting_api.rs](../crates/gitcraft-core/src/hosting_api.rs), [ui/src/components/PullReview.tsx](../ui/src/components/PullReview.tsx) | [Hosting tests](../crates/gitcraft-core/src/hosting.rs), [browser checks](../ui/tests/browser/verify.tsx) |
| Testing, packaging, and updates | [test.yml](../.github/workflows/test.yml), [release.yml](../.github/workflows/release.yml), [scripts/release-assets.mjs](../scripts/release-assets.mjs), [ui/src/lib/updates.ts](../ui/src/lib/updates.ts) | [Testing guide](../TESTING.md), [release guide](../RELEASING.md) |

## Trace a Git action

1. Follow the UI handler to `act` in [repo.ts](../ui/src/lib/repo.ts).
2. Follow `api.run` in [api.ts](../ui/src/lib/api.ts) into the Rust command `run_action` in [main.rs](../src/main.rs).
3. Find the action and its repository checks in [actions.rs](../crates/gitcraft-core/src/actions.rs). Put Git state assertions in the native regression fixture and renderer assertions near the affected UI.
4. Run the app from the [testing guide](../TESTING.md) if the change touches native platform behavior.

For example, `feature_stash_modes_keep_the_live_index_and_unselected_work` uses a disposable repository to verify what a stash leaves staged and what stays in the working tree. Run it with `cargo test --locked feature_stash_modes_keep_the_live_index_and_unselected_work -- --test-threads=1`. Browse the fixture in [regressions.rs](../crates/gitcraft-core/src/regressions.rs).
