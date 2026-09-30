// Bundle Monaco locally (no CDN) so the app works offline. Language grammars
// are registered lazily by Monaco, so only C++ is actually loaded.
import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.main";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

export const LATTICE_THEME = "lattice-dark";

monaco.editor.defineTheme(LATTICE_THEME, {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "keyword", foreground: "8ec5ff" },
    { token: "string", foreground: "e6d98a" },
    { token: "number", foreground: "c9e265" },
    { token: "comment", foreground: "5b6b7c", fontStyle: "italic" },
    { token: "delimiter.angle", foreground: "9aa7b5" },
    { token: "keyword.directive", foreground: "d69be8" },
  ],
  colors: {
    "editor.background": "#0b1118",
    "editor.lineHighlightBackground": "#101922",
    "editorLineNumber.foreground": "#3f4d5c",
    "editorLineNumber.activeForeground": "#9aa7b5",
    "editorCursor.foreground": "#c9e265",
    "editor.selectionBackground": "#26384a",
    "editorIndentGuide.background1": "#17212b",
  },
});

loader.config({ monaco });
