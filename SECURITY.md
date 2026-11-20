# Security policy

## Report a vulnerability

Please use GitHub's [private vulnerability reporting form](https://github.com/MystifiedSky/GitCraft/security/advisories/new) for security problems. Avoid posting exploit details, credentials, or private repository contents in a public issue.

Include the affected GitCraft version, platform, steps to reproduce the issue, likely impact, and a minimal example when possible. Use a disposable repository and redact sensitive data.

## Updates and fixes

Use the [latest published release](https://github.com/MystifiedSky/GitCraft/releases/latest). The latest stable release is the supported release line. Security fixes are recorded in the [changelog](CHANGELOG.md). Backports are evaluated case by case.

Dependency advisories are reviewed against the code GitCraft actually builds. If a dependency requires a temporary backport, its source, changes, and removal conditions should be documented alongside the patch.
