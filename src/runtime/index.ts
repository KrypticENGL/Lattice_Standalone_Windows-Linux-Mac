/**
 * Frontend boundary for program execution. The UI talks to the native
 * execution engine only through the functions here; it never spawns compilers
 * or processes itself. Types mirror `src-tauri/src/runtime/session.rs`.
 *
 * Observation (recording a run's runtime state) is opt-in per run; the recording
 * stays in the backend and the UI asks for it one step at a time as text.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isNative } from "../utils/native";

export type SessionState =
  | "idle"
  | "compiling"
  | "compilationFailed"
  | "ready"
  | "running"
  | "completed"
  | "failed"
  | "timedOut"
  | "terminated";

export type TerminationReason = "exited" | "timeout" | "userRequested" | "launchFailed";

export interface CompilerDiagnostic {
  severity: "error" | "warning" | "note";
  file: string | null;
  line: number | null;
  column: number | null;
  message: string;
  context: string;
}

export interface CompilerInfo {
  kind: "clang" | "gcc";
  path: string;
  version: string;
}

/** Summary of an observed run (mirrors `ObservationSummary` in session.rs). */
export interface ObservationSummary {
  /** Events recorded. */
  events: number;
  /** The step to open at: the last state before the program's frames are popped. */
  finalStep: number;
  /** Recording stopped at its event budget; the program ran on unobserved. */
  truncated: boolean;
  eventLimit: number;
  /** Set when the run was not instrumented (and why). */
  skipped: string | null;
  issues: string[];
}

/** Whether runs can be observed on this machine (mirrors `ObservationStatus`). */
export interface ObservationStatus {
  available: boolean;
  libclangPath: string | null;
  libclangVersion: string | null;
  /** Why not, and what to do about it. */
  hint: string | null;
  eventLimit: number;
}

/** A recorded run at one step, as text (mirrors `StepView`). */
export interface StepView {
  /** State after this many events; 0 is before the program did anything. */
  step: number;
  total: number;
  event: string | null;
  text: string;
  /** This state is where the recording stopped. */
  truncatedHere: boolean;
}

/**
 * One step of a recorded run as data for the visualization (mirrors `viz::GraphView`).
 * Deliberately generic: frames with variables, objects as trees of typed slots,
 * pointers as targets. There is no "list", "tree" or "graph" here.
 *
 * Anchors: every slot has an address a renderer can attach arrows to: the object id
 * followed by the path inside it (`12`, `12.1`, `12.1[3]`: `.i` is the i-th field,
 * `[i]` the i-th element). A pointer's `target` uses the same (object, path).
 */
export interface GraphView {
  step: number;
  total: number;
  /** The recording ends at this state; the program ran on unobserved. */
  truncatedHere: boolean;
  event: string | null;
  /** Source line the event is attributed to. */
  line: number | null;
  /** Anchors that changed at this step. */
  changed: string[];
  threads: ThreadView[];
  globals: VariableView[];
  objects: ObjectView[];
  /** Objects that qualified but did not fit. */
  objectsOmitted: number;
  /**
   * The objects asked for by id (`observationGraph`'s `focus`), whether or not they are
   * drawn: a freed object nothing points at any more, or a variable's own storage.
   * The same type as `objects`: an object has one description.
   */
  focus: ObjectView[];
}

/**
 * One user-facing step of a run (mirrors `observe::TimelineStep`): a run of raw runtime
 * events presented as one change. `start`/`end` are raw step numbers, the same numbers
 * `observationGraph` takes, so the state to show for a step is the one at `end`.
 */
export interface TimelineStep {
  kind: "call" | "return" | "scopeEnd" | "statement" | "truncated";
  start: number;
  end: number;
  /** Raw events folded into this step. */
  events: number;
  label: string;
  line: number | null;
  /** Call depth (0 = main). */
  depth: number;
}

export interface ThreadView {
  id: number;
  /** Outermost frame first. */
  frames: FrameView[];
}

export interface FrameView {
  id: number;
  thread: number;
  /** Position on its thread's stack; 0 is the outermost frame (`main`). */
  depth: number;
  function: string;
  /** Source file this frame last reported being in. */
  file: string | null;
  line: number | null;
  variables: VariableView[];
}

export type VariableKind = "parameter" | "local" | "global" | "staticLocal";

export interface VariableView {
  name: string;
  kind: VariableKind;
  inBlock: boolean;
  /** The variable's storage object: its anchor. */
  object: number;
  slot: SlotView;
}

/** When an object began and ended, in timeline steps (the numbers the slider shows). */
export interface LifetimeView {
  allocatedStep: number;
  /** The step at which the end first shows; null while it lives. */
  endedStep: number | null;
  endReason: "freed" | "scopeExit" | "frameExit" | "programExit" | "other" | null;
  /** Source line of the declaration/allocation. */
  originLine: number | null;
}

export interface ObjectView {
  id: number;
  state: "alive" | "allocated" | "destroyed" | "unknown";
  storage: string;
  address: string | null;
  lifetime: LifetimeView;
  slot: SlotView;
}

export interface SlotView {
  /** Field name, `[i]` for an element, or null for a root. */
  name: string | null;
  ty: string;
  value: ValueView;
}

export type ValueView =
  | { kind: "scalar"; text: string }
  | { kind: "pointer"; target: TargetView; reference: boolean }
  | { kind: "aggregate"; fields: SlotView[] }
  | { kind: "array"; elements: SlotView[]; omitted: number }
  | { kind: "unavailable"; reason: string };

export interface TargetView {
  kind: "null" | "object" | "unresolved";
  object: number | null;
  /** Path inside `object`: "" for the whole object, else `.1`, `.1[3]`. */
  path: string;
  /** The target's lifetime has ended. */
  dangling: boolean;
}

export interface RuntimeSession {
  id: string;
  project: string;
  sourceFiles: string[];
  state: SessionState;
  compiler: CompilerInfo | null;
  diagnostics: CompilerDiagnostic[];
  compilerOutput: string;
  executablePath: string | null;
  workspacePath: string | null;
  workspaceRemoved: boolean;
  startedAtMs: number;
  endedAtMs: number | null;
  durationMs: number | null;
  compileDurationMs: number | null;
  runDurationMs: number | null;
  stdout: string;
  stderr: string;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  exitCode: number | null;
  terminationReason: TerminationReason | null;
  error: string | null;
  runTimeoutMs: number;
  /** Present when the run was observed. */
  observation: ObservationSummary | null;
}

export interface SourceFile {
  name: string;
  contents: string;
}

export const isActive = (s: SessionState | undefined): boolean =>
  s === "compiling" || s === "ready" || s === "running";

const SESSION_EVENT = "lattice://session-state";

/** Compile and run; resolves with the final session once it has ended. */
export function runProgram(
  project: string,
  files: SourceFile[],
  observe = false,
  /** The open solution's folder: the built executable is also kept in its `target` folder. */
  solution: string | null = null,
): Promise<RuntimeSession> {
  return invoke<RuntimeSession>("run_program", { request: { project, files, observe, solution } });
}

export function observationStatus(): Promise<ObservationStatus> {
  return invoke<ObservationStatus>("observation_status");
}

/** The recorded run of `sessionId` after `step` events. Fails if it was replaced by a newer one. */
export function observationStep(sessionId: string, step: number): Promise<StepView> {
  return invoke<StepView>("observation_step", { sessionId, step });
}

/**
 * The recorded run of `sessionId` after `step` events as data for the visualization.
 * `focus` names objects being inspected; they come back in `GraphView.focus`.
 */
export function observationGraph(sessionId: string, step: number, focus: number[] = []): Promise<GraphView> {
  return invoke<GraphView>("observation_graph", { sessionId, step, focus });
}

/** The user-facing steps of `sessionId`'s recording (raw events grouped; the raw data is unchanged). */
export function observationTimeline(sessionId: string): Promise<TimelineStep[]> {
  return invoke<TimelineStep[]>("observation_timeline", { sessionId });
}

export function stopProgram(sessionId: string): Promise<boolean> {
  return invoke<boolean>("stop_program", { sessionId });
}

/** Subscribe to state changes of any session. No-op outside the desktop shell. */
export async function onSessionState(cb: (s: RuntimeSession) => void): Promise<UnlistenFn> {
  if (!isNative()) return () => {};
  return listen<RuntimeSession>(SESSION_EVENT, (e) => cb(e.payload));
}
