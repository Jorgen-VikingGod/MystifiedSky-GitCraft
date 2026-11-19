// Monaco (the code editor) bundled locally — no network needed.
import * as monaco from "monaco-editor/editor";
import "monaco-editor/editor/browser/coreCommands";
import "monaco-editor/features/codeEditor/register";
import "monaco-editor/features/diffEditor/register";
import "monaco-editor/features/clipboard/register";
import "monaco-editor/features/find/register";
import "monaco-editor/features/contextmenu/register";
import "monaco-editor/features/folding/register";
import "monaco-editor/features/documentSymbols/register";
import "monaco-editor/features/bracketMatching/register";
import "monaco-editor/features/comment/register";
import "monaco-editor/features/cursorUndo/register";
import "monaco-editor/features/indentation/register";
import "monaco-editor/features/linesOperations/register";
import "monaco-editor/features/lineSelection/register";
import "monaco-editor/features/multicursor/register";
import "monaco-editor/features/wordOperations/register";
import "monaco-editor/features/readOnlyMessage/register";
import "monaco-editor/features/tokenization/register";
import "monaco-editor/features/longLinesHelper/register";
import "monaco-editor/features/unicodeHighlighter/register";
import "monaco-editor/features/fontZoom/register";
import "monaco-editor/features/codicon/register";
import "monaco-editor/basic-languages/monaco.contribution";
import "monaco-editor/language/json/monaco.contribution";
import { registerEditorTheme } from "./editor-theme";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";

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
