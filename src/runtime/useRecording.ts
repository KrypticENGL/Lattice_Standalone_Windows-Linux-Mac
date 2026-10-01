import { useCallback, useEffect, useRef, useState } from "react";
import {
  observationGraph,
  observationTimeline,
  type GraphView,
  type ObservationSummary,
  type RuntimeSession,
  type TimelineStep,
} from "./index";
import { nextRaw, prevRaw, rawStepOf, userStepAt } from "./timelineSteps";

/** Milliseconds between steps while playing. */
const PLAY_INTERVAL_MS = 450;

export interface Recording {
  /** There is a recording to browse (the run was observed and recorded events). */
  available: boolean;
  summary: ObservationSummary | null;
  sessionId: string | null;
  /** Raw position: events applied. What the backend and the visualization are keyed by. */
  step: number;
  total: number;
  setStep: (n: number) => void;
  /**
   * The user-facing timeline: raw events grouped into steps a reader recognises. Null
   * until loaded or while `detailed` is on, in which case every raw event is a step.
   */
  steps: TimelineStep[] | null;
  /** Show every raw runtime event as its own step instead of the aggregated timeline. */
  detailed: boolean;
  setDetailed: (on: boolean) => void;
  /** Position and length in the timeline the user sees (equal to `step`/`total` when `detailed`). */
  userStep: number;
  userTotal: number;
  setUserStep: (n: number) => void;
  next: () => void;
  prev: () => void;
  /** The step just completed, if the aggregated timeline is showing and the position is past the start. */
  current: TimelineStep | null;
  /** The program at `step`, as data. Null until the first step has loaded. */
  graph: GraphView | null;
  /** The `focus` ids `graph` was fetched with (joined by commas): lets a consumer tell a stale snapshot from a current one. */
  graphFocus: string;
  error: string | null;
  playing: boolean;
  play: () => void;
  pause: () => void;
}

/**
 * The position in the current session's recording, and the program's state there.
 * One instance is shared by everything that shows the recording (the editor's
 * line highlight, the visualization, the Runtime tab), so they always agree on
 * which step is being looked at. `focus` names objects an inspector is looking at; they
 * are described in `graph.focus` at whatever step is showing.
 */
export function useRecording(session: RuntimeSession | null, focus: readonly number[] = []): Recording {
  const summary = session?.observation ?? null;
  const total = summary && !summary.skipped ? summary.events : 0;
  const openAt = summary && !summary.skipped ? Math.min(summary.finalStep, total) : 0;
  const sessionId = session?.id ?? null;

  const [step, setStepRaw] = useState(0);
  const [graph, setGraph] = useState<GraphView | null>(null);
  const [graphFocus, setGraphFocus] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const [rawSteps, setRawSteps] = useState<TimelineStep[] | null>(null);
  const [detailed, setDetailed] = useState(false);
  const ticket = useRef(0);

  // The aggregated timeline is derived from the recording once per run; the raw events are not touched.
  useEffect(() => {
    setRawSteps(null);
    if (!sessionId || total === 0) return;
    let live = true;
    observationTimeline(sessionId)
      .then((s) => live && setRawSteps(s))
      .catch(() => live && setRawSteps(null)); // falls back to raw events
    return () => {
      live = false;
    };
  }, [sessionId, total]);
  const steps = detailed ? null : rawSteps;

  const setStep = useCallback((n: number) => setStepRaw(Math.max(0, Math.min(total, Math.round(n)))), [total]);

  // A finished observed run opens where the program is about to return, not after
  // everything has been torn down (an empty picture).
  useEffect(() => {
    setPlaying(false);
    setStepRaw(openAt);
    setGraph(null);
    setError(null);
  }, [sessionId, total, openAt]);

  // Once the aggregated steps arrive, a position inside a step snaps back to the step before it.
  useEffect(() => {
    if (steps) setStepRaw((s) => rawStepOf(steps, userStepAt(steps, s)));
  }, [steps]);

  const focusKey = focus.join(",");
  useEffect(() => {
    if (!sessionId || total === 0) return;
    const mine = ++ticket.current;
    observationGraph(sessionId, step, focusKey === "" ? [] : focusKey.split(",").map(Number))
      .then((g) => {
        if (mine === ticket.current) {
          setGraph(g);
          setGraphFocus(focusKey);
          setError(null);
        }
      })
      .catch((e) => {
        if (mine === ticket.current) setError(String(e));
      });
  }, [sessionId, step, total, focusKey]);

  // Playback: advance until the end, then stop.
  useEffect(() => {
    if (!playing) return;
    const id = window.setInterval(() => {
      setStepRaw((s) => {
        if (s >= total) {
          setPlaying(false);
          return s;
        }
        return steps ? nextRaw(steps, s) : s + 1;
      });
    }, PLAY_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [playing, total, steps]);

  const play = useCallback(() => {
    // Playing from the end starts over.
    setStepRaw((s) => (s >= total ? 0 : s));
    setPlaying(true);
  }, [total]);
  const pause = useCallback(() => setPlaying(false), []);

  const userStep = steps ? userStepAt(steps, step) : step;
  const userTotal = steps ? steps.length : total;
  const setUserStep = useCallback(
    (n: number) => setStep(steps ? rawStepOf(steps, Math.max(0, Math.min(steps.length, Math.round(n)))) : n),
    [steps, setStep],
  );
  const next = useCallback(() => setStepRaw((s) => Math.min(total, steps ? nextRaw(steps, s) : s + 1)), [steps, total]);
  const prev = useCallback(() => setStepRaw((s) => Math.max(0, steps ? prevRaw(steps, s) : s - 1)), [steps]);
  const current = steps && userStep > 0 ? steps[userStep - 1] : null;

  return { available: total > 0, summary, sessionId, step, total, setStep, steps, detailed, setDetailed, userStep, userTotal, setUserStep, next, prev, current, graph, graphFocus, error, playing, play, pause };
}
