import type { TimelineStep } from "./index";

/**
 * Maps between raw steps (events applied, what the backend and the visualization use)
 * and user steps (the aggregated timeline the reader moves through). User step `n`
 * (1-based) ends at raw step `steps[n - 1].end`; user step 0 is raw step 0.
 * All pure, so the mapping is testable without a recording.
 */

/** Number of steps completed at raw step `raw` (binary search over the step ends). */
export function userStepAt(steps: readonly TimelineStep[], raw: number): number {
  let lo = 0;
  let hi = steps.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (steps[mid].end <= raw) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** The raw step that shows user step `n`. */
export function rawStepOf(steps: readonly TimelineStep[], n: number): number {
  if (n <= 0) return 0;
  return steps[Math.min(n, steps.length) - 1].end;
}

/** The raw step one user step after `raw` (clamped to the last). */
export function nextRaw(steps: readonly TimelineStep[], raw: number): number {
  return rawStepOf(steps, userStepAt(steps, raw) + 1);
}

/** The raw step one user step before `raw` (a position inside a step first snaps back to its start). */
export function prevRaw(steps: readonly TimelineStep[], raw: number): number {
  const n = userStepAt(steps, raw);
  const atBoundary = n === 0 || steps[n - 1].end === raw;
  return rawStepOf(steps, atBoundary ? n - 1 : n);
}
