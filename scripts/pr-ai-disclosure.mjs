import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

export function disclosureError(body) {
  // Ignore comments and fenced code without reading their contents as Markdown.
  const visible = [];
  let fence;
  let inComment = false;
  for (const line of (body ?? "").split(/\r?\n/)) {
    const marker = inComment ? null : line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (fence) {
      if (marker && marker[1][0] === fence[0] && marker[1].length >= fence.length && !marker[2].trim()) fence = undefined;
    } else if (marker) {
      fence = marker[1];
    } else {
      let content = "";
      let rest = line;
      while (rest) {
        const delimiter = inComment ? "-->" : "<!--";
        const index = rest.indexOf(delimiter);
        if (!inComment) content += index < 0 ? rest : rest.slice(0, index);
        if (index < 0) break;
        inComment = !inComment;
        rest = rest.slice(index + delimiter.length);
      }
      visible.push(content);
    }
  }

  const start = visible.findIndex(line => /^## AI disclosure\s*$/i.test(line));
  if (start < 0) return "Add the AI disclosure section from the pull request template.";
  const next = visible.findIndex((line, i) => i > start && /^#{1,2}\s/.test(line));
  const section = visible.slice(start + 1, next < 0 ? undefined : next);

  const selections = section.filter(line => /^- \[x\] (I used AI for this PR|I did not use AI for this PR)\.?\s*$/i.test(line));
  if (selections.length !== 1) return "Select exactly one AI disclosure checkbox.";
  if (/did not use AI/i.test(selections[0])) return;

  for (const heading of ["Tools and models", "How AI was used"]) {
    const start = section.findIndex(line => line.trim() === `### ${heading}`);
    const end = section.findIndex((line, i) => i > start && /^#{1,3}\s/.test(line));
    const value = start < 0 ? "" : section.slice(start + 1, end < 0 ? undefined : end).join(" ").trim();
    if (!value || /^(?:[-*]\s+)?(?:todo|tbd|none|n\/?a|not applicable|\?+|[-.]+)[.!]?$/i.test(value)) {
      return `Complete the "${heading}" part of the AI disclosure.`;
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const pullRequest = JSON.parse(readFileSync(process.argv[2], "utf8"));
  const error = disclosureError(pullRequest.body);
  if (error) {
    console.error(error);
    process.exitCode = 1;
  } else {
    console.log("AI disclosure is complete.");
  }
}
