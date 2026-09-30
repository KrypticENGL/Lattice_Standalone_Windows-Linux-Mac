import { useCallback, useEffect, useRef, useState } from "react";
import type { CursorPosition } from "../editor/CodeEditor";
import { VisualizationCanvas } from "../visualization/VisualizationCanvas";
import { Panel } from "../ui/Panel";
import { Sidebar } from "../ui/Sidebar";
import { StatusBar } from "../ui/StatusBar";
import { Toolbar } from "../ui/Toolbar";
import { ViewPlaceholder } from "../ui/ViewPlaceholder";
import { getAppInfo, type AppInfo } from "../utils/native";
import { useExecution } from "../runtime/useExecution";
import { statusText } from "../runtime/status";
import { appConfig } from "../config/appConfig";
import { EditorView } from "./EditorView";
import { views, type ViewId } from "./views";

export function App() {
  const [view, setView] = useState<ViewId>("editor");
  const [cursor, setCursor] = useState<CursorPosition>({ line: 1, column: 1 });
  const [info, setInfo] = useState<AppInfo | null>(null);

  const getValueRef = useRef<() => string>(() => "");
  const getSource = useCallback(() => getValueRef.current(), []);
  const onEditorReady = useCallback((fn: () => string) => {
    getValueRef.current = fn;
  }, []);
  const exec = useExecution(getSource, appConfig.defaultFileName);

  // Ctrl+Enter runs (or does nothing while a run is active).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "Enter" && !exec.running) {
        e.preventDefault();
        void exec.run();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [exec.running, exec.run]);

  useEffect(() => {
    void getAppInfo().then(setInfo);
  }, []);

  // Ctrl+1..N switches views.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey || e.shiftKey) return;
      const target = views[Number(e.key) - 1];
      if (target) {
        e.preventDefault();
        setView(target.id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const other = views.find((v) => v.id === view && v.description);

  return (
    <div className="app">
      <Toolbar running={exec.running} onRun={() => void exec.run()} onStop={exec.stop} />
      <div className="body">
        <Sidebar active={view} onSelect={setView} />
        <main className="content">
          {/* Kept mounted (hidden) so editor state survives view switches. */}
          <div className="view" hidden={view !== "editor"}>
            <EditorView onCursorChange={setCursor} onEditorReady={onEditorReady} session={exec.session} runError={exec.localError} />
          </div>
          {view === "visualizer" && (
            <div className="view workspace-single">
              <Panel title="Visualization"><VisualizationCanvas /></Panel>
            </div>
          )}
          {other && (
            <div className="view">
              <ViewPlaceholder title={other.label} description={other.description!} />
            </div>
          )}
        </main>
      </div>
      <StatusBar line={cursor.line} column={cursor.column} info={info} status={statusText(exec.session, exec.running)} />
    </div>
  );
}
