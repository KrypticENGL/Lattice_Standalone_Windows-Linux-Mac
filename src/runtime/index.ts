/**
 * Frontend boundary for program execution. The UI talks to the native
 * execution engine only through the functions here; it never spawns compilers
 * or processes itself. Types mirror `src-tauri/src/runtime/session.rs`.
 *
 * Runtime observation (events, data model) is not implemented yet.
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
}

export interface SourceFile {
  name: string;
  contents: string;
}

export const isActive = (s: SessionState | undefined): boolean =>
  s === "compiling" || s === "ready" || s === "running";

const SESSION_EVENT = "lattice://session-state";

/** Compile and run; resolves with the final session once it has ended. */
export function runProgram(project: string, files: SourceFile[]): Promise<RuntimeSession> {
  return invoke<RuntimeSession>("run_program", { request: { project, files } });
}

export function stopProgram(sessionId: string): Promise<boolean> {
  return invoke<boolean>("stop_program", { sessionId });
}

/** Subscribe to state changes of any session. No-op outside the desktop shell. */
export async function onSessionState(cb: (s: RuntimeSession) => void): Promise<UnlistenFn> {
  if (!isNative()) return () => {};
  return listen<RuntimeSession>(SESSION_EVENT, (e) => cb(e.payload));
}
