import { useCallback, useEffect, useRef, useState } from "react";
import type { CursorPosition } from "../editor/CodeEditor";
import { VisualizationCanvas } from "../visualization/VisualizationCanvas";
import { Panel } from "../ui/Panel";
import { Sidebar } from "../ui/Sidebar";
import { StatusBar } from "../ui/StatusBar";
import { Toolbar } from "../ui/Toolbar";
import { ViewPlaceholder } from "../ui/ViewPlaceholder";
import { getAppInfo, isNative, type AppInfo } from "../utils/native";
import { observationStatus, type ObservationStatus } from "../runtime";
import { useExecution } from "../runtime/useExecution";
import { useRecording } from "../runtime/useRecording";
import { statusText } from "../runtime/status";
import { appConfig } from "../config/appConfig";
import { useClangd } from "../lsp/useClangd";
import { ClangdDialog } from "../ui/ClangdDialog";
import { EditorView } from "./EditorView";
import { views, type ViewId } from "./views";

export function App() {
  const [view, setView] = useState<ViewId>("editor");
  const [cursor, setCursor] = useState<CursorPosition>({ line: 1, column: 1 });
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [clangdDialog, setClangdDialog] = useState(false);
  const language = useClangd();
  const [observeWanted, setObserveWanted] = useObservePreference();
  const obsStatus = useObservationStatus();
  const observeUnavailable = obsStatus && !obsStatus.available ? (obsStatus.hint ?? "Observation is not available.") : null;
  const observe = observeWanted && observeUnavailable === null;

  const getValueRef = useRef<() => string>(() => "");
  const getSource = useCallback(() => getValueRef.current(), []);
  const onEditorReady = useCallback((fn: () => string) => {
    getValueRef.current = fn;
  }, []);
  const exec = useExecution(getSource, appConfig.defaultFileName, observe);
  const recording = useRecording(exec.session);

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
      <Toolbar
        running={exec.running}
        onRun={() => void exec.run()}
        onStop={exec.stop}
        observe={observe}
        onObserveChange={setObserveWanted}
        observeUnavailable={observeUnavailable}
      />
      <div className="body">
        <Sidebar active={view} onSelect={setView} />
        <main className="content">
          {/* Kept mounted (hidden) so editor state survives view switches. */}
          <div className="view" hidden={view !== "editor"}>
            <EditorView onCursorChange={setCursor} onEditorReady={onEditorReady} session={exec.session} recording={recording} runError={exec.localError} language={language} onConfigureClangd={() => setClangdDialog(true)} />
          </div>
          {view === "visualizer" && (
            <div className="view workspace-single">
              <Panel title="Visualization"><VisualizationCanvas recording={recording} /></Panel>
            </div>
          )}
          {other && (
            <div className="view">
              <ViewPlaceholder title={other.label} description={other.description!} />
            </div>
          )}
        </main>
      </div>
      <StatusBar line={cursor.line} column={cursor.column} info={info} status={statusText(exec.session, exec.running)} language={language} onLanguageClick={() => setClangdDialog(true)} />
      {clangdDialog && <ClangdDialog onClose={() => setClangdDialog(false)} />}
    </div>
  );
}

const OBSERVE_KEY = "lattice.observe";

/** Whether the user wants runs observed; remembered across sessions (best effort). */
function useObservePreference(): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState(() => {
    try {
      return localStorage.getItem(OBSERVE_KEY) === "1";
    } catch {
      return false;
    }
  });
  const set = useCallback((value: boolean) => {
    setOn(value);
    try {
      localStorage.setItem(OBSERVE_KEY, value ? "1" : "0");
    } catch {
      /* not persisted; still works for this session */
    }
  }, []);
  return [on, set];
}

/** Whether observation is possible; re-checked when the window regains focus (the user may have installed libclang). */
function useObservationStatus(): ObservationStatus | null {
  const [status, setStatus] = useState<ObservationStatus | null>(null);
  useEffect(() => {
    if (!isNative()) return;
    const refresh = () => void observationStatus().then(setStatus).catch(() => setStatus(null));
    refresh();
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, []);
  return status;
}
