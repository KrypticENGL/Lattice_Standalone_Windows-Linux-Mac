import { useEffect, useState } from "react";
import type { CompilerDiagnostic, RuntimeSession } from "../runtime";

const tabs = ["Output", "Errors", "Runtime"] as const;
type Tab = (typeof tabs)[number];

interface Props {
  session: RuntimeSession | null;
  /** UI-side failure (e.g. running in a plain browser). */
  runError: string | null;
}

const seconds = (ms: number) => `${(ms / 1000).toString()} s`;

/** Tabbed panel showing the result of the current session. Arrow keys move between tabs. */
export function OutputPanel({ session, runError }: Props) {
  const [active, setActive] = useState<Tab>("Output");
  const state = session?.state;

  // Surface problems where the user will look for them.
  useEffect(() => {
    if (state === "compilationFailed") setActive("Errors");
    else if (state === "compiling" || state === "completed" || state === "timedOut") setActive("Output");
  }, [state]);
  useEffect(() => {
    if (runError || (state === "failed" && session?.error)) setActive("Errors");
  }, [runError, state, session?.error]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = tabs.indexOf(active);
    if (e.key === "ArrowRight") setActive(tabs[(i + 1) % tabs.length]);
    if (e.key === "ArrowLeft") setActive(tabs[(i + tabs.length - 1) % tabs.length]);
  };

  return (
    <section className="panel output-panel">
      <div className="tabs" role="tablist" onKeyDown={onKeyDown}>
        {tabs.map((t) => (
          <button key={t} role="tab" aria-selected={t === active}
            tabIndex={t === active ? 0 : -1}
            className={`tab ${t === active ? "active" : ""}`}
            onClick={() => setActive(t)}>
            {t}
          </button>
        ))}
      </div>
      <div className="panel-body output-body" role="tabpanel">
        {active === "Output" && <OutputTab session={session} />}
        {active === "Errors" && <ErrorsTab session={session} runError={runError} />}
        {active === "Runtime" && <RuntimeTab session={session} />}
      </div>
    </section>
  );
}

function OutputTab({ session: s }: { session: RuntimeSession | null }) {
  if (!s) return <span className="muted">Press Run to compile and execute main.cpp.</span>;
  if (s.state === "compiling") return <span className="muted">Compiling…</span>;
  if (s.state === "compilationFailed") return <span className="muted">Compilation failed — see Errors.</span>;
  if (s.state === "failed" && s.exitCode === null && s.error) return <span className="muted">Could not run — see Errors.</span>;

  const noOutput = !s.stdout && !s.stderr;
  return (
    <>
      {s.state === "running" && <p className="out-note">Running…</p>}
      {s.stdout}
      {s.stdoutTruncated && <p className="out-note">[stdout truncated]</p>}
      {s.stderr && <div className="out-stderr">{s.stderr}</div>}
      {s.stderrTruncated && <p className="out-note">[stderr truncated]</p>}
      {noOutput && s.state === "completed" && <span className="muted">(program produced no output)</span>}
      {s.state === "timedOut" && (
        <p className="diag-error">
          Execution was terminated because it exceeded the configured timeout of {seconds(s.runTimeoutMs)}.
        </p>
      )}
      {s.state === "terminated" && <p className="out-note">Execution was stopped.</p>}
      {s.state === "failed" && s.exitCode !== null && (
        <p className="diag-error">Program exited with code {s.exitCode}.</p>
      )}
    </>
  );
}

function ErrorsTab({ session: s, runError }: { session: RuntimeSession | null; runError: string | null }) {
  if (runError) return <p className="diag-error">{runError}</p>;
  if (!s) return <span className="muted">No errors.</span>;
  if (s.error) return <p className="diag-error">{s.error}</p>;
  if (s.diagnostics.length === 0) return <span className="muted">No errors.</span>;
  return (
    <>
      {s.diagnostics.map((d, i) => (
        <Diagnostic key={i} d={d} />
      ))}
    </>
  );
}

function Diagnostic({ d }: { d: CompilerDiagnostic }) {
  const where = d.file ? `${d.file}${d.line ? `:${d.line}` : ""}${d.column ? `:${d.column}` : ""}: ` : "";
  return (
    <div className="diag">
      <div className={`diag-${d.severity}`}>
        {where}
        {d.severity}: {d.message}
      </div>
      {d.context && <div className="diag-context">{d.context}</div>}
    </div>
  );
}

function RuntimeTab({ session: s }: { session: RuntimeSession | null }) {
  if (!s) return <span className="muted">No session yet.</span>;
  const rows: [string, string][] = [
    ["Session", s.id],
    ["State", s.state],
    ["Compiler", s.compiler ? `${s.compiler.version} (${s.compiler.path})` : "—"],
    ["Compile time", s.compileDurationMs !== null ? `${s.compileDurationMs} ms` : "—"],
    ["Run time", s.runDurationMs !== null ? `${s.runDurationMs} ms` : "—"],
    ["Exit code", s.exitCode !== null ? String(s.exitCode) : "—"],
    ["Ended because", s.terminationReason ?? "—"],
    ["Workspace", s.workspacePath ? `${s.workspacePath}${s.workspaceRemoved ? " (removed)" : ""}` : "—"],
  ];
  return (
    <>
      <table className="runtime-table">
        <tbody>
          {rows.map(([k, v]) => (
            <tr key={k}><td>{k}</td><td>{v}</td></tr>
          ))}
        </tbody>
      </table>
      <p className="out-note">Runtime data-structure observation is not implemented yet.</p>
    </>
  );
}
