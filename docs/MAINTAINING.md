# Maintainer guide

Use these review practices as responsibilities are shared across more contributors.

## Triage issues

1. Start with reports of lost work, broken updates, and regressions in core Git operations. Redirect sensitive vulnerability reports to the private channel in the [security policy](../SECURITY.md).
2. Search open and closed issues. Link duplicates to the issue where the work is tracked. For a new bug, ask for the smallest missing detail needed to reproduce it, such as the starting index state or package format.
3. Leave a short triage note with the affected area, reproduction status, and next action. Use existing labels such as `bug`, `documentation`, `enhancement`, and `help wanted` to help contributors find work.
4. Use `good first issue` when the task has a clear finish line, useful file pointers, and a way to verify it. Coordinate assignments in the issue to avoid duplicate work. If work stalls, leave a note and make the task available again when the contributor steps away or stops responding.
5. For a larger feature, agree on the user workflow and the first reviewable slice in the issue. Record the decision and link the implementation PRs. Summarize decisions made in chat in the public issue when appropriate.

## Review a pull request

- Check that the PR describes one clear change and includes enough evidence for another person to repeat the relevant checks. Ask for focused revisions or a split when a diff is hard to verify.
- For changes that write Git state, inspect the success and failure paths. Check the relevant refs, index, and working tree on a disposable repository. For UI changes, ask for before and after screenshots with sample data. For platform changes, identify the native platforms tested and any verification still pending.
- Separate changes required for correctness from optional suggestions. Record blockers in the PR so the author and later reviewers can see what remains.
- Check the AI disclosure for each PR. If AI was used, look for the tools and how they contributed, plus the author’s verification. For an automated dependency PR, a maintainer can complete the disclosure after confirming which automation authored it.
- Confirm the required CI checks passed for the current revision before merging. Verify changelog entries, compatibility notes, and third-party notices when they apply. Use the [release guide](../RELEASING.md) when a change needs preview testing.

## Share responsibility

Start with issue triage and reviews in an area a contributor knows well. Use the [code map](ARCHITECTURE.md) to route reviews to people familiar with the affected code. Grant repository access in stages based on the work someone takes on.

When areas have active reviewers who have agreed to maintain them, add them to `CODEOWNERS` to request reviews automatically. Assign a backup for release, updater, and Git state changes before making area approvals mandatory. Keep CI, release publishing, and signing responsibilities documented so another maintainer can take over with the appropriate access.
