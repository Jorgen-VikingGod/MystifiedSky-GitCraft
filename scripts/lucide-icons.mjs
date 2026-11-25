// Write the Lucide icons the egui interface uses as SVG files, drawn in white so egui can tint them.
// Usage: node scripts/lucide-icons.mjs <lucide-react folder> <icon-name>...   (names as in lucide, e.g. git-branch)
// Get the package with `npm install --prefix /tmp/lucide lucide-react@0.468.0` and pass
// /tmp/lucide/node_modules/lucide-react.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const [pkg, ...names] = process.argv.slice(2);
if (!pkg) throw new Error("Usage: node scripts/lucide-icons.mjs <lucide-react folder> <icon-name>...");
const source = join(pkg, "dist/esm/icons");
const target = join(root, "crates/gitcraft/assets/icons");

for (const name of names) {
  let text = readFileSync(join(source, `${name}.js`), "utf8");
  // Older names re-export the icon under its current name.
  const alias = /export \{ default \} from '\.\/([^']+)\.js';/.exec(text);
  if (alias) text = readFileSync(join(source, `${alias[1]}.js`), "utf8");
  const match = /createLucideIcon\("[^"]+",\s*(\[[\s\S]*\])\s*\);/.exec(text);
  if (!match) throw new Error(`No icon data in ${name}.js`);
  // The data is plain object literals: quote the keys and read it as JSON, without running it.
  const json = match[1].replace(/([{,]\s*)([A-Za-z_][\w-]*)\s*:/g, '$1"$2":').replace(/,(\s*[\]}])/g, "$1");
  const nodes = JSON.parse(json);
  const body = nodes.map(([tag, attrs]) => {
    const list = Object.entries(attrs).filter(([key]) => key !== "key").map(([key, value]) => `${key}="${value}"`).join(" ");
    return `  <${tag} ${list}/>`;
  }).join("\n");
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">\n${body}\n</svg>\n`;
  writeFileSync(join(target, `${name}.svg`), svg);
}
