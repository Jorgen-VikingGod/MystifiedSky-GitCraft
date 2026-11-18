// Loaded only by the explicit --screenshot ... --shot-view validation test mode.
import { api } from "./api";
import { useStore } from "./store";
import { act, openRepoPath, repoData, loadMoreRepo, readRepoSnapshot, queryClient, releaseRepo } from "./repo";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { runInTerminal } from "./terminal-launch";
export function install() {
  Object.assign(window, { __gitcraftVerification: { api, store: useStore, act, openRepoPath, repoData, loadMoreRepo, readRepoSnapshot, queryClient, releaseRepo, invoke, listen, runInTerminal,
    editor: () => import("./monaco"), updates: () => import("./updates") } });
}
