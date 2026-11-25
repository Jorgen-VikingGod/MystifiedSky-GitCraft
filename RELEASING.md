# Releasing GitCraft

Run **Release** from Actions on `main` and select **preview** or **stable**. The workflow builds the selected commit and adds the workflow run number to the patch component of the base version in `Cargo.toml` (`[workspace.package]`). For example, base `0.1.0` plus run 1 yields `0.1.1`. The workflow updates the version in its checkout only. It creates a version tag at the source commit when it publishes, so no version bump commits or manually pushed tags are needed.

The workflow reuses the passing **Tests** push run on `main` for the exact release commit. It runs the full test matrix if that result is unavailable.

The workflow builds Windows x64 (NSIS and MSI) and ARM64 (NSIS), macOS (a universal app, as a DMG and an update archive), and Linux x64 and ARM64 (AppImage, DEB, and RPM). Separate macOS jobs compile Apple silicon and Intel binaries in parallel, then `lipo` combines them. [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) makes every package except the RPM, from the `[package.metadata.packager]` settings in [crates/gitcraft/Cargo.toml](crates/gitcraft/Cargo.toml). [cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm) makes the RPM from `[package.metadata.generate-rpm]`. The workflow signs the updater bundles, checks that all expected assets are present, and verifies signatures with the app's public key before publishing to [GitHub Releases](https://github.com/MystifiedSky/GitCraft/releases/latest). Until that succeeds, installed copies continue to see the previous release.

For a macOS packaging rehearsal, dispatch **macos-dry-run** from the branch to test. It builds and checks both architectures and uploads an unsigned universal DMG as a workflow artifact. The dry run uses a release run number and has no publishing step.

Use **preview** for changes that benefit from testing on installed builds. Share the prerelease URL with volunteers and ask them to exercise the changed workflows on disposable repositories. Preview releases omit the updater manifest and the stable updater stays on the latest stable release. Preview users can reinstall the current stable version manually. The updater offers stable again once its version is newer than the installed preview.

When a preview is ready for everyone, dispatch **stable** on `main` after verifying that its tip is the commit you tested. The stable run builds a new version and publishes its updater manifest. Treat a different tip as a new candidate.

Use **Re-run failed jobs** in Actions to retry a failed release with the same version. If a higher version has already published, the retry skips publication. Use **Run workflow** on `main` if you need a fresh build. To change the base version, set it with `node scripts/set-version.mjs <version>` and commit the changed manifests and lockfiles. Keep the next calculated version higher than the current release.

## Changelog

Write new entries under `## [Unreleased]` in `CHANGELOG.md`. At publication, the workflow reads the changelog from the release commit and the previous published stable tag. The GitHub release page groups new entries under **What's Changed** and ends with **Full Changelog**, which compares those tags. This also works when several releases queue up before the changelog is archived. A preview and its eventual stable release can show the same changes because both compare against the last stable version.

For a larger release, enter an optional Markdown overview in the **highlights** field of the Release workflow. Use it to explain the release's theme, point readers to a feature guide, or give a short preview of upcoming work. Keep the overview brief and leave the detailed bullets in the changelog. Enter the overview again when dispatching the stable run if you want it on both releases.

Keep each bullet focused on the change users will notice. Once a pull request has a number, append its link to the relevant bullet. Use a separate parenthesized link for each PR if several shipped the same change. Leave direct commits without a PR unlinked.

```markdown
- Install the Linux RPM on Fedora and RHEL. ([#42](https://github.com/MystifiedSky/GitCraft/pull/42))
```

These links go into release notes with the bullet. GitHub supplies author credit and a **New Contributors** section when it identifies the linked PRs and first contributions in the release range. The release keeps its curated entries and links if GitHub's credit lookup fails. Adding a link to an already shipped bullet does not publish it again. If the link is added after publication but before archiving, the dated section keeps it.

To review the formatted page locally, save `CHANGELOG.md` from the previous stable tag as a UTF-8 file and run the following command with the real tag names. The optional sixth argument is a file containing GitHub-generated release Markdown for the same range. Set the `RELEASE_HIGHLIGHTS` environment variable to include your overview.

```sh
node scripts/changelog-release.mjs publish-notes CHANGELOG.md previous-changelog.md MystifiedSky/GitCraft vNEXT vPREVIOUS
```

Archive dated sections in a changelog pull request, oldest release first. Save `CHANGELOG.md` from the released tag and its previous published tag as UTF-8 files, then run `node scripts/changelog-release.mjs record <version> MystifiedSky/GitCraft <released-changelog> --previous <previous-changelog> --date <publication-date>`. Entries added after the released commit stay under Unreleased. Leave already published bullets as written until they are archived so the workflow can recognize them in the next release. Describe distinct changes with specific wording so two different changes do not look like duplicate entries.

## Updating installed copies

GitCraft checks GitHub Releases at startup and every hour while it’s open. When it finds a newer version, it downloads the update and verifies the signature using the public key built into the app. Choose **Update Ready (Restart GitCraft)** in the bottom right to install it. Turn off **Automatically download GitCraft updates** in **Preferences → General** to stop background checks and downloads. **File → Check for Update** still works with the setting off.

Install a published release once if your current build predates the signing key. The update feed (`latest.json`), the signatures and the installer names keep the format earlier releases read, so copies installed before GitCraft became a native app update to it too. Linux AppImage builds can update in place; DEB and RPM installations should be upgraded using the matching package from GitHub.

Update signing is separate from Windows Authenticode signing and Apple notarization. First-time installations may still show platform security prompts.

## Signing key

The repository already has the public key in [crates/gitcraft-core/src/updates.rs](crates/gitcraft-core/src/updates.rs) and the corresponding `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` Actions secrets (named for the tool that first made them; the release workflow passes them to cargo-packager). Keep a backup of that same private key and its password in a secure location. Replacing the key would require users to install a new build manually before receiving more updates.

For a new repository or restoration, install cargo-packager (`cargo install cargo-packager --locked --version 0.11.8`) and run `powershell -ExecutionPolicy Bypass -File scripts/setup-updates.ps1` from the app checkout. Use the existing key and its password when restoring this repository.
