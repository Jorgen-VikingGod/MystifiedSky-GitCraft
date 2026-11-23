// Keep the app manifests and lockfiles on the same version for locked builds. Needs only Node,
// so release builds of the native interface can run it without installing the front end.
// Usage (from app/):  npm --prefix ui run version:set -- 0.2.0
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? "")) {
  console.error("Give a version like 0.2.0");
  process.exit(1);
}
const app = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

const json = (file, edit) => {
  const p = join(app, file);
  const data = JSON.parse(readFileSync(p, "utf8"));
  edit(data);
  writeFileSync(p, JSON.stringify(data, null, 2) + "\n");
};
json("ui/package.json", (c) => (c.version = version));
json("ui/package-lock.json", (c) => {
  c.version = version;
  c.packages[""].version = version;
});

// The base version lives in [workspace.package]; every GitCraft crate (and the desktop app, which
// reads it from Cargo.toml) takes it from there.
const cargo = join(app, "Cargo.toml");
const text = readFileSync(cargo, "utf8");
const workspace = /(^\[workspace\.package\]\r?\n(?:[^\[\r\n][^\r\n]*\r?\n)*?version = ")[^"\r\n]+/m;
if (!workspace.test(text)) throw new Error("Missing [workspace.package] version in Cargo.toml");
writeFileSync(cargo, text.replace(workspace, (_, start) => start + version));

const lock = join(app, "Cargo.lock");
let lockText = readFileSync(lock, "utf8");
for (const name of ["gitcraft", "gitcraft-core", "gitcraft-egui"]) {
  const entry = new RegExp(`(^\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = ")[^"\\r\\n]+`, "m");
  if (!entry.test(lockText)) throw new Error(`Missing ${name} package in Cargo.lock`);
  lockText = lockText.replace(entry, (_, start) => start + version);
}
writeFileSync(lock, lockText);

console.log(`Version set to ${version}.`);
