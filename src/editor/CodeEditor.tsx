import { useEffect, useRef } from "react";
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
  /** Highlight this line (the one an observed run is at); null clears it. */
  highlightLine?: number | null;
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

export function CodeEditor({ onCursorChange, onReady, highlightLine = null }: Props) {
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const marks = useRef<editor.IEditorDecorationsCollection | null>(null);

  useEffect(() => {
    const ed = editorRef.current;
    const collection = marks.current;
    if (!ed || !collection) return;
    if (highlightLine) {
      collection.set([
        {
          range: { startLineNumber: highlightLine, startColumn: 1, endLineNumber: highlightLine, endColumn: 1 },
          options: { isWholeLine: true, className: "exec-line", linesDecorationsClassName: "exec-line-gutter" },
        },
      ]);
      ed.revealLineInCenterIfOutsideViewport(highlightLine);
    } else {
      collection.set([]);
    }
  }, [highlightLine]);

  return (
    <Editor
      language="cpp"
      theme={LATTICE_THEME}
      defaultValue={placeholderCode}
      options={options}
      loading={null}
      onMount={(ed) => {
        editorRef.current = ed;
        marks.current = ed.createDecorationsCollection([]);
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
