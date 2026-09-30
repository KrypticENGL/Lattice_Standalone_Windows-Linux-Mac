import Editor from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { appConfig } from "../config/appConfig";
import { clangd } from "../lsp/controller";
import { LATTICE_THEME } from "./monacoSetup";
import { placeholderCode } from "./placeholderCode";

export interface CursorPosition {
  line: number;
  column: number;
}

interface Props {
  onCursorChange: (pos: CursorPosition) => void;
  /** Hands the parent a function returning the editor's current text. */
  onReady?: (getValue: () => string) => void;
}

const options: editor.IStandaloneEditorConstructionOptions = {
  fontFamily: "'Cascadia Code', Consolas, 'Courier New', monospace",
  fontSize: 14,
  minimap: { enabled: false },
  scrollBeyondLastLine: false,
  automaticLayout: true,
  tabSize: 4,
  renderLineHighlight: "line",
  padding: { top: 12 },
  smoothScrolling: false,
  fixedOverflowWidgets: true,
  parameterHints: { enabled: true },
};

export function CodeEditor({ onCursorChange, onReady }: Props) {
  return (
    <Editor
      language="cpp"
      theme={LATTICE_THEME}
      defaultValue={placeholderCode}
      options={options}
      loading={null}
      onMount={(ed) => {
        const report = () => {
          const p = ed.getPosition();
          if (p) onCursorChange({ line: p.lineNumber, column: p.column });
        };
        ed.onDidChangeCursorPosition(report);
        report();
        onReady?.(() => ed.getValue());
        // clangd supplies completion, diagnostics, hover and navigation; detaches when the editor is disposed.
        clangd.attach(ed, appConfig.defaultFileName);
      }}
    />
  );
}
