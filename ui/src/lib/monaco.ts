// Monaco (the code editor) bundled locally — no network needed.
import * as monaco from "monaco-editor/esm/vs/editor/editor.api";
import "monaco-editor/esm/vs/editor/browser/coreCommands.js";
import "monaco-editor/esm/vs/editor/browser/widget/codeEditor/codeEditorWidget.js";
import "monaco-editor/esm/vs/editor/browser/widget/diffEditor/diffEditor.contribution.js";
import "monaco-editor/esm/vs/editor/contrib/clipboard/browser/clipboard.js";
import "monaco-editor/esm/vs/editor/contrib/find/browser/findController.js";
import "monaco-editor/esm/vs/editor/contrib/contextmenu/browser/contextmenu.js";
import "monaco-editor/esm/vs/editor/contrib/folding/browser/folding.js";
import "monaco-editor/esm/vs/editor/contrib/bracketMatching/browser/bracketMatching.js";
import "monaco-editor/esm/vs/editor/contrib/comment/browser/comment.js";
import "monaco-editor/esm/vs/editor/contrib/cursorUndo/browser/cursorUndo.js";
import "monaco-editor/esm/vs/editor/contrib/indentation/browser/indentation.js";
import "monaco-editor/esm/vs/editor/contrib/linesOperations/browser/linesOperations.js";
import "monaco-editor/esm/vs/editor/contrib/lineSelection/browser/lineSelection.js";
import "monaco-editor/esm/vs/editor/contrib/multicursor/browser/multicursor.js";
import "monaco-editor/esm/vs/editor/contrib/wordOperations/browser/wordOperations.js";
import "monaco-editor/esm/vs/editor/contrib/readOnlyMessage/browser/contribution.js";
import "monaco-editor/esm/vs/editor/contrib/tokenization/browser/tokenization.js";
import "monaco-editor/esm/vs/editor/contrib/longLinesHelper/browser/longLinesHelper.js";
import "monaco-editor/esm/vs/editor/contrib/unicodeHighlighter/browser/unicodeHighlighter.js";
import "monaco-editor/esm/vs/editor/contrib/fontZoom/browser/fontZoom.js";
import "monaco-editor/esm/vs/base/browser/ui/codicons/codiconStyles.js";
import "monaco-editor/esm/vs/basic-languages/monaco.contribution";
import "monaco-editor/esm/vs/language/json/monaco.contribution";
import { registerEditorTheme } from "./editor-theme";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";

declare global {
  interface Window {
    MonacoEnvironment?: monaco.Environment;
  }
}

// Basic grammars share the editor worker; JSON diagnostics use their own worker.
self.MonacoEnvironment = { getWorker: (_module, label) => label === "json" ? new JsonWorker() : new EditorWorker() };

// Dark theme with GitCraft's colours (added-line / deleted-line = green / red at 20 %).
monaco.editor.defineTheme("gitcraft-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#1c1e23",
    "editorGutter.background": "#1c1e23",
    "editorLineNumber.foreground": "#ffffff55",
    "editor.lineHighlightBackground": "#ffffff08",
    "diffEditor.insertedLineBackground": "#5cb85c33",
    "diffEditor.removedLineBackground": "#d9413d33",
    "diffEditor.insertedTextBackground": "#5cb85c40",
    "diffEditor.removedTextBackground": "#d9413d40",
    "diffEditorGutter.insertedLineBackground": "#5cb85c40",
    "diffEditorGutter.removedLineBackground": "#d9413d40",
    "editorOverviewRuler.border": "#00000000",
    "scrollbarSlider.background": "#ffffff26",
  },
});

monaco.editor.defineTheme("gitcraft-light", {
  base: "vs",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#fcfcfc",
    "editorGutter.background": "#fcfcfc",
    "diffEditor.insertedLineBackground": "#5cb85c33",
    "diffEditor.removedLineBackground": "#d9413d33",
  },
});

/** The editor theme for GitCraft's current theme. */
export function editorTheme(): string {
  return (document.documentElement.dataset.theme ?? "dark").startsWith("light") ? "gitcraft-light" : "gitcraft-dark";
}

/** Monaco language id for a file name, from the languages Monaco knows. */
export function languageFor(file: string): string {
  file = file.split("#")[0]; // views add "#<sha>" to tell versions apart
  const name = file.split("/").pop()!.toLowerCase();
  const ext = name.includes(".") ? "." + name.split(".").pop() : name;
  for (const l of monaco.languages.getLanguages()) {
    if (l.extensions?.some((e) => e.toLowerCase() === ext) || l.filenames?.some((f) => f.toLowerCase() === name)) return l.id;
  }
  return "plaintext";
}

registerEditorTheme(() => monaco.editor.setTheme(editorTheme()));

export const EDITOR_FONT = "'Cascadia Mono', Consolas, 'SF Mono', Menlo, 'DejaVu Sans Mono', 'Liberation Mono', ui-monospace, monospace";

export { monaco };
