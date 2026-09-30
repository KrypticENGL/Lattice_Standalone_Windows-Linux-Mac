import type { RuntimeSession } from "./index";

/** One-line status for the status bar. */
export function statusText(s: RuntimeSession | null, running: boolean): string {
  if (!s) return running ? "Starting…" : "Ready";
  switch (s.state) {
    case "idle":
      return "Ready";
    case "compiling":
      return "Compiling…";
    case "ready":
      return "Starting…";
    case "running":
      return "Running…";
    case "compilationFailed":
      return "Compilation failed";
    case "completed":
      return `Finished (exit 0) in ${s.runDurationMs ?? 0} ms`;
    case "failed":
      return s.exitCode !== null ? `Exited with code ${s.exitCode}` : "Failed";
    case "timedOut":
      return "Timed out";
    case "terminated":
      return "Stopped";
  }
}
