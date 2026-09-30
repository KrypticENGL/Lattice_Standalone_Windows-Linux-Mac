import { useCallback, useState } from "react";
import { CodeEditor, type CursorPosition } from "../editor/CodeEditor";
import "../editor/monacoSetup";
import { VisualizationCanvas } from "../visualization/VisualizationCanvas";
import { OutputPanel } from "../ui/OutputPanel";
import { Panel } from "../ui/Panel";
import { Splitter } from "../ui/Splitter";
import { appConfig } from "../config/appConfig";

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

interface Props {
  onCursorChange: (pos: CursorPosition) => void;
}

/** Editor + visualization + output split. */
export function EditorView({ onCursorChange }: Props) {
  const [editorWidth, setEditorWidth] = useState(45); // % of workspace width
  const [outputHeight, setOutputHeight] = useState(220); // px

  const dragColumns = useCallback((dx: number) => {
    setEditorWidth((w) => clamp(w + (dx / window.innerWidth) * 100, 20, 75));
  }, []);
  const dragRows = useCallback((dy: number) => {
    setOutputHeight((h) => clamp(h - dy, 80, window.innerHeight * 0.6));
  }, []);

  return (
    <div className="workspace" style={{ gridTemplateRows: `minmax(0,1fr) 5px ${outputHeight}px` }}>
      <div className="top-row" style={{ gridTemplateColumns: `${editorWidth}% 5px minmax(0,1fr)` }}>
        <Panel title={appConfig.defaultFileName} className="editor-panel">
          <CodeEditor onCursorChange={onCursorChange} />
        </Panel>
        <Splitter orientation="vertical" onDrag={dragColumns} />
        <Panel title="Visualization" className="viz-panel">
          <VisualizationCanvas />
        </Panel>
      </div>
      <Splitter orientation="horizontal" onDrag={dragRows} />
      <OutputPanel />
    </div>
  );
}
