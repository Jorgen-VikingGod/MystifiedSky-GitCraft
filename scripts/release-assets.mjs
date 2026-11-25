import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, rmdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [version, repo] = process.argv.slice(2);
if (!/^\d+\.\d+\.\d+$/.test(version ?? "") || !/^[\w.-]+\/[\w.-]+$/.test(repo ?? "")) {
  throw new Error("Usage: node scripts/release-assets.mjs <version> <owner/repo>");
}

const output = "release-assets";
mkdirSync(output);
const platforms = {};
const uploaded = new Set();
const updater = readFileSync(new URL("../crates/gitcraft-core/src/updates.rs", import.meta.url), "utf8");
const encodedKey = /pub const PUBLIC_KEY: &str = "([A-Za-z0-9+/]+={0,2})";/.exec(updater)?.[1];
if (!encodedKey) throw new Error("GitCraft's update public key is missing");
const publicKey = Buffer.from(encodedKey, "base64").toString("utf8").trim().split(/\r?\n/).at(-1);

function verifySignature(file, signature) {
  const directory = mkdtempSync(join(tmpdir(), "gitcraft-signature-"));
  const decoded = join(directory, "update.minisig");
  try {
    writeFileSync(decoded, Buffer.from(signature, "base64"));
    const result = spawnSync("/usr/bin/minisign", ["-V", "-q", "-P", publicKey, "-m", file, "-x", decoded], {
      encoding: "utf8",
    });
    if (result.error || result.status !== 0) {
      throw new Error(`Could not verify ${file} with GitCraft's update public key: ${result.error?.message ?? result.stderr.trim()}`);
    }
  } finally {
    rmSync(decoded, { force: true });
    rmdirSync(directory);
  }
}

function bundle(platform, suffix, targets = []) {
  const root = join("bundles", `release-${platform}`);
  const files = readdirSync(root, { recursive: true, withFileTypes: true });
  const matches = files.filter((file) => file.isFile() && file.name.endsWith(suffix));
  if (matches.length !== 1) {
    throw new Error(`Expected one ${platform} bundle ending with ${suffix}, found ${matches.length}`);
  }
  const { name, parentPath } = matches[0];
  const source = join(parentPath, name);
  if (targets.length) {
    const sigName = `${name}.sig`;
    const sigPath = join(parentPath, sigName);
    const signature = readFileSync(sigPath, "utf8").trim();
    if (!/^[A-Za-z0-9+/]+={0,2}$/.test(signature)) {
      throw new Error(`Missing or malformed updater signature: ${sigName}`);
    }
    verifySignature(source, signature);
    copy(sigPath, sigName);
    const url = `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(name)}`;
    for (const target of targets) platforms[target] = { url, signature };
  }
  copy(source, name);
}

function copy(source, name) {
  if (uploaded.has(name)) throw new Error(`Duplicate release asset: ${name}`);
  copyFileSync(source, join(output, name));
  uploaded.add(name);
}

bundle("windows", "-setup.exe", ["windows-x86_64", "windows-x86_64-nsis"]);
bundle("windows", ".msi", ["windows-x86_64-msi"]);
bundle("windows-arm64", "-setup.exe", ["windows-aarch64", "windows-aarch64-nsis"]);
bundle("macos", ".dmg");
bundle("macos", ".app.tar.gz", ["darwin-aarch64", "darwin-x86_64"]);
bundle("linux", ".AppImage", ["linux-x86_64"]);
bundle("linux", ".deb");
bundle("linux", ".rpm");
bundle("linux-arm64", ".AppImage", ["linux-aarch64"]);
bundle("linux-arm64", ".deb");
bundle("linux-arm64", ".rpm");

writeFileSync(join(output, "latest.json"), JSON.stringify({
  version,
  notes: `Release notes: https://github.com/${repo}/releases/tag/v${version}`,
  pub_date: new Date().toISOString(),
  platforms,
}, null, 2) + "\n");
