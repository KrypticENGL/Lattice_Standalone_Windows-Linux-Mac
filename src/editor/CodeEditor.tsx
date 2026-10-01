import { useEffect, useRef } from "react";
import Editor from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { clangd } from "../lsp/controller";
import { LATTICE_THEME } from "./monacoSetup";
import { useContextMenu, type MenuItem } from "../ui/ContextMenu";

export interface CursorPosition {
  line: number;
  column: number;
}

interface Props {
  onCursorChange: (pos: CursorPosition) => void;
  /** The file being edited: its name (for clangd) and its text when the editor opens. */
  fileName: string;
  initialValue: string;
  onChange: (text: string) => void;
  /** Offered in the editor's own right-click menu as "Save Solution". */
  onSave?: () => void;
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
  contextmenu: false, // the app's own menu is shown instead (see `editMenu`)
};

const readClipboard = () => navigator.clipboard.readText().catch(() => "");

/** Right-click menu: the usual editing commands, then Save Solution. */
function editMenu(ed: editor.IStandaloneCodeEditor, save: (() => void) | undefined): MenuItem[] {
  const sel = ed.getSelection();
  const hasSel = !!sel && !sel.isEmpty();
  const selected = () => (sel ? ed.getModel()?.getValueInRange(sel) ?? "" : "");
  const run = (id: string) => () => {
    ed.focus();
    ed.trigger("menu", id, null);
  };
  const copy = () => void navigator.clipboard.writeText(selected()).catch(() => {});
  return [
    { label: "Undo", keys: "Ctrl+Z", action: run("undo") },
    { label: "Redo", keys: "Ctrl+Y", action: run("redo") },
    { separator: true },
    {
      label: "Cut",
      keys: "Ctrl+X",
      disabled: !hasSel,
      action: () => {
        copy();
        ed.focus();
        ed.executeEdits("menu", [{ range: sel!, text: "", forceMoveMarkers: true }]);
      },
    },
    { label: "Copy", keys: "Ctrl+C", disabled: !hasSel, action: copy },
    {
      label: "Paste",
      keys: "Ctrl+V",
      action: () =>
        void readClipboard().then((text) => {
          if (!text) return;
          ed.focus();
          const range = ed.getSelection();
          if (range) ed.executeEdits("menu", [{ range, text, forceMoveMarkers: true }]);
        }),
    },
    { label: "Select All", keys: "Ctrl+A", action: run("editor.action.selectAll") },
    { separator: true },
    { label: "Format Document", action: run("editor.action.formatDocument") },
    { label: "Command Palette", keys: "F1", action: run("editor.action.quickCommand") },
    { separator: true },
    { label: "Save Solution", keys: "Ctrl+S", action: () => save?.() },
  ];
}

/** One editor per file: remount it (React `key`) to switch files. */
export function CodeEditor({ onCursorChange, fileName, initialValue, onChange, onSave, highlightLine = null }: Props) {
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const show = useContextMenu();
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
      defaultValue={initialValue}
      options={options}
      loading={null}
      onMount={(ed) => {
        ed.onContextMenu((e) => {
          // Put the caret under the pointer unless the click is inside the selection.
          const t = e.target.position;
          const sel = ed.getSelection();
          if (t && !(sel && !sel.isEmpty() && sel.containsPosition(t))) ed.setPosition(t);
          show(e.event.browserEvent, editMenu(ed, saveRef.current));
        });
        editorRef.current = ed;
        marks.current = ed.createDecorationsCollection([]);
        const report = () => {
          const p = ed.getPosition();
          if (p) onCursorChange({ line: p.lineNumber, column: p.column });
        };
        ed.onDidChangeCursorPosition(report);
        report();
        ed.onDidChangeModelContent(() => onChange(ed.getValue()));
        // clangd supplies completion, diagnostics, hover and navigation; detaches when the editor is disposed.
        clangd.attach(ed, fileName);
      }}
    />
  );
}
