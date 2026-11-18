import { existsSync, mkdirSync, realpathSync, statSync } from 'node:fs';
import { isAbsolute, join, relative, sep } from 'node:path';

// Only launch app builds from this checkout; never execute an arbitrary CLI path.
export function desktopOptions(args, repository) {
  if (args.length > 2) throw new Error('Usage: verify-desktop.mjs [release|debug] [report-tag]');
  let executable;
  switch (args[0] ?? 'release') {
    case 'release':
    case 'target/release/gitcraft.exe':
      executable = join(repository, 'target', 'release', 'gitcraft.exe');
      break;
    case 'debug':
    case 'target/debug/gitcraft.exe':
      executable = join(repository, 'target', 'debug', 'gitcraft.exe');
      break;
    default:
      throw new Error('Choose the release or debug build from this checkout.');
  }
  const tag = args[1] ?? 'windows';
  if (tag.length < 1 || tag.length > 48 || !/^[a-zA-Z0-9]/.test(tag) || /[^a-zA-Z0-9_-]/.test(tag)) {
    throw new Error('Report tags must contain 1–48 ASCII letters, digits, underscores or hyphens, starting with a letter or digit.');
  }
  return { executable, tag };
}

export function containedPath(repository, path) {
  const base = realpathSync(repository);
  const canonical = realpathSync(path);
  const child = relative(base, canonical);
  if (child === '..' || child.startsWith(`..${sep}`) || isAbsolute(child)) {
    throw new Error('Desktop verification paths must stay inside this checkout.');
  }
  return canonical;
}

export function artifactDirectory(repository) {
  let directory = realpathSync(repository);
  // Check each existing parent before creating anything beneath it. This also
  // refuses a target/verification junction that points outside the checkout.
  for (const part of ['target', 'verification']) {
    const next = join(directory, part);
    if (!existsSync(next)) mkdirSync(next);
    directory = containedPath(repository, next);
    if (!statSync(directory).isDirectory()) throw new Error('Expected an artifact directory.');
  }
  return directory;
}

export function windowsTools() {
  const windows = process.env.SystemRoot ?? 'C:\\Windows';
  const powershell = join(windows, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const ping = join(windows, 'System32', 'PING.EXE');
  const command = join(windows, 'System32', 'cmd.exe');
  const roots = [...new Set([process.env.ProgramW6432, process.env.ProgramFiles, process.env['ProgramFiles(x86)'], 'C:\\Program Files'].filter(Boolean))];
  const git = roots.flatMap(root => [join(root, 'Git', 'cmd', 'git.exe'), join(root, 'Git', 'bin', 'git.exe')]).find(path => existsSync(path));
  if (!git) throw new Error('Install Git for Windows under Program Files to run desktop verification.');
  for (const path of [powershell, ping, command, git]) {
    if (!isAbsolute(path) || !statSync(path).isFile()) throw new Error('A required Windows verification executable is missing.');
  }
  return { powershell, ping, command, git };
}
