// Set GitCraft's version in Cargo.toml ([workspace.package]) and Cargo.lock, for locked builds.
// Usage: node scripts/set-version.mjs 0.2.0
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? "")) {
  console.error("Give a version like 0.2.0");
  process.exit(1);
}
const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const cargo = join(root, "Cargo.toml");
const text = readFileSync(cargo, "utf8");
const workspace = /(^\[workspace\.package\]\r?\n(?:[^[\r\n][^\r\n]*\r?\n)*?version = ")[^"\r\n]+/m;
if (!workspace.test(text)) throw new Error("Missing [workspace.package] version in Cargo.toml");
writeFileSync(cargo, text.replace(workspace, (_, start) => start + version));

const lock = join(root, "Cargo.lock");
let lockText = readFileSync(lock, "utf8");
for (const name of ["gitcraft", "gitcraft-core"]) {
  const entry = new RegExp(`(^\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = ")[^"\\r\\n]+`, "m");
  if (!entry.test(lockText)) throw new Error(`Missing ${name} package in Cargo.lock`);
  lockText = lockText.replace(entry, (_, start) => start + version);
}
writeFileSync(lock, lockText);

console.log(`Version set to ${version}.`);
