import { useLayoutEffect, useRef } from "react";
import type { DialogState, TabUi } from "./store";

type Draft = { file: string; save: () => Promise<boolean>; discard: () => void; saving: () => boolean; forget: () => void };
const drafts = new Map<string, Draft>();

export function useEditorDraft(tabId: string, file: string, dirty: boolean, save: Draft["save"], discard: Draft["discard"], saving: Draft["saving"]) {
  const current = useRef({ save, discard, saving });
  useLayoutEffect(() => { current.current = { save, discard, saving }; });
  useLayoutEffect(() => {
    if (!dirty) return;
    const entry: Draft = {
      file,
      save: () => current.current.save(),
      discard: () => current.current.discard(),
      saving: () => current.current.saving(),
      forget: () => { if (drafts.get(tabId) === entry) drafts.delete(tabId); },
    };
    drafts.set(tabId, entry);
    return entry.forget;
  }, [tabId, file, dirty]);
}

export function editorKey(ui: TabUi) {
  if (ui.pr || ui.issue || ui.rebase) return null;
  if (ui.conflict) return `conflict:${ui.conflict}`;
  if (ui.diff) return `diff:${JSON.stringify(ui.diff)}`;
  return null;
}

export function forgetEditorDraft(tabId: string) {
  drafts.get(tabId)?.forget();
}

/** Return true when the caller must pause until a choice is made. */
export function protectEditorDrafts(show: (dialog: DialogState) => void, proceed: () => void, tabId?: string) {
  const entry = tabId === undefined ? undefined : drafts.get(tabId);
  const pending = tabId === undefined ? [...drafts.values()] : entry ? [entry] : [];
  if (!pending.length) return false;
  if (pending.some(draft => draft.saving())) return true;
  const single = pending.length === 1;
  show({
    kind: "choice",
    title: single ? "Save changes before leaving?" : "Save changes before closing?",
    text: pending.map(entry => entry.file).join("\n"),
    choices: [
      { label: "Discard", variant: "danger", onSelect: () => {
        for (const entry of pending) { entry.discard(); entry.forget(); }
        proceed();
      } },
      { label: "Save", variant: "primary", onSelect: async () => {
        for (const entry of pending) {
          if (!(await entry.save())) return;
          entry.forget();
        }
        proceed();
      } },
    ],
  });
  return true;
}
