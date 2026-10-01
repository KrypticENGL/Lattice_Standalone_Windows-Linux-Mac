/**
 * Builders for `GraphView` snapshots, shared by the layout and inspector tests. They
 * produce the same shapes the backend's `viz::build` does, so tests read like programs:
 * frames with variables, objects with slots, pointers with targets.
 */
import type {
  FrameView,
  GraphView,
  LifetimeView,
  ObjectView,
  SlotView,
  ThreadView,
  VariableKind,
  VariableView,
} from "../runtime";

export const scalar = (name: string | null, ty: string, text: string): SlotView => ({ name, ty, value: { kind: "scalar", text } });

export const ptr = (
  name: string | null,
  object: number | null,
  path = "",
  dangling = false,
  kind: "object" | "null" | "unresolved" = "object",
  ty = "Node*",
): SlotView => ({
  name,
  ty,
  value: { kind: "pointer", reference: false, target: { kind: object === null ? (kind === "object" ? "null" : kind) : kind, object, path, dangling } },
});

export const agg = (ty: string, fields: SlotView[], name: string | null = null): SlotView => ({ name, ty, value: { kind: "aggregate", fields } });

export const arr = (ty: string, elements: SlotView[], omitted = 0): SlotView => ({ name: null, ty, value: { kind: "array", elements, omitted } });

export const life = (allocatedStep: number, endedStep: number | null = null, endReason: LifetimeView["endReason"] = null): LifetimeView => ({
  allocatedStep,
  endedStep,
  endReason: endedStep === null ? null : (endReason ?? "freed"),
  originLine: null,
});

/** A heap object with any slot. */
export const obj = (id: number, slot: SlotView, state: ObjectView["state"] = "alive", lifetime: LifetimeView = life(1)): ObjectView => ({
  id,
  state,
  storage: "heap",
  address: null,
  lifetime: state === "destroyed" && lifetime.endedStep === null ? life(lifetime.allocatedStep, lifetime.allocatedStep + 1) : lifetime,
  slot,
});

/** A heap `Node { int value; Node* next; }`. */
export const node = (id: number, value: number, next: SlotView, state: ObjectView["state"] = "alive", lifetime?: LifetimeView): ObjectView =>
  obj(id, agg("Node", [scalar("value", "int", String(value)), next]), state, lifetime);

export const variable = (name: string, object: number, slot: SlotView, kind: VariableKind = "local"): VariableView => ({
  name,
  kind,
  inBlock: false,
  object,
  slot,
});

export const frame = (
  id: number,
  fn: string,
  variables: VariableView[],
  p: Partial<Omit<FrameView, "id" | "function" | "variables">> = {},
): FrameView => ({ id, thread: 0, depth: 0, function: fn, file: "main.cpp", line: 3, variables, ...p });

/** One thread (id 0) with these frames, outermost first; `depth` is filled in. */
export const stackOf = (...frames: FrameView[]): ThreadView[] => [
  { id: 0, frames: frames.map((f, i) => ({ ...f, depth: i })) },
];

export const graph = (p: Partial<GraphView>): GraphView => ({
  step: 1,
  total: 1,
  truncatedHere: false,
  event: null,
  line: null,
  changed: [],
  threads: [],
  globals: [],
  objects: [],
  objectsOmitted: 0,
  focus: [],
  ...p,
});
