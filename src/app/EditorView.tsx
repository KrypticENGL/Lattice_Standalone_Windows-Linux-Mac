import { useCallback, useState } from "react";
import { CodeEditor, type CursorPosition } from "../editor/CodeEditor";
import "../editor/monacoSetup";
import { VisualizationCanvas } from "../visualization/VisualizationCanvas";
import type { Inspector } from "../visualization/useInspector";
import { OutputPanel } from "../ui/OutputPanel";
import type { RuntimeSession } from "../runtime";
import type { Recording } from "../runtime/useRecording";
import { Panel } from "../ui/Panel";
import { Splitter } from "../ui/Splitter";
import type { Project } from "../project/useProject";
import { FileTabs } from "../ui/FileTabs";
import { useSaveMenu } from "../ui/ContextMenu";
import type { ClangdSnapshot } from "../lsp/controller";
import { LanguageBanner } from "../ui/LanguageBanner";

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

interface Props {
  onCursorChange: (pos: CursorPosition) => void;
  project: Project;
  session: RuntimeSession | null;
  recording: Recording;
  inspector: Inspector;
  runError: string | null;
  language: ClangdSnapshot;
  onConfigureClangd: () => void;
}

/** Editor + visualization + output split. */
export function EditorView({ onCursorChange, project, session, recording, inspector, runError, language, onConfigureClangd }: Props) {
  const onContextMenu = useSaveMenu(project);
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
      <div className="top-row" onContextMenu={onContextMenu} style={{ gridTemplateColumns: `${editorWidth}% 5px minmax(0,1fr)` }}>
        <Panel className="editor-panel">
          <div className="editor-stack">
            <FileTabs project={project} />
            <LanguageBanner language={language} onConfigure={onConfigureClangd} />
            <div className="editor-fill">
              <CodeEditor
                key={`${project.generation}:${project.active}`}
                fileName={project.active}
                initialValue={project.textOf(project.active)}
                onChange={project.setActiveText}
                onSave={() => void project.save()}
                onCursorChange={onCursorChange}
                highlightLine={recording.available && recording.step > 0 ? (recording.graph?.line ?? null) : null}
              />
            </div>
          </div>
        </Panel>
        <Splitter orientation="vertical" onDrag={dragColumns} />
        <Panel title="Visualization" className="viz-panel">
          <VisualizationCanvas recording={recording} inspector={inspector} />
        </Panel>
      </div>
      <Splitter orientation="horizontal" onDrag={dragRows} />
      <OutputPanel session={session} runError={runError} recording={recording} />
    </div>
  );
}
