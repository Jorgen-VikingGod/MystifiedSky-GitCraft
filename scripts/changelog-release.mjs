// Keeps CHANGELOG.md in step with published releases.
//
//   node scripts/changelog-release.mjs notes <changelog> [previous-changelog]      print entries added since the previous release
//   node scripts/changelog-release.mjs publish-notes <changelog> <previous-changelog> <owner/repo> <tag> [previous-tag] [generated-markdown]
//   node scripts/changelog-release.mjs record <version> <owner/repo> <shipped-changelog> [--previous <changelog>] [--date YYYY-MM-DD]
//                                                                                   move the entries that shipped into a dated section
//
// "shipped-changelog" is CHANGELOG.md as it was in the released commit. Only entries found there are moved, so entries added to
// Unreleased by commits pushed after the release stay under Unreleased. Running it twice for one version changes nothing.
import { readFileSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export const CATEGORIES = ['Added', 'Changed', 'Deprecated', 'Removed', 'Fixed', 'Security'];

const normalize = (text) => text.replace(/\r\n/g, '\n');

// A trailing PR link credits a change without changing its release identity.
const prLink = / \(\[#([1-9]\d*)\]\((https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/\1)\)\)$/;
const generatedCredit = /^[-*] .* by @([\w-]+(?:\[bot\])?) in (https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/[1-9]\d*)$/;
const generatedFirstContribution = /^[-*] @([\w-]+) made their first contribution in (https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/[1-9]\d*)$/;

function splitPrLinks(entry) {
  const links = [];
  let match;
  while ((match = prLink.exec(entry))) {
    links.unshift(match[0]);
    entry = entry.slice(0, match.index);
  }
  return { text: entry, links };
}

const entryKey = (entry) => splitPrLinks(entry).text.replace(/\s+/g, ' ');

/** Splits the file around the Unreleased section: text before it, its body, and everything after (versions and link references). */
export function splitUnreleased(text) {
  const src = normalize(text);
  const start = src.search(/^## \[Unreleased\][^\n]*$/m);
  if (start < 0) throw new Error('CHANGELOG.md has no "## [Unreleased]" section.');
  const afterHeading = src.indexOf('\n', start) + 1;
  const rest = src.slice(afterHeading);
  const next = rest.search(/^(## \[|\[[^\]]+\]: )/m);
  const bodyEnd = next < 0 ? rest.length : next;
  return { head: src.slice(0, afterHeading), body: rest.slice(0, bodyEnd), tail: rest.slice(bodyEnd) };
}

/** Reads a section body into { Added: [entry, ...], ... }. An entry is a bullet with its continuation lines, unchanged. */
export function parseEntries(body) {
  const out = {};
  let category = null;
  let entry = null;
  const flush = () => {
    if (category && entry) (out[category] ??= []).push(entry.join('\n').trimEnd());
    entry = null;
  };
  for (const line of normalize(body).split('\n')) {
    const heading = line.startsWith("### ") ? line.slice(4).trimEnd() : "";
    if (heading) {
      flush();
      category = heading;
    } else if (/^- /.test(line)) {
      flush();
      entry = [line];
    } else if (entry && (line.trim() === '' || /^\s+/.test(line))) {
      entry.push(line);
    } else if (line.trim() !== '') {
      flush(); // prose such as an old "Expected next release" note: not an entry
    }
  }
  flush();
  return out;
}

const categoryOrder = (names) => [...CATEGORIES.filter((c) => names.includes(c)), ...names.filter((c) => !CATEGORIES.includes(c))];

/** Writes entries back as "### Category" blocks with a blank line between entries. */
export function renderEntries(entries) {
  const names = categoryOrder(Object.keys(entries).filter((c) => entries[c]?.length));
  return names.map((c) => `### ${c}\n\n${entries[c].join('\n\n')}\n`).join('\n');
}

export const isEmpty = (entries) => Object.values(entries).every((list) => !list?.length);

function newEntries(changelogText, previousChangelog = '') {
  const entries = parseEntries(splitUnreleased(changelogText).body);
  const previous = previousChangelog.trim() ? parseEntries(splitUnreleased(previousChangelog).body) : {};
  const seen = new Set(Object.values(previous).flat().map(entryKey));
  for (const category of categoryOrder(Object.keys(entries))) {
    entries[category] = entries[category].filter((entry) => {
      const id = entryKey(entry);
      if (seen.has(id)) return false;
      seen.add(id);
      return true;
    });
  }
  return entries;
}

/** The shipped entries as GitHub release notes (empty string when there are none). */
export function releaseNotes(changelogText, previousChangelog) {
  return renderEntries(newEntries(changelogText, previousChangelog));
}

export function formatRelease({ changelog, previousChangelog, repo, tag, previousTag, highlights = '', generated = '' }) {
  const base = `https://github.com/${repo}`;
  const prPrefix = `${base.toLowerCase()}/pull/`;
  const entries = newEntries(changelog, previousChangelog);
  const authors = new Map();
  const firstContributions = new Map();
  // Use GitHub's generated notes for attribution. The changelog supplies the actual change descriptions.
  for (const line of normalize(generated).split('\n')) {
    const credit = generatedCredit.exec(line.trimEnd());
    const first = generatedFirstContribution.exec(line.trimEnd());
    const match = credit ?? first;
    if (!match) continue;
    const [, author, url] = match;
    const key = url.toLowerCase();
    if (!key.startsWith(prPrefix)) continue;
    authors.set(key, author);
    if (first) firstContributions.set(key, author);
  }

  const credited = new Map();
  for (const list of Object.values(entries)) {
    for (const [index, entry] of list.entries()) {
      const { text, links } = splitPrLinks(entry);
      list[index] = text + links.map((link) => {
        const [, number, url] = prLink.exec(link);
        const key = url.toLowerCase();
        const author = authors.get(key);
        if (!author) return link;
        const first = firstContributions.get(key);
        if (first && !credited.has(first.toLowerCase())) {
          credited.set(first.toLowerCase(), `- @${first} made their first contribution in [#${number}](${base}/pull/${number}).`);
        }
        return ` (by @${author} in [#${number}](${base}/pull/${number}))`;
      }).join('');
    }
  }

  const sections = [];
  if (highlights.trim()) sections.push(`## Highlights\n\n${highlights.trim()}`);
  if (!isEmpty(entries)) sections.push(`## What's Changed\n\n${renderEntries(entries).trimEnd()}`);
  if (credited.size) sections.push(`## New Contributors\n\n${[...credited.values()].join('\n')}`);
  if (previousTag) sections.push(`**Full Changelog**: ${base}/compare/${previousTag}...${tag}`);
  return sections.length ? `${sections.join('\n\n')}\n` : '';
}

/**
 * Returns the new CHANGELOG.md text, or null when nothing needs to change (the version is already recorded, or nothing shipped).
 */
export function recordRelease({ current, shipped, previousChangelog, version, repo, date }) {
  const text = normalize(current);
  if (!/^\d+\.\d+\.\d+$/.test(version ?? '')) throw new Error('Release version must be three numeric components (for example 1.2.3).');
  const releaseHeading = `## [${version}]`;
  if (text.split('\n').some((line) => line.startsWith(releaseHeading))) return null;
  const shippedEntries = newEntries(shipped, previousChangelog);
  if (isEmpty(shippedEntries)) return null;

  const { head, body, tail } = splitUnreleased(text);
  const remaining = parseEntries(body);
  for (const [category, list] of Object.entries(shippedEntries)) {
    for (const [index, entry] of list.entries()) {
      const shippedBullet = splitPrLinks(entry);
      const at = remaining[category]?.findIndex((pending) => splitPrLinks(pending).text === shippedBullet.text) ?? -1;
      if (at >= 0) {
        const [pending] = remaining[category].splice(at, 1);
        const links = new Set([...shippedBullet.links, ...splitPrLinks(pending).links]);
        list[index] = shippedBullet.text + [...links].join('');
      }
    }
  }
  // Keep the shipped wording even if a later commit reworded it in Unreleased.
  const section = shippedEntries;

  const unreleased = isEmpty(remaining) ? '\n' : `\n${renderEntries(remaining)}\n`;
  let tailText = tail;
  const compare = /^\[Unreleased\]: (https:\/\/github\.com\/[^/]+\/[^/]+)\/compare\/(v[^.\s]+\.[^.\s]+\.[^.\s]+)\.\.\.HEAD\s*$/m.exec(tailText);
  const base = compare?.[1] ?? `https://github.com/${repo}`;
  const previous = compare?.[2];
  const links = [`[Unreleased]: ${base}/compare/v${version}...HEAD`, previous ? `[${version}]: ${base}/compare/${previous}...v${version}` : `[${version}]: ${base}/releases/tag/v${version}`];
  if (compare) tailText = tailText.replace(compare[0], links.join('\n'));
  else tailText = `${tailText.trimEnd()}\n\n${links.join('\n')}\n`;

  return `${head}${unreleased}\n## [${version}] - ${date}\n\n${renderEntries(section)}\n${tailText.replace(/^\n+/, '')}`.replace(/\n{3,}/g, '\n\n');
}

function main(argv) {
  const [command, ...args] = argv;
  if (command === 'notes') {
    const [currentPath, previousPath] = args;
    process.stdout.write(releaseNotes(readFileSync(currentPath, 'utf8'), previousPath ? readFileSync(previousPath, 'utf8') : ''));
  } else if (command === 'publish-notes') {
    const [currentPath, previousPath, repo, tag, previousTag = '', generatedPath] = args;
    const generated = generatedPath ? readFileSync(generatedPath, 'utf8') : '';
    process.stdout.write(formatRelease({
      changelog: readFileSync(currentPath, 'utf8'),
      previousChangelog: previousPath ? readFileSync(previousPath, 'utf8') : '',
      repo,
      tag,
      previousTag,
      highlights: process.env.RELEASE_HIGHLIGHTS,
      generated,
    }));
  } else if (command === 'record') {
    const [version, repo, shippedPath] = args;
    const dateFlag = args.indexOf('--date');
    const previousFlag = args.indexOf('--previous');
    const date = dateFlag >= 0 ? args[dateFlag + 1] : new Date().toISOString().slice(0, 10);
    if (!/^\d+\.\d+\.\d+$/.test(version ?? '') || !repo || !shippedPath) throw new Error('Usage: record <version> <owner/repo> <shipped-changelog> [--date YYYY-MM-DD]');
    const current = readFileSync('CHANGELOG.md', 'utf8');
    const previousChangelog = previousFlag >= 0 ? readFileSync(args[previousFlag + 1], 'utf8') : '';
    const next = recordRelease({ current, shipped: readFileSync(shippedPath, 'utf8'), previousChangelog, version, repo, date });
    if (next === null) console.log(`CHANGELOG.md already records ${version} or nothing shipped; no change.`);
    else {
      writeFileSync('CHANGELOG.md', next);
      console.log(`Recorded ${version} in CHANGELOG.md.`);
    }
  } else {
    throw new Error('Expected notes, publish-notes, or record. See the usage examples at the top of this script.');
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main(process.argv.slice(2));
