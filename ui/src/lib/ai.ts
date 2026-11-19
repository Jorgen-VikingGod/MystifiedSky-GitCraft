import { invokeRead } from "./api";
// AI helpers use the selected local or hosted connection; patch files and agents remain local.
import { invoke } from "@tauri-apps/api/core";
import { invokeAi } from "./ai-connection";
import { act } from "./repo";
import { useStore } from "./store";

export interface ComposedCommit {
  summary: string;
  description: string;
  files: string[];
}

export interface ConflictCheck {
  target: string;
  files: string[];
  note: string | null;
}

export interface AgentSession {
  id: string;
  title: string;
  modified: number;
  messages: number;
  agent: "claude" | "codex";
  path: string;
  status: "recent" | "finished" | "idle";
}

export const ai = {
  commitMessage: (path: string, sha?: string, preview?: number | null) => invokeAi<[string, string]>("ai_commit_message", { path, sha: sha ?? null, ...(preview == null ? {} : { preview }) }),
  stashMessage: (path: string, preview?: number | null) => invokeAi<string>("ai_stash_message", { path, ...(preview == null ? {} : { preview }) }),
  explainCommit: (path: string, sha: string) => invokeAi<string>("ai_explain_commit", { path, sha }),
  explainBranch: (path: string, branch: string, base?: string) => invokeAi<string>("ai_explain_branch", { path, branch, base: base ?? null }),
  compose: (path: string, preview?: number | null) => invokeAi<ComposedCommit[]>("ai_compose", { path, ...(preview == null ? {} : { preview }) }),
  savePatch: (path: string, shas: string[], name: string) => invoke<string | null>("save_patch", { path, shas, name }),
  pickPatch: () => invoke<string | null>("pick_patch_file"),
  conflictCheck: (path: string, target?: string, signal?: AbortSignal) => invokeRead<ConflictCheck>("conflict_check", { path, target: target ?? null }, signal),
  agents: (path: string, paths: string[], claudeDir: string, codexDir: string) => invoke<AgentSession[]>("agent_sessions", { path, paths, claudeDir, codexDir }),
};

/** Run one AI request with a busy note; setup errors open AI Preferences. */
async function withAi<T>(label: string, f: () => Promise<T>): Promise<T | null> {
  const s = useStore.getState();
  s.setBusy(label);
  try {
    return await f();
  } catch (e) {
    const msg = String(e);
    s.toast(false, msg);
    if (msg.includes("Preferences > AI")) s.setDialog({ kind: "prefs", section: "ai" });
    return null;
  } finally {
    s.setBusy(null);
  }
}

async function withReviewedFiles<T>(path: string, kind: "commit" | "stash" | "compose", label: string, run: (preview: number | null) => Promise<T>) {
  const review = await withAi("Checking which files to share…", () => invoke<{ id: number | null; files: string[] }>("ai_file_preview", { path, kind }));
  if (!review) return null;
  if (review.id !== null && !await new Promise<boolean>(resolve => {
    useStore.getState().setDialog({
      kind: "choice",
      title: "Send these untracked files to AI?",
      text: review.files.map(file => JSON.stringify(file)).join("\n"),
      onCancel: () => resolve(false),
      choices: [{ label: "Send and continue", variant: "primary", onSelect: () => resolve(true) }],
    });
  })) return null;
  return withAi(label, () => run(review.id));
}

export async function generateCommitMessage(path: string) {
  const before = useStore.getState().drafts[path];
  const r = await withReviewedFiles(path, "commit", "Writing a commit message…", preview => ai.commitMessage(path, undefined, preview));
  if (!r) return;
  const state = useStore.getState();
  if (state.drafts[path] !== before) {
    state.setDialog({ kind: "choice", title: "Generated commit message", text: `${r[0]}\n\n${r[1]}\n\nYour draft changed while this was generated.`,
      choices: [{ label: "Use generated message", onSelect: () => state.setDraft(path, { summary: r[0], description: r[1] }) }] });
    return;
  }
  state.setDraft(path, { summary: r[0], description: r[1] });
}

export async function generateStashTitle(path: string): Promise<string | null> {
  return withReviewedFiles(path, "stash", "Naming the stash…", preview => ai.stashMessage(path, preview));
}

export async function explainCommit(path: string, sha: string, summary: string) {
  const text = await withAi("Explaining the commit…", () => ai.explainCommit(path, sha));
  if (text) useStore.getState().setDialog({ kind: "info", title: `Explain: ${summary}`, text });
}

export async function explainBranch(path: string, branch: string) {
  const text = await withAi(`Explaining ${branch}…`, () => ai.explainBranch(path, branch));
  if (text) useStore.getState().setDialog({ kind: "info", title: `What ${branch} changes`, text });
}

/** "Compose with AI": suggest focused commits for the uncommitted work, then create them after a confirm. */
export async function composeCommits(path: string) {
  const commits = await withReviewedFiles(path, "compose", "Grouping your changes into commits…", preview => ai.compose(path, preview));
  if (!commits) return;
  const text = commits.map((c, i) => `${i + 1}. ${c.summary}\n${c.files.map((f) => `     ${f}`).join("\n")}`).join("\n\n");
  useStore.getState().setDialog({
    kind: "choice",
    title: `Create ${commits.length} commit${commits.length === 1 ? "" : "s"}?`,
    text: `${text}\n\nYou can undo this with Undo.`,
    choices: [{ label: `Create ${commits.length} Commit${commits.length === 1 ? "" : "s"}`, variant: "primary", onSelect: () => void act(path, { type: "composeCommits", commits }) }],
  });
}

/** Save uncommitted changes (no shas) or commits as a .patch file. */
export async function savePatch(path: string, shas: string[], name: string) {
  const s = useStore.getState();
  try {
    const file = await ai.savePatch(path, shas, name);
    if (file) s.toast(true, `Patch saved to ${file}`);
  } catch (e) {
    s.toast(false, String(e));
  }
}

export async function applyPatch(path: string) {
  const file = await ai.pickPatch();
  if (file) await act(path, { type: "applyPatchFile", file });
}

export const patchName = (summary: string) =>
  (summary.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 52) || "changes") + ".patch";
