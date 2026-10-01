import { useEffect, useRef, useState } from "react";
import { observationStep, type RuntimeSession, type StepView } from "../runtime";
import type { Recording } from "../runtime/useRecording";

/**
 * The recorded run of the current session as text, at the same step the
 * visualization is showing (the position is shared through `recording`). A
 * diagnostic view of what was observed, not a visualization: the backend renders
 * each step; this only asks for it.
 */
export function ObservationView({ session, recording }: { session: RuntimeSession; recording: Recording }) {
  const summary = session.observation;
  const { step, total, setStep } = recording;
  const [view, setView] = useState<StepView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const latest = useRef(0);

  useEffect(() => {
    if (!summary || summary.skipped || total === 0) return;
    const ticket = ++latest.current;
    observationStep(session.id, step)
      .then((v) => {
        if (ticket === latest.current) {
          setView(v);
          setError(null);
        }
      })
      .catch((e) => {
        if (ticket === latest.current) setError(String(e));
      });
  }, [session.id, step, summary, total]);

  if (!summary) return null;
  if (summary.skipped) return <p className="out-note">Not observed: {summary.skipped}</p>;
  if (total === 0) return <p className="out-note">The program ran, but no events were recorded.</p>;

  return (
    <div className="obs">
      {summary.truncated && (
        <p className="diag-warning">
          Recording stopped after {summary.eventLimit.toLocaleString()} events. The program kept running, so
          the later part of the run, including its end, is not shown.
        </p>
      )}
      {summary.issues.map((m, i) => (
        <p key={i} className="diag-error">{m}</p>
      ))}
      <div className="obs-controls">
        <button className="btn" onClick={() => setStep(0)} disabled={step === 0} title="Before the program started">
          Start
        </button>
        <button className="btn" onClick={() => setStep(step - 1)} disabled={step === 0} aria-label="Previous event">
          ◀
        </button>
        <input
          type="range"
          min={0}
          max={total}
          value={step}
          onChange={(e) => setStep(Number(e.target.value))}
          aria-label="Position in the run"
        />
        <button className="btn" onClick={() => setStep(step + 1)} disabled={step >= total} aria-label="Next event">
          ▶
        </button>
        <button className="btn" onClick={() => setStep(total)} disabled={step >= total} title="The last recorded state">
          End
        </button>
        <span className="muted">
          event {step.toLocaleString()} of {total.toLocaleString()}
        </span>
      </div>
      {error && <p className="diag-error">{error}</p>}
      {view && (
        <>
          <p className="obs-event">{view.event ?? "(before the program ran)"}</p>
          {view.truncatedHere && (
            <p className="diag-warning">The recording ends here; the program ran on unobserved.</p>
          )}
          <pre className="obs-text">{view.text}</pre>
        </>
      )}
    </div>
  );
}
