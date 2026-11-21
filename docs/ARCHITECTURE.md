# GitCraft code map

GitCraft is a Tauri desktop app. React renders the interface, Tauri connects the renderer to the Rust backend, and Rust invokes the configured Git executable and handles native platform integration.

| Area | Start here | Where to verify it |
| --- | --- | --- |
| App startup, windows, and command registration | [src/main.rs](../src/main.rs), [ui/src/App.tsx](../ui/src/App.tsx), [capabilities/default.json](../capabilities/default.json) | [Packaged desktop checks](../TESTING.md#desktop-smoke-test) |
| Repository reads and changes | [src/git.rs](../src/git.rs), [src/actions.rs](../src/actions.rs), [src/files.rs](../src/files.rs), [src/read_jobs.rs](../src/read_jobs.rs) | [Native regression fixtures](../src/regressions.rs) |
| Commit graph and selection | [src/graph.rs](../src/graph.rs), [ui/src/components/GraphView.tsx](../ui/src/components/GraphView.tsx), [ui/src/lib/graphnav.ts](../ui/src/lib/graphnav.ts) | [Native layout tests](../src/graph.rs), [renderer navigation tests](../ui/src/lib/graphnav.test.ts), [browser checks](../ui/tests/browser/verify.tsx) |
| Renderer commands, state, and settings | [ui/src/lib/api.ts](../ui/src/lib/api.ts), [ui/src/lib/repo.ts](../ui/src/lib/repo.ts), [ui/src/lib/store.ts](../ui/src/lib/store.ts), [src/settings_file.rs](../src/settings_file.rs) | [Repository tests](../ui/src/lib/repo.test.ts), [persistence tests](../ui/src/lib/persistence.test.ts) |
| Repository watching, terminals, and native launchers | [src/watch.rs](../src/watch.rs), [src/terminal.rs](../src/terminal.rs), [src/external.rs](../src/external.rs) | [Native watcher and terminal tests](../TESTING.md#build-and-run-the-checks), [desktop checks](../TESTING.md#desktop-smoke-test) |
| Hosting and review | [src/hosting.rs](../src/hosting.rs), [src/hosting_api.rs](../src/hosting_api.rs), [ui/src/components/PullReview.tsx](../ui/src/components/PullReview.tsx) | [Hosting tests](../src/hosting.rs), [browser checks](../ui/tests/browser/verify.tsx) |
| Testing, packaging, and updates | [test.yml](../.github/workflows/test.yml), [release.yml](../.github/workflows/release.yml), [scripts/release-assets.mjs](../scripts/release-assets.mjs), [ui/src/lib/updates.ts](../ui/src/lib/updates.ts) | [Testing guide](../TESTING.md), [release guide](../RELEASING.md) |

## Trace a Git action

1. Follow the UI handler to `act` in [repo.ts](../ui/src/lib/repo.ts).
2. Follow `api.run` in [api.ts](../ui/src/lib/api.ts) into the Rust command `run_action` in [main.rs](../src/main.rs).
3. Find the action and its repository checks in [actions.rs](../src/actions.rs). Put Git state assertions in the native regression fixture and renderer assertions near the affected UI.
4. Run the app from the [testing guide](../TESTING.md) if the change touches native platform behavior.

For example, `feature_stash_modes_keep_the_live_index_and_unselected_work` uses a disposable repository to verify what a stash leaves staged and what stays in the working tree. Run it with `cargo test --locked feature_stash_modes_keep_the_live_index_and_unselected_work -- --test-threads=1`. Browse the fixture in [regressions.rs](../src/regressions.rs).
