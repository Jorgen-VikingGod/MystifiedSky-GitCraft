import { useText } from "@/lib/locale";
import { protectEditorDrafts } from "@/lib/editor-drafts";
import { useShallow } from "zustand/react/shallow";
import { EDITORS } from "./prefs/GeneralSections";
import { checkForUpdates, download, installAndRestart, RELEASES_URL, updateMenuItems, useUpdates } from "@/lib/updates";
import { hosting } from "@/lib/hosting";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DropMenu, type MenuEntry } from "./ui/menu";
import { useStore } from "@/lib/store";
import { api } from "@/lib/api";
import { act } from "@/lib/repo";
import { signOutAll } from "@/lib/integrations";
import { isMac, MOD } from "@/lib/utils";
import { resetZoom, stepZoom } from "@/lib/zoom";

const win = () => getCurrentWindow();

/** Leave the macOS title bar clear for the native window controls. */
export function TitleBar() {
  return isMac ? <div className="h-[30px] bg-titlebar shrink-0" data-tauri-drag-region /> : null;
}

export function MenuBar() {
  const t = useText();
  const s = useStore(useShallow((s) => ({ tabs: s.tabs, activeId: s.activeId, settings: s.settings, closed: s.closed, closeTab: s.closeTab, newTab: s.newTab, reopenTab: s.reopenTab, setDialog: s.setDialog, setPalette: s.setPalette, setSettings: s.setSettings, toast: s.toast })));
  const tab = s.tabs.find((t) => t.id === s.activeId);
  const repoPath = tab?.kind === "repo" ? tab.path : undefined;
  const set = s.settings;
  const up = useUpdates();

  const file: MenuEntry[] = [
    // File menu order.
    { label: "New Tab", shortcut: `${MOD}+T`, onSelect: s.newTab },
    { label: "Close Tab", shortcut: `${MOD}+W`, onSelect: () => s.closeTab(s.activeId) },
    { label: "Reopen Closed Tab", shortcut: `${MOD}+Shift+T`, disabled: s.closed.length === 0, onSelect: s.reopenTab },
    { separator: true },
    { label: "Clone Repo...", shortcut: `${MOD}+N`, onSelect: () => s.setDialog({ kind: "clone" }) },
    { label: "Init Repo...", shortcut: `${MOD}+I`, onSelect: () => s.setDialog({ kind: "init" }) },
    { label: "Open Repo...", shortcut: `${MOD}+O`, onSelect: () => s.setDialog({ kind: "open" }) },
    { label: "Open Repo in New Window…", onSelect: async () => {
      try {
        const path = repoPath ?? await api.pickFolder("Choose a repository to open in a new window");
        if (path) await api.openRepositoryWindow(path);
      } catch (error) { s.toast(false, String(error)); }
    } },
    { label: "Open Repo Management", shortcut: `Alt+${MOD}+O`, onSelect: () => s.setDialog({ kind: "repoManagement" }) },
    { label: `Open Repo in ${editorName(set.externalEditor)}`, shortcut: `${MOD}+Shift+E`, disabled: !repoPath, onSelect: () => repoPath && api.openInEditor(repoPath) },
    { separator: true },
    { label: "Open External Terminal", shortcut: "Alt+T", disabled: !repoPath, onSelect: () => repoPath && api.openTerminal(repoPath) },
    { label: "Open in File Manager", shortcut: "Alt+O", disabled: !repoPath, onSelect: () => repoPath && api.openFileManager(repoPath) },
    { separator: true },
    { label: "File History / Blame...", shortcut: `${MOD}+Shift+H`, disabled: !repoPath, onSelect: () => s.setDialog({ kind: "fileFinder" }) },
    { separator: true },
    { label: "Preferences...", shortcut: `${MOD}+,`, onSelect: () => s.setDialog({ kind: "prefs" }) },
    { separator: true },
    // Update lines (only those that fit the current update state).
    ...updateMenuItems(up).map((item) => ({
      label: item.label,
      disabled: item.disabled,
      onSelect: () => (item.run === "install" ? installAndRestart() : item.run === "download" ? download() : item.run === "check" ? checkForUpdates(true) : undefined),
    })),
    {
      // Sign out signs out at once and shows a "Successfully signed out" toast.
      // GitCraft has no separate account, so signing out removes every saved service token.
      label: "Sign Out",
      onSelect: () => signOutAll().then(() => s.toast(true, "Successfully signed out"), (e) => s.toast(false, String(e))),
    },
    { label: "Exit", shortcut: "Alt+F4", onSelect: () => win().close() },
  ];
  const edit: MenuEntry[] = [
    { label: "Undo", shortcut: `${MOD}+Z`, disabled: !repoPath, onSelect: () => repoPath && act(repoPath, { type: "undo" }) },
    { label: "Redo", shortcut: `${MOD}+Y`, disabled: !repoPath, onSelect: () => repoPath && act(repoPath, { type: "redo" }) },
    { separator: true },
    { label: "Cut", shortcut: `${MOD}+X`, onSelect: () => document.execCommand("cut") },
    { label: "Copy", shortcut: `${MOD}+C`, onSelect: () => document.execCommand("copy") },
    { label: "Paste", shortcut: `${MOD}+V`, onSelect: () => navigator.clipboard.readText().then((t) => document.execCommand("insertText", false, t)) },
    { label: "Select All", shortcut: `${MOD}+A`, onSelect: () => document.execCommand("selectAll") },
  ];
  const view: MenuEntry[] = [
    { label: `${set.showLeft ? "✓ " : "   "}Show Left Panel`, shortcut: `${MOD}+J`, onSelect: () => s.setSettings({ showLeft: !set.showLeft }) },
    { label: `${set.showRight ? "✓ " : "   "}Show Commit Details Panel`, shortcut: `${MOD}+K`, onSelect: () => s.setSettings({ showRight: !set.showRight }) },
    { label: `${set.toolbarLabels ? "✓ " : "   "}Show Toolbar Labels`, onSelect: () => s.setSettings({ toolbarLabels: !set.toolbarLabels }) },
    { label: `${set.showTerminal ? "✓ " : "   "}Show Terminal Panel`, shortcut: `${MOD}+\``, onSelect: () => s.setSettings({ showTerminal: !set.showTerminal }) },
    { separator: true },
    { label: "Increase Zoom", shortcut: `${MOD}+=`, onSelect: () => stepZoom(1) },
    { label: "Decrease Zoom", shortcut: `${MOD}+-`, onSelect: () => stepZoom(-1) },
    { label: "Reset Zoom", shortcut: `${MOD}+0`, onSelect: resetZoom },
    { separator: true },
    { label: "Toggle Full Screen", shortcut: "F11", onSelect: async () => win().setFullscreen(!(await win().isFullscreen())) },
    { label: "Activity Log", onSelect: () => s.setDialog({ kind: "log" }) },
    { label: "Reload Window", onSelect: () => { if (!protectEditorDrafts(useStore.getState().setDialog, () => location.reload())) location.reload(); } },
  ];
  const help: MenuEntry[] = [
    { label: "Command Palette", shortcut: `${MOD}+Shift+P`, onSelect: () => s.setPalette("root") },
    { label: "Keyboard Shortcuts", shortcut: `${MOD}+/`, onSelect: () => s.setDialog({ kind: "shortcuts" }) },
    { label: "Feature Plan", onSelect: () => s.setDialog({ kind: "plan" }) },
    { label: "Copy diagnostics", onSelect: async () => {
      const { toast } = useStore.getState();
      try {
        await navigator.clipboard.writeText(await api.diagnostics());
        toast(true, "Diagnostics copied");
      } catch (error) { toast(false, String(error)); }
    } },
    { label: "View Activity Logs", onSelect: () => s.setDialog({ kind: "log" }) },
    { separator: true },
    { label: "View Release Notes", onSelect: () => hosting.openUrl(RELEASES_URL) },
    { label: "Join Discord", onSelect: () => hosting.openUrl("https://discord.gg/BsUgBu65YG") },
    { label: "About GitCraft", onSelect: () => s.setDialog({ kind: "about" }) },
  ];

  return (
    <div className="h-[26px] bg-menubar flex items-center px-0.5 shrink-0 text-[13px] text-white/90">
      {(
        [
          ["File", file],
          ["Edit", edit],
          ["View", view],
          ["Help", help],
        ] as const
      ).map(([name, items]) => (
        <DropMenu
          key={name}
          items={items.map(item => item.label ? { ...item, label: t(item.label) } : item)}
          trigger={<button className="px-[7px] h-[22px] rounded-[3px] hover:bg-white/10 data-[state=open]:bg-white/10 outline-none">{t(name)}</button>}
        />
      ))}
    </div>
  );
}

/** "Open Repo in {0}" (menuStrings OpenRepoInNamedExternalEditor). */
export function editorName(tool: string) {
  return EDITORS.find(([id]) => id === tool)?.[1] ?? "External Editor";
}
