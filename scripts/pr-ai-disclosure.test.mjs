import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { disclosureError } from "./pr-ai-disclosure.mjs";

const template = readFileSync(new URL("../.github/pull_request_template.md", import.meta.url), "utf8");

test("a missing section and an untouched template fail", () => {
  assert.match(disclosureError(null), /Add the AI disclosure/);
  assert.match(disclosureError(template), /exactly one/);
});

test("an explicit no-AI declaration passes", () => {
  const body = template.replace("[ ] I did not use AI", "[x] I did not use AI");
  assert.equal(disclosureError(body), undefined);
  assert.match(disclosureError(body.replace("[ ] I used AI", "[x] I used AI")), /exactly one/);
});

test("AI use requires both the tools and the work they did", () => {
  const body = template.replace("[ ] I used AI", "[X] I used AI");
  assert.match(disclosureError(body), /Tools and models/);
  const withTool = body.replace("### Tools and models", "### Tools and models\nExample assistant");
  assert.match(disclosureError(withTool), /How AI was used/);
  assert.match(disclosureError(withTool.replace("### How AI was used", "### How AI was used\nTODO.")), /How AI was used/);
  const complete = withTool.replace("### How AI was used", "### How AI was used\nDrafted tests and explained a fixture.");
  assert.equal(disclosureError(complete.replaceAll("\n", "\r\n")), undefined);
});

test("a disclosure in a fenced example or HTML comment does not count", () => {
  const body = "## AI disclosure\n- [x] I did not use AI for this PR.";
  assert.match(disclosureError("~~~markdown\n" + body + "\n~~~"), /Add the AI disclosure/);
  assert.match(disclosureError("<!--\n" + body + "\n-->"), /Add the AI disclosure/);
});

test("comment markers in fenced code do not hide a disclosure", () => {
  const body = "## AI disclosure\n- [x] I did not use AI for this PR.";
  assert.equal(disclosureError("```html\n<!--\n```\n\n" + body), undefined);
  assert.equal(disclosureError("<!--\n```html\n-->\n\n" + body), undefined);
});

test("an indented heading starts a separate disclosure answer", () => {
  const body = "## AI disclosure\n- [x] I used AI for this PR.\n### Tools and models\n ### How AI was used\nDrafted tests.";
  assert.match(disclosureError(body), /Tools and models/);
  assert.equal(disclosureError(body.replace("### Tools and models", "### Tools and models\nExample assistant")), undefined);
});
