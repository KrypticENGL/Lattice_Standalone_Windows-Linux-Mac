import { useCallback, useEffect, useRef, useState } from "react";
import type { CursorPosition } from "../editor/CodeEditor";
import { VisualizationCanvas } from "../visualization/VisualizationCanvas";
import { Panel } from "../ui/Panel";
import { Sidebar } from "../ui/Sidebar";
import { StatusBar } from "../ui/StatusBar";
import { Toolbar } from "../ui/Toolbar";
import { ViewPlaceholder } from "../ui/ViewPlaceholder";
import { getAppInfo, isNative, type AppInfo } from "../utils/native";
import { observationStatus, type ObservationStatus, type RuntimeSession } from "../runtime";
import { useExecution } from "../runtime/useExecution";
import { useRecording } from "../runtime/useRecording";
import { useInspector } from "../visualization/useInspector";
import { statusText } from "../runtime/status";
import { useProject, type Project } from "../project/useProject";
import { Dialog } from "../ui/Dialogs";
import { ContextMenuProvider, SaveArea } from "../ui/ContextMenu";
import { MenuBar } from "../ui/MenuBar";
import { SolutionExplorer } from "../ui/SolutionExplorer";
import { useClangd } from "../lsp/useClangd";
import { ClangdDialog } from "../ui/ClangdDialog";
import { EditorView } from "./EditorView";
import { views, type ViewId } from "./views";

export function App() {
  const [view, setView] = useState<ViewId>("editor");
  const [explorerOpen, setExplorerOpen] = useState(false);
  const closeExplorer = useCallback(() => setExplorerOpen(false), []);
  // Project is a slide-in drawer over the current view; every other entry is a page.
  const selectView = useCallback((id: ViewId) => {
    if (id === "project") setExplorerOpen((o) => !o);
    else setView(id);
  }, []);
  const [cursor, setCursor] = useState<CursorPosition>({ line: 1, column: 1 });
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [clangdDialog, setClangdDialog] = useState(false);
  const language = useClangd();
  const [observeWanted, setObserveWanted] = useObservePreference();
  const obsStatus = useObservationStatus();
  const observeUnavailable = obsStatus && !obsStatus.available ? (obsStatus.hint ?? "Observation is not available.") : null;
  const observe = observeWanted && observeUnavailable === null;

  // The project and the execution hook depend on each other (a run reads the project's files;
  // opening a solution adopts its recording as the current session), so route through a ref.
  const adoptRef = useRef<(s: RuntimeSession | null) => void>(() => {});
  const adoptSession = useCallback((s: RuntimeSession | null) => adoptRef.current(s), []);
  const sessionIdRef = useRef<string | null>(null);
  const projectRef = useRef<Project>(null as unknown as Project);
  const project = useProject({ observe: observeWanted, setObserve: setObserveWanted, getSessionId: () => sessionIdRef.current, adoptSession });
  const getSolution = useCallback(() => ({ name: projectRef.current.title, path: projectRef.current.path }), []);
  const exec = useExecution(project.getFiles, observe, getSolution);
  adoptRef.current = exec.adopt;
  projectRef.current = project;
  sessionIdRef.current = exec.session?.observation && !exec.session.observation.skipped ? exec.session.id : null;
  const inspector = useInspector();
  const recording = useRecording(exec.session, inspector.focus);
  // A selection belongs to one recording: a new run starts with the inspector closed.
  const sessionId = exec.session?.id;
  useEffect(() => inspector.close(), [sessionId, inspector.close]);

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

  // Ctrl+S saves the solution, Ctrl+Shift+S saves as, Ctrl+O opens one, Ctrl+N starts a new one.
  const { save, saveAs, openFile, newSolution } = project;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey) return;
      const k = e.key.toLowerCase();
      if (k === "s") {
        e.preventDefault();
        void (e.shiftKey ? saveAs() : save());
      } else if (k === "o" && !e.shiftKey) {
        e.preventDefault();
        void openFile();
      } else if (k === "n" && !e.shiftKey) {
        e.preventDefault();
        void newSolution();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [save, saveAs, openFile, newSolution]);

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
        selectView(target.id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selectView]);

  const other = views.find((v) => v.id === view && v.description);

  return (
    <ContextMenuProvider>
    <div className="app">
      <MenuBar project={project} enabled={isNative()} />
      <Toolbar
        project={project}
        running={exec.running}
        onRun={() => void exec.run()}
        onStop={exec.stop}
        observe={observe}
        onObserveChange={setObserveWanted}
        observeUnavailable={observeUnavailable}
      />
      <div className="body">
        <Sidebar active={view} projectOpen={explorerOpen} onSelect={selectView} />
        <main className="content">
          {/* Kept mounted (hidden) so editor state survives view switches. */}
          <div className="view" hidden={view !== "editor"}>
            <EditorView onCursorChange={setCursor} project={project} session={exec.session} recording={recording} inspector={inspector} runError={exec.localError} language={language} onConfigureClangd={() => setClangdDialog(true)} />
          </div>
          {view === "visualizer" && (
            <SaveArea className="view workspace-single" project={project}>
              <Panel title="Visualization"><VisualizationCanvas recording={recording} inspector={inspector} /></Panel>
            </SaveArea>
          )}
          {other && (
            <div className="view">
              <ViewPlaceholder title={other.label} description={other.description!} />
            </div>
          )}
        </main>
        <SolutionExplorer project={project} open={explorerOpen} onClose={closeExplorer} refreshKey={exec.session?.id ?? null} />
      </div>
      <StatusBar line={cursor.line} column={cursor.column} info={info} status={statusText(exec.session, exec.running)} language={language} onLanguageClick={() => setClangdDialog(true)} />
      {project.dialog && <Dialog spec={project.dialog.spec} onDone={project.dialog.done} />}
      {clangdDialog && <ClangdDialog onClose={() => setClangdDialog(false)} />}
    </div>
    </ContextMenuProvider>
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
