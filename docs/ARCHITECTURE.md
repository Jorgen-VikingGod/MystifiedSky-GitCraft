# GitCraft code map

GitCraft is a native Rust desktop app. [crates/gitcraft](../crates/gitcraft) draws the interface with egui and handles windows, launches, updates and packaging. [crates/gitcraft-core](../crates/gitcraft-core) is the back end: it runs the configured Git executable, reads repositories, talks to hosting services and AI providers, and runs terminals, independent of the interface. Shared Craft libraries (craft-window, craft-menu, craft-keymap, craft-dialogs, craft-ipc and others) provide the window frame, menus, key bindings, file dialogs and the single-instance hand-off.

| Area | Start here | Where to verify it |
| --- | --- | --- |
| Startup, launches and windows | [main.rs](../crates/gitcraft/src/main.rs), [instance.rs](../crates/gitcraft/src/instance.rs), [macos.rs](../crates/gitcraft/src/macos.rs), [app/mod.rs](../crates/gitcraft/src/app/mod.rs) | [Instance tests](../crates/gitcraft/src/instance.rs), [desktop smoke test](../TESTING.md#desktop-smoke-test) |
| Repository reads and changes | [git.rs](../crates/gitcraft-core/src/git.rs), [actions.rs](../crates/gitcraft-core/src/actions.rs), [files.rs](../crates/gitcraft-core/src/files.rs), [session.rs](../crates/gitcraft-core/src/session.rs), [read_jobs.rs](../crates/gitcraft-core/src/read_jobs.rs) | [Regression fixtures](../crates/gitcraft-core/src/regressions.rs) |
| A repository tab: loading, refreshing and the action queue | [app/repo.rs](../crates/gitcraft/src/app/repo.rs), [app/screen.rs](../crates/gitcraft/src/app/screen.rs) | [Interface tests](../crates/gitcraft/src/app/mod.rs) |
| Commit graph and selection | [graph.rs](../crates/gitcraft-core/src/graph.rs), [app/graph.rs](../crates/gitcraft/src/app/graph.rs) | [Layout tests](../crates/gitcraft-core/src/graph.rs), `--screenshot` |
| Panels, diffs, editor, history and merge tool | [app/left.rs](../crates/gitcraft/src/app/left.rs), [app/right.rs](../crates/gitcraft/src/app/right.rs), [app/diff.rs](../crates/gitcraft/src/app/diff.rs), [app/history.rs](../crates/gitcraft/src/app/history.rs), [app/rebase.rs](../crates/gitcraft/src/app/rebase.rs), [app/conflict.rs](../crates/gitcraft/src/app/conflict.rs) | `--screenshot shot.png --shot-view diff`, `history`, `rebase`, `conflict` |
| Commands, menus, keys and dialogs | [app/keys.rs](../crates/gitcraft/src/app/keys.rs), [app/menu.rs](../crates/gitcraft/src/app/menu.rs), [app/ctxmenu.rs](../crates/gitcraft/src/app/ctxmenu.rs), [app/dialogs.rs](../crates/gitcraft/src/app/dialogs.rs) | [Interface tests](../crates/gitcraft/src/app/mod.rs) |
| Preferences and saved state | [settings.rs](../crates/gitcraft/src/settings.rs), [app/prefs_ui.rs](../crates/gitcraft/src/app/prefs_ui.rs), [settings_file.rs](../crates/gitcraft-core/src/settings_file.rs) | `--screenshot shot.png --shot-view prefs` |
| Watching, terminals and launchers | [watch.rs](../crates/gitcraft-core/src/watch.rs), [terminal.rs](../crates/gitcraft-core/src/terminal.rs), [app/terminal.rs](../crates/gitcraft/src/app/terminal.rs), [external.rs](../crates/gitcraft-core/src/external.rs) | [Testing guide](../TESTING.md#build-and-run-the-checks) |
| Hosting and review | [hosting.rs](../crates/gitcraft-core/src/hosting.rs), [hosting_api.rs](../crates/gitcraft-core/src/hosting_api.rs), [app/hosting.rs](../crates/gitcraft/src/app/hosting.rs), [app/markdown.rs](../crates/gitcraft/src/app/markdown.rs) | [Hosting tests](../crates/gitcraft-core/src/hosting.rs), [content policy](hosted-content-policy.md) |
| Updates, packaging and releases | [updates.rs](../crates/gitcraft-core/src/updates.rs), [app/updater.rs](../crates/gitcraft/src/app/updater.rs), the packager settings in [Cargo.toml](../crates/gitcraft/Cargo.toml), [release.yml](../.github/workflows/release.yml), [release-assets.mjs](../scripts/release-assets.mjs) | [Update tests](../crates/gitcraft-core/src/updates.rs), [release guide](../RELEASING.md) |

## Trace a Git action

1. Follow the menu item, key or button to `App::act` in [app/repo.rs](../crates/gitcraft/src/app/repo.rs), which queues the action for the tab.
2. The queue runs it with `Engine::perform` in [actions.rs](../crates/gitcraft-core/src/actions.rs) away from the window, then `acted` refreshes the tab.
3. Put Git state assertions in the regression fixture and interface assertions near the affected screen.
4. Run the app from the [testing guide](../TESTING.md) if the change touches platform behavior.

For example, `feature_stash_modes_keep_the_live_index_and_unselected_work` uses a disposable repository to verify what a stash leaves staged and what stays in the working tree. Run it with `cargo test --locked -p gitcraft-core feature_stash_modes_keep_the_live_index_and_unselected_work -- --test-threads=1`. Browse the fixture in [regressions.rs](../crates/gitcraft-core/src/regressions.rs).
