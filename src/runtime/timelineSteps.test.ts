import { describe, expect, it } from "vitest";
import type { TimelineStep } from "./index";
import { nextRaw, prevRaw, rawStepOf, userStepAt } from "./timelineSteps";

const step = (start: number, end: number): TimelineStep => ({
  kind: "statement", start, end, events: end - start, label: "", line: null, depth: 0,
});
const steps = [step(0, 3), step(3, 7), step(7, 8)];

describe("timeline steps", () => {
  it("maps raw positions to user steps and back", () => {
    expect([0, 2, 3, 6, 7, 8].map((r) => userStepAt(steps, r))).toEqual([0, 0, 1, 1, 2, 3]);
    expect([0, 1, 2, 3].map((n) => rawStepOf(steps, n))).toEqual([0, 3, 7, 8]);
  });
  it("moves one user step at a time and clamps", () => {
    expect(nextRaw(steps, 0)).toBe(3);
    expect(nextRaw(steps, 3)).toBe(7);
    expect(nextRaw(steps, 8)).toBe(8);
    expect(prevRaw(steps, 7)).toBe(3);
    expect(prevRaw(steps, 5)).toBe(3);
    expect(prevRaw(steps, 3)).toBe(0);
    expect(prevRaw(steps, 0)).toBe(0);
  });
});
