# Contributing to GitCraft

Thanks for helping improve GitCraft. Small, focused changes are easiest to review.

For usage questions or early feedback on a feature idea, [join the GitCraft Discord](https://discord.gg/BsUgBu65YG).

## Report a bug or suggest a feature

Open an [issue](https://github.com/MystifiedSky/GitCraft/issues) with the behavior you expected, what happened, your GitCraft version, operating system, and steps to reproduce it. Use a disposable repository when sharing a reproduction. Remove credentials, private repository URLs, and personal data from logs and screenshots.

Report security vulnerabilities privately using the process in [SECURITY.md](SECURITY.md).

## Pick a change

Choose a [good first issue](https://github.com/MystifiedSky/GitCraft/labels/good%20first%20issue) or propose a focused improvement. Docs, translations, and reproducible test cases all help. Comment on the issue when you start so others can coordinate. Open an issue before investing in a large feature or architectural change. See the [roadmap](docs/ROADMAP.md) for current priorities.

## Build and check a change

Install Git, stable Rust, and Node.js 22.12 or later. On Windows, also install Visual Studio Build Tools with the **Desktop development with C++** workload, a Windows SDK, and the Microsoft Edge WebView2 Runtime. See [TESTING.md](TESTING.md) for Linux and macOS prerequisites. From the directory containing `Cargo.toml`:

```powershell
npm --prefix ui ci --ignore-scripts
npm --prefix ui run build
npm --prefix ui test
cargo test --locked -- --test-threads=1
node --test scripts/changelog-release.test.mjs
```

Run the app with `cargo run --locked` and exercise the affected behavior in a disposable repository. For Git operations, check the resulting branch, index, and working tree as well as the visible result.

See [TESTING.md](TESTING.md) for Ubuntu/WSL and macOS setup and the desktop smoke-test checklist. Pull requests run the test workflow on Windows, Ubuntu, and macOS.

## Submit a pull request

1. Fork GitCraft and create a new feature branch from the current `main` in your fork.
2. Push the branch to your fork and open a pull request into `MystifiedSky/GitCraft:main`. Use a draft while you want early feedback or are still testing.
3. Link the issue, complete the PR template, and mark it ready for review. Maintainers check that Windows, macOS, Linux, and browser CI passed before merging.

- Explain the problem, the resulting behavior, and the checks you ran.
- Include screenshots for visible changes, using sample data.
- Add a regression test when it helps verify a behavior change.
- Document notable changes under **Unreleased** in [CHANGELOG.md](CHANGELOG.md). Use the existing categories and leave published releases and comparison links alone.
- Keep dependency lockfiles in sync and explain dependency changes.

Contributions are distributed under the project's [GPL-3.0-only license](LICENSE). For third-party code, icons, fonts, or other assets, include the source and license and retain the original notices. Use original work, public specifications, or suitably licensed material.

## Build from source and package

From the directory containing `Cargo.toml`, install the frontend dependencies and launch the desktop app:

```powershell
npm --prefix ui ci
cargo run --locked
```

Use **Open a repository** for an existing checkout, or open the sample repository to explore the graph and file panel. The Rust build runs the frontend build after the npm dependencies are available. Restart `cargo run` to pick up UI changes.

To compile a release executable:

```powershell
npm --prefix ui run build
cargo build --release --locked
```

On Windows, the executable is `target/release/gitcraft.exe`. To package an installer (NSIS and MSI on Windows; deb, rpm, AppImage, or dmg on those systems):

```powershell
npm --prefix ui run installer
```

Installers are written to `target/release/bundle/`. Maintainers publish preview installers and signed stable updates through GitHub Actions. See [RELEASING.md](RELEASING.md).

