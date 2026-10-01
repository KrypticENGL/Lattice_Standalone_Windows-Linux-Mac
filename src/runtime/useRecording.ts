import { useCallback, useEffect, useRef, useState } from "react";
import { observationGraph, type GraphView, type ObservationSummary, type RuntimeSession } from "./index";

/** Milliseconds between steps while playing. */
const PLAY_INTERVAL_MS = 450;

export interface Recording {
  /** There is a recording to browse (the run was observed and recorded events). */
  available: boolean;
  summary: ObservationSummary | null;
  sessionId: string | null;
  step: number;
  total: number;
  setStep: (n: number) => void;
  /** The program at `step`, as data. Null until the first step has loaded. */
  graph: GraphView | null;
  error: string | null;
  playing: boolean;
  play: () => void;
  pause: () => void;
}

/**
 * The position in the current session's recording, and the program's state there.
 * One instance is shared by everything that shows the recording (the editor's
 * line highlight, the visualization, the Runtime tab), so they always agree on
 * which step is being looked at.
 */
export function useRecording(session: RuntimeSession | null): Recording {
  const summary = session?.observation ?? null;
  const total = summary && !summary.skipped ? summary.events : 0;
  const openAt = summary && !summary.skipped ? Math.min(summary.finalStep, total) : 0;
  const sessionId = session?.id ?? null;

  const [step, setStepRaw] = useState(0);
  const [graph, setGraph] = useState<GraphView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const ticket = useRef(0);

  const setStep = useCallback((n: number) => setStepRaw(Math.max(0, Math.min(total, Math.round(n)))), [total]);

  // A finished observed run opens where the program is about to return, not after
  // everything has been torn down (an empty picture).
  useEffect(() => {
    setPlaying(false);
    setStepRaw(openAt);
    setGraph(null);
    setError(null);
  }, [sessionId, total, openAt]);

  useEffect(() => {
    if (!sessionId || total === 0) return;
    const mine = ++ticket.current;
    observationGraph(sessionId, step)
      .then((g) => {
        if (mine === ticket.current) {
          setGraph(g);
          setError(null);
        }
      })
      .catch((e) => {
        if (mine === ticket.current) setError(String(e));
      });
  }, [sessionId, step, total]);

  // Playback: advance until the end, then stop.
  useEffect(() => {
    if (!playing) return;
    const id = window.setInterval(() => {
      setStepRaw((s) => {
        if (s >= total) {
          setPlaying(false);
          return s;
        }
        return s + 1;
      });
    }, PLAY_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [playing, total]);

  const play = useCallback(() => {
    // Playing from the end starts over.
    setStepRaw((s) => (s >= total ? 0 : s));
    setPlaying(true);
  }, [total]);
  const pause = useCallback(() => setPlaying(false), []);

  return { available: total > 0, summary, sessionId, step, total, setStep, graph, error, playing, play, pause };
}
