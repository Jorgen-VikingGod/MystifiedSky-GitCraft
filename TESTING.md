# Testing GitCraft across desktop platforms

Use automated tests on each OS, then test the installed app in a real desktop session. The release workflow builds packages for all three platforms; the test workflow runs the suites on each OS. Neither replaces interactive validation.

## Ubuntu on a Windows development machine

For quick Linux development, use a separate Ubuntu 24.04 WSL 2 distribution with WSLg. From an administrator PowerShell terminal:

```powershell
wsl --install -d Ubuntu-24.04
```

Complete Ubuntu's first-run account setup. Follow Microsoft's [WSL GUI setup](https://learn.microsoft.com/en-us/windows/wsl/tutorials/gui-apps) if Linux windows do not open. Keep the checkout inside the Linux filesystem, such as `~/src/GitCraft`, rather than `/mnt/c`, for Linux filesystem behavior and file-watch performance. Install tools within Ubuntu; Windows installations of Rust do not replace Linux installations.

In Ubuntu:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config curl git \
  libgtk-3-dev libxkbcommon-dev libwayland-dev libdbus-1-dev
mkdir -p ~/src
cd ~/src
git clone https://github.com/MystifiedSky/GitCraft.git
cd GitCraft
```

Use Git 2.38 or later. Install stable Rust using the [official Rust installer](https://www.rust-lang.org/tools/install), and Node.js 22.12 or later if you work on the release scripts. Reopen the terminal, verify `cargo --version` and `git --version`, then run the commands below from the cloned repository root. GitCraft draws with the GPU through Vulkan, Metal, DirectX 12 or OpenGL; under WSLg it uses OpenGL.

WSLg is useful for everyday development. Also test packaged releases in a full Ubuntu Desktop VM or physical Linux desktop to check menus, file dialogs, notifications, credential storage, terminal behavior, and desktop installation. Use a disposable VM with a snapshot for package and updater testing. CI currently uses Ubuntu 22.04 as the Linux release build baseline; testing 24.04 additionally checks a newer desktop environment.

## macOS

Use a Mac, a remote Mac you can access interactively, or a macOS VM hosted on Apple hardware. GitHub's macOS runners provide automated build/test feedback; an interactive Mac session is still needed for desktop checks.

Install Apple's command-line developer tools:

```bash
xcode-select --install
```

Install stable Rust, clone the repository, and use the same build/test commands below. Verify Apple silicon and Intel separately before declaring both fully validated. The release pipeline compiles a universal app, while the test job runs on the runner's native architecture.

## Build and run the checks

From the directory containing `Cargo.toml` on each OS:

```bash
cargo test --locked --workspace -- --test-threads=1
node --test scripts/changelog-release.test.mjs
cargo run --locked
```

The app is in [crates/gitcraft](crates/gitcraft) and its back end in [crates/gitcraft-core](crates/gitcraft-core). Take a picture of the window without touching your preferences with `cargo run --locked -- --screenshot shot.png --theme light`; `--shot-view` shows a particular view first (see `--help`).

The Rust suite uses disposable repositories. Run it serially because some tests change process-wide runtime preferences. Live AI tests are ignored by default and require explicit local setup.

Only one GitCraft window runs per user: a second launch hands its folder or `gitcraft://clone?url=` link to the running one and exits. To try that without touching your own preferences, start both launches with `HOME` (on Windows, `LOCALAPPDATA`) pointing at an empty folder, for example `HOME=/tmp/gc cargo run -- repo-a`, then `HOME=/tmp/gc cargo run -- repo-b`: the second command exits at once and `repo-b` opens as a tab in the first window. `--new-window` opens a separate window instead.

To package the app locally, install the packager once with `cargo install cargo-packager --locked --version 0.11.8`, then run `cargo build --release --locked -p gitcraft` and `cargo packager --release -p gitcraft --formats <formats>`: `nsis` or `wix` on Windows, `app` or `dmg` on macOS, `deb` or `appimage` on Linux. For an RPM, install `cargo install cargo-generate-rpm --locked --version 0.21.1` and run `cargo generate-rpm -p crates/gitcraft`. Published updater artifacts are built separately by the signed release pipeline; a local build alone does not verify the published update flow.

## Choose the right checks

| Change | Start with | Evidence to include in the PR |
| --- | --- | --- |
| Git reads or writes | An existing Rust fixture near the operation or in [crates/gitcraft-core/src/regressions.rs](crates/gitcraft-core/src/regressions.rs) | The starting repo state, the action, and the resulting refs, staged changes, or working files that matter to the fix |
| Interface logic or layout | The nearest test in [crates/gitcraft](crates/gitcraft), and a `--screenshot` picture in the dark and light themes | The test and outcome, with before and after screenshots for visible changes |
| Native windows, file dialogs, launchers, credential storage, or updater behavior | A [desktop smoke test](#desktop-smoke-test) on the affected platform | The OS, architecture, package or build tested, and repeatable steps |
| Documentation or translations | Follow the steps or open the changed screen in the affected language | The command, screen, or links you checked |

For a bug fix, reproduce the original failure when practical and keep the fixture as small as possible. Prefer a disposable repository with synthetic files and a local remote. For example, the [stash regression](crates/gitcraft-core/src/regressions.rs) checks the live index and unselected work after a stash. If you cannot reproduce a platform-specific report, state the gap in the PR so a reviewer can verify it.

## Desktop smoke test

Use a disposable repository with a local bare remote. Record the app version, OS version, architecture, package format, and results.

- Install, launch, close, and reopen the packaged app. Launch it again with a folder while it runs: the folder opens as a tab.
- Open a `gitcraft://clone?url=` link and "Open in GitCraft" from Explorer or the Finder.
- Open, clone, and initialize repositories, including paths with spaces and Unicode.
- Stage a file, hunk, and line; commit; amend; verify the index and working tree.
- Create and switch branches; merge, resolve a conflict, rebase, and exercise guarded undo/redo.
- Stash and restore work; fetch, pull, and push against the local remote.
- Verify graph scrolling, author images, diffs, file watching, and multiple repository tabs.
- Open the terminal, external editor, and native file dialogs. Verify preferences persist.
- Check credential storage, notifications, and optional AI integrations in a real signed-in desktop session.
- Install a previous published version, check for updates, restart into the new version, and verify the displayed version. Test Linux package upgrades separately from AppImage updates.

Report failures with reproduction steps and sanitized logs. Keep platform validation marked as in progress until the packaged desktop flows have been exercised.
