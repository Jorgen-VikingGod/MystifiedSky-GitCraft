import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatRelease, parseEntries, recordRelease, releaseNotes } from './changelog-release.mjs';

const shipped = `# Changelog

## [Unreleased]

### Added

- First feature.

- Second feature,
  continued on a second line.

### Fixed

- A fix.

## [0.1.5] - 2026-11-11

### Added

- Old.

[Unreleased]: https://github.com/o/r/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/o/r/compare/v0.1.4...v0.1.5
`;

// A later commit added one more entry and reworded nothing.
const current = shipped.replace('### Fixed', '- Third feature, added after the release.\n\n### Fixed');

test('parses bullets with continuation lines and ignores prose', () => {
  const entries = parseEntries('Expected next release: `0.1.6`.\n\n### Added\n\n- One.\n\n- Two\n  more.\n');
  assert.deepEqual(entries, { Added: ['- One.', '- Two\n  more.'] });
});

test('moves only the shipped entries into a dated section and updates the links', () => {
  const out = recordRelease({ current, shipped, version: '0.1.6', repo: 'o/r', date: '2026-11-12' });
  const [unreleased, rest] = out.split('## [0.1.6] - 2026-11-12');
  assert.match(unreleased, /Third feature/);
  assert.doesNotMatch(unreleased, /First feature/);
  assert.match(rest, /First feature/);
  assert.match(rest, /continued on a second line/);
  assert.match(rest, /A fix/);
  assert.doesNotMatch(rest.split('## [0.1.5]')[0], /Third feature/);
  assert.match(out, /\[Unreleased\]: https:\/\/github.com\/o\/r\/compare\/v0\.1\.6\.\.\.HEAD/);
  assert.match(out, /\[0\.1\.6\]: https:\/\/github.com\/o\/r\/compare\/v0\.1\.5\.\.\.v0\.1\.6/);
  assert.match(out, /\[0\.1\.5\]: /);
});

test('is idempotent and ignores releases with no entries', () => {
  const once = recordRelease({ current, shipped, version: '0.1.6', repo: 'o/r', date: '2026-11-12' });
  assert.equal(recordRelease({ current: once, shipped, version: '0.1.6', repo: 'o/r', date: '2026-11-12' }), null);
  const empty = shipped.replace(/### Added[\s\S]*?(?=## \[0\.1\.5\])/, '');
  assert.equal(recordRelease({ current: empty, shipped: empty, version: '0.1.7', repo: 'o/r', date: '2026-11-12' }), null);
});

test('release notes list the Unreleased entries by category', () => {
  const notes = releaseNotes(shipped);
  assert.match(notes, /^### Added\n\n- First feature\./);
  assert.match(notes, /### Fixed\n\n- A fix\./);
  assert.equal(releaseNotes('# C\n\n## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n'), '');
});

test('queued releases exclude already published bullets even when main still lists them', () => {
  const baseline = shipped.replace('### Added', '### Changed').replace('continued on a second line.', '  continued on a second line.');
  const newNotes = releaseNotes(current, baseline);
  assert.equal(newNotes, '### Added\n\n- Third feature, added after the release.\n');
  assert.equal(releaseNotes(shipped, baseline), '');
  const archived = recordRelease({ current, shipped: current, previousChangelog: baseline, version: '0.1.7', repo: 'o/r', date: '2026-11-14' });
  const section = archived.split('## [0.1.7] - 2026-11-14\n\n')[1].split('\n## [0.1.5]')[0];
  assert.equal(section.trim(), newNotes.trim());
});

test('release notes include a new bullet only once even if listed in multiple categories', () => {
  const duplicate = shipped.replace('- A fix.', '- A fix.\n- First feature.');
  assert.equal(releaseNotes(duplicate), [
    '### Added', '', '- First feature.', '', '- Second feature,',
    '  continued on a second line.', '', '### Fixed', '', '- A fix.', '',
  ].join('\n'));
});

test('PR attribution is preserved for new notes and does not republish shipped notes', () => {
  const pr = ' ([#42](https://github.com/o/r/pull/42))';
  const another = ' ([#43](https://github.com/o/r/pull/43))';
  const linked = current
    .replace('- First feature.', '- First feature.' + pr + another)
    .replace('- Third feature, added after the release.', '- Third feature, added after the release.' + another);
  assert.equal(releaseNotes(linked, shipped), '### Added\n\n- Third feature, added after the release.' + another + '\n');
  assert.equal(releaseNotes(shipped, linked), '');
  assert.match(releaseNotes(linked), /First feature\. \(\[#42\]/);
});

test('the release page uses curated entries and credits only their PRs from the release range', () => {
  const changelog = current
    .replace('- First feature.', '- First feature. ([#41](https://github.com/o/r/pull/41))')
    .replace('added after the release.', 'added after the release. ([#42](https://github.com/o/r/pull/42)) ([#43](https://github.com/o/r/pull/43))');
  const notes = formatRelease({
    changelog,
    previousChangelog: shipped,
    repo: 'o/r',
    tag: 'v0.1.7',
    previousTag: 'v0.1.6',
    highlights: '  More control over your repository.  ',
    generated: [
      "## What's Changed",
      '* Old PR by @old-author in https://github.com/o/r/pull/41',
      '* Unused PR title by @new-author in https://github.com/O/R/pull/42',
      '* Same number in another repo by @wrong-author in https://github.com/o/elsewhere/pull/43',
      '## New Contributors',
      '* @new-author made their first contribution in https://github.com/o/r/pull/42',
      '* @unrelated made their first contribution in https://github.com/o/r/pull/44',
    ].join('\r\n'),
  });
  assert.equal(notes, [
    '## Highlights', '', 'More control over your repository.', '',
    "## What's Changed", '', '### Added', '',
    '- Third feature, added after the release. (by @new-author in [#42](https://github.com/o/r/pull/42)) ([#43](https://github.com/o/r/pull/43))', '',
    '## New Contributors', '',
    '- @new-author made their first contribution in [#42](https://github.com/o/r/pull/42).', '',
    '**Full Changelog**: https://github.com/o/r/compare/v0.1.6...v0.1.7', '',
  ].join('\n'));
});

test('the release page works without highlights, generated credits, or new changes', () => {
  const args = { changelog: shipped, previousChangelog: shipped, repo: 'o/r', tag: 'v0.1.7', previousTag: 'v0.1.6' };
  assert.equal(formatRelease(args), '**Full Changelog**: https://github.com/o/r/compare/v0.1.6...v0.1.7\n');
  const changelog = shipped.replace('A fix.', 'A fix. ([#22](https://github.com/o/r/pull/22))');
  assert.match(formatRelease({ ...args, changelog, previousChangelog: '' }), /A fix\. \(\[#22\]/);
});

test('archiving keeps references added to a shipped bullet without consuming new entries', () => {
  const pr = ' ([#42](https://github.com/o/r/pull/42))';
  const linked = current.replace('- First feature.', '- First feature.' + pr);
  const archived = recordRelease({ current: linked, shipped, version: '0.1.6', repo: 'o/r', date: '2026-11-12' });
  const [pending, release] = archived.split('## [0.1.6]');
  assert.doesNotMatch(pending, /First feature/);
  assert.match(pending, /Third feature/);
  assert.ok(release.includes('- First feature.' + pr + '\n'));
});

test('a substantive link or a changed description still counts as a new note', () => {
  const changed = shipped.replace('- First feature.', '- First feature. ([guide](https://example.test/guide))');
  assert.equal(releaseNotes(changed, shipped), '### Added\n\n- First feature. ([guide](https://example.test/guide))\n');
});

test('category headings and trailing whitespace accept long inputs without backtracking', () => {
  assert.deepEqual(parseEntries(`### Fixed${' '.repeat(100000)}\n\n- Kept.`), { Fixed: ['- Kept.'] });
  const current = `# Changelog\n\n## [Unreleased]\n\n### Fixed\n\n- Kept.\n\n[old]: https://example.test/old\n${' '.repeat(100000)}`;
  const released = recordRelease({ current, shipped: current, version: '1.0.0', repo: 'owner/project', date: '2026-11-14' });
  assert.ok(released.includes('[Unreleased]: https://github.com/owner/project/compare/v1.0.0...HEAD'));
});

test('release versions are validated instead of interpreted as regular expressions', () => {
  for (const version of ['1.2.3\\', '1.2.3.*', '(a+)+$', '1.2.3]\n## [Unreleased]', '', undefined]) {
    assert.throws(() => recordRelease({ current, shipped, version, repo: 'o/r', date: '2026-11-14' }), /Release version/);
  }
  const withNearbyVersion = current + '\n## [10.1.6] - 2026-11-13\n';
  assert.ok(recordRelease({ current: withNearbyVersion, shipped, version: '0.1.6', repo: 'o/r', date: '2026-11-14' }));
});


test('records all shipped notes when only some pending notes were reworded', () => {
  const changed = shipped.replace('First feature.', 'First feature reworded.');
  const result = recordRelease({ current: changed, shipped, version: '0.1.7', repo: 'o/r', date: '2026-11-14' });
  const [pending, release] = result.split('## [0.1.7]');
  assert.match(pending, /First feature reworded/);
  assert.match(release, /First feature\./);
  assert.match(release, /Second feature/);
  assert.match(release, /A fix/);
});
