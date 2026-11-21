# GitCraft roadmap

## Current priorities

- Protect local work through staging, stashing, merge, rebase, and undo. Add regression fixtures for reported failures.
- Verify installers, updates, and native integrations on Windows, macOS, and Linux. Run a preview for changes that need real desktop feedback.
- Make common workflows easier to learn through sample-repository examples, keyboard shortcuts, and translations.

## Support

The latest stable release is the supported release line. Prioritize data loss, broken updates, and regressions in core Git workflows. The [test guide](../TESTING.md) records build prerequisites, the minimum Git version, and platform checks. GitCraft requires Git 2.38 or newer.

| Platform | Native PR tests | Release packages |
| --- | --- | --- |
| Windows | GitHub Actions Windows Server 2022 x64 | x64 MSI and NSIS, ARM64 NSIS |
| macOS | GitHub Actions latest macOS runner | Universal DMG for Apple silicon and Intel |
| Linux | GitHub Actions Ubuntu 22.04 x64 | x64 and ARM64 AppImage, DEB, and RPM |

The runner versions describe the environments used for automated tests. Installer smoke testing on real desktops is tracked in the [validation checklist](../BUGS_AND_OPTIMIZATIONS.md). Current build and update instructions are in the [release guide](../RELEASING.md).

Use [GitHub Issues](https://github.com/MystifiedSky/GitCraft/issues) to track reproducible bugs and accepted work. Use [Discord](https://discord.gg/BsUgBu65YG) for usage questions and early ideas. Follow the [security policy](../SECURITY.md) for private vulnerability reporting.

## Contribute

Start with a [good first issue](https://github.com/MystifiedSky/GitCraft/labels/good%20first%20issue) or the [contributing guide](../CONTRIBUTING.md). The [validation and optimization checklist](../BUGS_AND_OPTIMIZATIONS.md) records deeper work that still needs investigation. Invite repeat contributors to review areas they know well. Keep release publishing and signing keys with maintainers.

Use the [maintainer guide](MAINTAINING.md) during weekly triage. Link issues here when they represent a current priority.
