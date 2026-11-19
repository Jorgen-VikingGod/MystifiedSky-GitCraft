// Stage 4 dialogs: remotes, worktrees, submodules, Gitflow, branch clean-up.
import { api, type RepoView } from "./api";
import { act, confirmThen, openRepoPath } from "./repo";
import { useStore, type FormField } from "./store";
import { sameFolder } from "./agents";
import { protectEditorDrafts } from "./editor-drafts";

const form = (title: string, fields: FormField[], confirm: string, onSubmit: (v: Record<string, string | boolean>) => void, help?: string) =>
  useStore.getState().setDialog({ kind: "form", title, fields, confirm, onSubmit, help });

const str = (v: string | boolean | undefined) => (typeof v === "string" ? v.trim() : "");

export async function fetchMoreHistory(path: string) {
  const state = useStore.getState();
  try {
    const info = await api.historyFetchInfo(path);
    if (!info.remotes.length) { state.toast(false, "Add a remote before fetching history."); return; }
    const options = [
      ...(info.shallow ? [{ label: "Fetch 500 more commits", mode: "deepen" }, { label: "Fetch all history", mode: "unshallow" }] as const : []),
      ...(info.promisor_remotes.length ? [{ label: "Download filtered objects", mode: "objects" }] as const : []),
    ];
    if (!options.length) { state.toast(true, "This repository already has complete history."); return; }
    state.setDialog({
      kind: "choice", title: "Fetch more repository history",
      text: "Choose how much to download. Full history or filtered objects can be large.",
      choices: options.map(({ label, mode }) => ({
        label,
        onSelect: () => {
          const remotes = mode === "objects" ? info.promisor_remotes : info.remotes;
          form(label, [{ key: "remote", label: "Remote", value: remotes[0], options: remotes.map(remote => [remote, remote]) }], "Fetch",
            values => act(path, { type: "fetchHistory", remote: str(values.remote), mode }));
        },
      })),
    });
  } catch (error) { state.toast(false, String(error)); }
}

export function repositoryMaintenance(path: string) {
  useStore.getState().setDialog({
    kind: "choice", title: "Repository maintenance",
    text: "Optimize local Git data, rebuild the commit graph, or clean up stale remote-tracking branches.",
    choices: [
      { label: "Run garbage collection", onSelect: () => confirmThen("Run Git garbage collection?",
        "Git may remove unreachable objects according to its configured expiry. Named branches, tags, and stash entries stay available.",
        "Run maintenance", () => void act(path, { type: "maintainRepository", task: "gc" }), false) },
      { label: "Write commit graph", onSelect: () => void act(path, { type: "maintainRepository", task: "commitGraph" }) },
      { label: "Fetch and prune remotes", onSelect: () => void act(path, { type: "fetch" }) },
    ],
  });
}

export function compareCommitRanges(path: string, before = "", after = "") {
  form("Compare commit ranges", [
    { key: "before", label: "Before", value: before, placeholder: "base..old-tip" },
    { key: "after", label: "After", value: after, placeholder: "base..new-tip" },
  ], "Compare", values => useStore.getState().setDialog({
    kind: "rangeDiff", path, before: str(values.before), after: str(values.after),
  }), "Compare two versions of a series of commits, such as before and after a rebase.");
}

export function addRemote(path: string) {
  useStore.getState().setDialog({ kind: "addRemote", path });
}

export async function editRemote(path: string, name: string) {
  const r = (await api.remotesList(path)).find((x) => x.name === name);
  form(
    "Edit Remote",
    [
      { key: "name", label: "Name", value: name },
      { key: "url", label: "Pull URL", value: r?.fetch_url ?? "" },
      { key: "push", label: "Push URL", value: r?.push_url ?? "" },
    ],
    "Save",
    (v) => act(path, { type: "remoteEdit", name, newName: str(v.name), url: str(v.url), pushUrl: str(v.push) }),
  );
}

export function removeRemote(path: string, name: string) {
  confirmThen(`Remove ${name}?`, "The remote and its remote-tracking branches are removed from this repository. Nothing on the server changes.", "Remove", () =>
    act(path, { type: "remoteRemove", name }),
  );
}

export async function recoverDiscardedChanges(path: string) {
  const state = useStore.getState();
  try {
    const backups = await api.discardedChanges(path);
    if (backups.length === 0) { state.toast(true, "No discarded changes to recover."); return; }
    form("Recover discarded changes", [{ key: "reference", label: "Backup", value: backups[0].reference,
      options: backups.map((backup, index) => [backup.reference, `${new Date(backup.time * 1000).toLocaleString()} · ${index + 1}. ${backup.subject}`]),
    }], "Restore", value => act(path, { type: "restoreDiscard", reference: str(value.reference) }),
      "Restores the files and staging from this backup. If your current changes overlap, Git may refuse the restore or report conflicts.");
  } catch (error) { state.toast(false, String(error)); }
}

export function addWorktree(repo: RepoView, branch?: string, start?: string) {
  const parent = repo.path.replace(/[\\/][^\\/]+$/, "");
  const name = (branch ?? "worktree").replace(/[\\/]/g, "-");
  form(
    "Add a new worktree",
    [
      { key: "path", label: "Folder for the worktree", value: `${parent}${repo.path.includes("\\") ? "\\" : "/"}${repo.name}-${name}`, browse: true },
      { key: "branch", label: "Branch", value: branch ?? "", placeholder: "existing branch, or a new branch name" },
      { key: "new", label: start ? `Create a new branch from ${start.slice(0, 8)}` : "Create this as a new branch from the checked-out commit", value: !branch, type: "checkbox" },
    ],
    "Create Worktree",
    async (v) => {
      const ok = await act(repo.path, { type: "worktreeAdd", path: str(v.path), branch: str(v.branch), newBranch: !!v.new, start: start ?? null });
      if (ok) await openRepoPath(str(v.path));
    },
    "A worktree is a second working folder for the same repository, with its own branch checked out.",
  );
}

export function lockWorktree(path: string, wt: string) {
  form("Lock worktree", [{ key: "msg", label: "Message (optional):", value: "", placeholder: "Lock message" }], "Lock", (v) => act(path, { type: "worktreeLock", path: wt, message: str(v.msg) }));
}

export function removeWorktree(path: string, wt: string) {
  const worktreeTab = () => useStore.getState().tabs.find(tab => tab.path && sameFolder(tab.path, wt));
  const remove = async (force: boolean) => {
    const state = useStore.getState();
    const tab = worktreeTab();
    if (tab && protectEditorDrafts(state.setDialog, () => void remove(force), tab.id)) return;
    if (await act(path, { type: "worktreeRemove", path: wt, force })) {
      if (tab) state.closeTab(tab.id);
      return;
    }
    if (force) return;
    const status = await api.repoStatus(wt).catch(() => null);
    if (!status) return;
    const untracked = new Set(status.unstaged.filter(file => file.status === "?").map(file => file.path));
    const tracked = new Set([...status.staged, ...status.unstaged].filter(file => file.status !== "?").map(file => file.path));
    if (!tracked.size && !untracked.size) return;
    confirmThen("Delete the worktree and its changes?",
      `${wt}\n\nThis deletes the working folder, including ${tracked.size} changed tracked and ${untracked.size} untracked files. The branch stays.`,
      "Delete worktree and changes", () => void remove(true));
  };
  confirmThen("Remove worktree?", `Remove the worktree at ${wt}? Its folder is deleted; its branch stays.`, "Remove", () => void remove(false));
}

export function addSubmodule(path: string) {
  form(
    "Add Submodule",
    [
      { key: "url", label: "Remote URL", value: "", placeholder: "https://github.com/owner/library.git" },
      { key: "path", label: "Path", value: "", placeholder: "libs/library" },
    ],
    "Add Submodule",
    (v) => act(path, { type: "submoduleAdd", url: str(v.url), path: str(v.path) }),
  );
}

export async function initGitflow(path: string) {
  const c = await api.gitflowConfig(path);
  form(
    c.initialized ? "Update Gitflow Config" : "Initialize Gitflow",
    [
      { key: "master", label: "Main/Master", value: c.master },
      { key: "develop", label: "Develop", value: c.develop },
      { key: "feature", label: "Feature prefix", value: c.feature },
      { key: "release", label: "Release prefix", value: c.release },
      { key: "hotfix", label: "Hotfix prefix", value: c.hotfix },
      { key: "versiontag", label: "Version Tag prefix", value: c.versiontag },
    ],
    c.initialized ? "Update Gitflow Config" : "Initialize Gitflow",
    (v) =>
      act(path, {
        type: "gitflowInit",
        config: { master: str(v.master), develop: str(v.develop), feature: str(v.feature), release: str(v.release), hotfix: str(v.hotfix), versiontag: str(v.versiontag), initialized: true },
      }),
    c.initialized ? "Changing these prefixes will cause any existing branches with different prefixes to be ignored by Gitflow." : undefined,
  );
}

export function startGitflow(path: string, kind: "feature" | "release" | "hotfix") {
  const label = { feature: "Feature", release: "Release", hotfix: "Hotfix" }[kind];
  form(`Start ${label}`, [{ key: "name", label: kind === "feature" ? "Feature name" : `${label} version`, value: "" }], `Start ${label}`, (v) =>
    act(path, { type: "gitflowStart", kind, name: str(v.name) }),
  );
}

export function finishGitflow(path: string, kind: "feature" | "release" | "hotfix", name: string) {
  const label = { feature: "Feature", release: "Release", hotfix: "Hotfix" }[kind];
  const fields: FormField[] = [{ key: "delete", label: "Delete branch", value: true, type: "checkbox" }];
  if (kind !== "feature") fields.push({ key: "tag", label: "Tag message", value: "", placeholder: "tag message" });
  form(`Finish ${label}: ${name}`, fields, `Finish ${label}`, (v) => act(path, { type: "gitflowFinish", kind, name, keep: !v.delete, tagMessage: str(v.tag) }));
}

export function openCleanup() {
  useStore.getState().setDialog({ kind: "cleanup" });
}
