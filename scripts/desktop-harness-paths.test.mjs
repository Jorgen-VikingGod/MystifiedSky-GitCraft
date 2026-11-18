import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, realpathSync, symlinkSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, isAbsolute, basename } from 'node:path';
import { desktopOptions, containedPath, artifactDirectory, windowsTools } from './desktop-harness-paths.mjs';

function fixture(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'gitcraft-harness-')));
  t.after(() => {
    assert.ok(isAbsolute(root) && basename(root).startsWith('gitcraft-harness-'));
    assert.equal(realpathSync(root), root);
    rmSync(root, { recursive: true, force: true });
  });
  return root;
}

test('only local debug/release builds can be selected', () => {
  const repository = join(tmpdir(), 'checkout');
  assert.deepEqual(desktopOptions([], repository), { executable: join(repository, 'target/release/gitcraft.exe'), tag: 'windows' });
  for (const build of ['release', 'target/release/gitcraft.exe']) assert.equal(desktopOptions([build,'csp-1'],repository).executable, join(repository, 'target/release/gitcraft.exe'));
  for (const build of ['debug', 'target/debug/gitcraft.exe']) assert.equal(desktopOptions([build],repository).executable, join(repository, 'target/debug/gitcraft.exe'));
  for (const build of ['../other.exe', 'C:\\Windows\\System32\\cmd.exe', '/bin/sh', 'release & calc.exe', 'release\n', 'target/release/../../other.exe']) {
    assert.throws(() => desktopOptions([build], repository));
  }
  assert.throws(() => desktopOptions(['release','ok','unexpected'], repository));
});

test('rejects traversal, drive, shell and control characters in artifact tags', () => {
  for (const tag of ['../../outside', '..\\outside', '/tmp/payload', 'C:\\payload', '..', '', '-hidden', 'a/b', 'a\\b', 'x&calc', 'x%PATH%', "x'y", 'x\ny', 'x\n', 'x\r\n', 'x\0y', 'x'.repeat(49)]) {
    assert.throws(() => desktopOptions(['release',tag], tmpdir()));
  }
  assert.equal(desktopOptions(['release','CSP_2026-11'],tmpdir()).tag, 'CSP_2026-11');
});

test('containment resolves aliases and refuses sibling-prefix paths', t => {
  const root = fixture(t), repository = join(root,'app'); mkdirSync(repository);
  const outside = join(root,'app-outside'); mkdirSync(outside);
  const inside = join(repository,'build.exe'); writeFileSync(inside,'fixture');
  assert.equal(containedPath(repository,inside), realpathSync(inside));
  assert.throws(() => containedPath(repository,outside));
  const alias = join(repository,'escape'); symlinkSync(outside,alias,process.platform==='win32'?'junction':'dir');
  assert.throws(() => containedPath(repository,alias));
});

test('an outside target junction is rejected before creating verification files', t => {
  const root = fixture(t), repository = join(root,'app'); mkdirSync(repository);
  const outside = join(root,'outside'); mkdirSync(outside);
  symlinkSync(outside,join(repository,'target'),process.platform==='win32'?'junction':'dir');
  assert.throws(() => artifactDirectory(repository));
  assert.equal(existsSync(join(outside,'verification')),false);
});

test('an outside verification junction is rejected', t => {
  const root = fixture(t), repository = join(root,'app'); mkdirSync(repository);
  mkdirSync(join(repository,'target')); const outside = join(root,'outside'); mkdirSync(outside);
  symlinkSync(outside,join(repository,'target/verification'),process.platform==='win32'?'junction':'dir');
  assert.throws(() => artifactDirectory(repository));
});

test('artifact directories are created inside the checkout', t => {
  const repository = fixture(t);
  const artifact = artifactDirectory(repository);
  assert.equal(artifact,realpathSync(join(repository,'target/verification')));
  assert.equal(artifactDirectory(repository),artifact);
});

test('Windows verification ignores a poisoned PATH', { skip: process.platform !== 'win32' }, t => {
  const root = fixture(t);
  writeFileSync(join(root,'git.exe'),'untrusted fixture');
  writeFileSync(join(root,'powershell.exe'),'untrusted fixture');
  const previous = process.env.PATH;
  try {
    process.env.PATH = root;
    for (const path of Object.values(windowsTools())) {
      assert.ok(isAbsolute(path) && existsSync(path));
      assert.ok(!path.startsWith(root));
    }
  } finally {
    if (previous === undefined) delete process.env.PATH; else process.env.PATH = previous;
  }
});
