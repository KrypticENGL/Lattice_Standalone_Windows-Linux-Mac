import Editor from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { LATTICE_THEME } from "./monacoSetup";
import { placeholderCode } from "./placeholderCode";

export interface CursorPosition {
  line: number;
  column: number;
}

interface Props {
  onCursorChange: (pos: CursorPosition) => void;
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
};

export function CodeEditor({ onCursorChange }: Props) {
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
      }}
    />
  );
}
