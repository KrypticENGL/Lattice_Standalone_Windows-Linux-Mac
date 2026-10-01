/**
 * The inspector's view model: a pure function from the runtime snapshot the canvas is
 * already drawing (`GraphView`) and the user's selection trail to "what to show about
 * it". No React, no I/O, no runtime state of its own.
 *
 *   URR → GraphView (the snapshot at the current step) → buildInspector → InspectorPanel
 *
 * Because it is recomputed from the snapshot, the inspector is synchronized with the
 * timeline for free: move the step and the same selection is resolved against the new
 * state. A selection that no longer exists says so (`kind: "missing"`), and an object that
 * has been freed is shown as freed, with when and why, from the lifetime the model keeps.
 *
 * It is as generic as the canvas: frames hold variables, objects hold slots, pointer slots
 * link to places. It knows nothing about lists, trees, arrays or STL types.
 */
import type { FrameView, GraphView, LifetimeView, ObjectView, SlotView, TargetView, VariableKind, VariableView } from "../runtime";
import type { SelectedRuntimeEntity, Trail } from "./selection";

// ---- the model ------------------------------------------------------------------

/** A pointer/reference that leads to a tracked object (or says why it does not). */
export interface Link {
  reference: boolean;
  /** `object`: leads to a tracked object; `null`, `unresolved`: it does not. */
  kind: TargetView["kind"];
  object: number | null;
  /** Path inside the object (`.1`, `[3]`); "" for the whole object. */
  path: string;
  /** `#7`, `#7.next`, `#3[2]`, or `null` / `untracked`. */
  label: string;
  /** The object it leads to has been freed. */
  dangling: boolean;
  /** What selecting it drills into; null when it leads nowhere. */
  entity: SelectedRuntimeEntity | null;
}

/** One line of a variable's or object's contents (a field, an element, or the value itself). */
export interface InspectorRow {
  depth: number;
  label: string;
  ty: string;
  /** Scalar text, `null`/`untracked`, or `‹reason›`; null for a group header and for a link. */
  text: string | null;
  link: Link | null;
  muted: boolean;
  /** Path of this slot inside its object (`.1[3]`), for highlighting. */
  path: string;
}

/** `list → #3`, `#3.next → #7`: a pointer, as a statement about two places. */
export interface Relationship {
  from: string;
  to: string;
  toObject: number;
  dangling: boolean;
  reference: boolean;
  entity: SelectedRuntimeEntity;
}

export interface VariableSummary {
  name: string;
  ty: string;
  kind: VariableKind;
  inBlock: boolean;
  /** `10`, `→ #3`, `null`, `{…}`, `[5]`. */
  summary: string;
  link: Link | null;
  entity: SelectedRuntimeEntity;
}

export interface ObjectSummary {
  id: number;
  ty: string;
  state: ObjectView["state"];
  storage: string;
  /** The variable whose storage this is, if any. */
  variable: string | null;
  summary: string;
  entity: SelectedRuntimeEntity;
}

export interface FrameContext {
  kind: "frame";
  frameId: number;
  function: string;
  /** `main.cpp:81`, or null when the observer did not say. */
  source: string | null;
  line: number | null;
  /** Position on the stack: 0 = outermost. */
  depth: number;
  /** Executing now (top of its thread's stack). */
  isTop: boolean;
  caller: { entity: SelectedRuntimeEntity; function: string } | null;
  parameters: VariableSummary[];
  locals: VariableSummary[];
  /** Runtime objects reachable from this frame's variables, nearest first. */
  objects: ObjectSummary[];
  relationships: Relationship[];
}

export interface VariableContext {
  kind: "variable";
  name: string;
  ty: string;
  variableKind: VariableKind;
  /** The frame it belongs to; null for a global. */
  frame: { entity: SelectedRuntimeEntity; function: string } | null;
  /** The object holding its value (the variable's own storage). */
  object: number;
  summary: string;
  rows: InspectorRow[];
  /** The objects its pointers lead to, one level deep. */
  targets: ObjectContext[];
}

export interface ObjectContext {
  kind: "object";
  id: number;
  ty: string;
  state: ObjectView["state"];
  storage: string;
  address: string | null;
  /** False once the object's lifetime has ended. */
  alive: boolean;
  /** `alive`, `allocated (not yet constructed)`, `freed at event #106`, ... */
  status: string;
  lifetime: LifetimeView | null;
  /** The variable this is the storage of, if any. */
  variable: string | null;
  rows: InspectorRow[];
  /** A field or element the selection points at inside the object (`.1[3]`), or "". */
  highlight: string;
  /** Pointers anywhere in the program that lead into this object. */
  referencedBy: Relationship[];
}

/** The selection no longer exists at this step. */
export interface MissingContext {
  kind: "missing";
  title: string;
  reason: string;
}

export type InspectorBody = FrameContext | VariableContext | ObjectContext | MissingContext;

export interface Crumb {
  label: string;
  entity: SelectedRuntimeEntity;
  missing: boolean;
}

export interface InspectorModel {
  /** The timeline position this was built for. */
  step: number;
  total: number;
  crumbs: Crumb[];
  body: InspectorBody;
}

// ---- the snapshot, indexed -------------------------------------------------------

interface Known {
  id: number;
  ty: string;
  state: ObjectView["state"];
  storage: string;
  address: string | null;
  lifetime: LifetimeView | null;
  slot: SlotView;
  variable: { name: string; function: string | null } | null;
}

interface Index {
  g: GraphView;
  objects: Map<number, Known>;
  frames: Map<number, FrameView>;
  /** Every named variable: frames' (with their frame) then globals'. */
  variables: { frame: FrameView | null; v: VariableView }[];
}

function indexSnapshot(g: GraphView): Index {
  const frames = new Map<number, FrameView>();
  const variables: Index["variables"] = [];
  const boundTo = new Map<number, { name: string; function: string | null }>();
  for (const t of g.threads) {
    for (const f of t.frames) {
      frames.set(f.id, f);
      for (const v of f.variables) {
        variables.push({ frame: f, v });
        boundTo.set(v.object, { name: v.name, function: f.function });
      }
    }
  }
  for (const v of g.globals) {
    variables.push({ frame: null, v });
    boundTo.set(v.object, { name: v.name, function: null });
  }

  const objects = new Map<number, Known>();
  // A variable's storage first (no lifetime info), then the fuller descriptions over it.
  for (const { v } of variables) {
    if (!objects.has(v.object)) {
      objects.set(v.object, {
        id: v.object,
        ty: v.slot.ty,
        state: "alive",
        storage: v.kind === "global" || v.kind === "staticLocal" ? "static" : "automatic",
        address: null,
        lifetime: null,
        slot: v.slot,
        variable: boundTo.get(v.object) ?? null,
      });
    }
  }
  for (const o of [...g.objects, ...g.focus]) {
    objects.set(o.id, {
      id: o.id,
      ty: o.slot.ty,
      state: o.state,
      storage: o.storage,
      address: o.address,
      lifetime: o.lifetime,
      slot: o.slot,
      variable: boundTo.get(o.id) ?? null,
    });
  }
  return { g, objects, frames, variables };
}

// ---- paths and labels ------------------------------------------------------------

type PathStep = { field: number } | { index: number };

function parsePath(path: string): PathStep[] {
  const steps: PathStep[] = [];
  for (const m of path.matchAll(/\.(\d+)|\[(\d+)\]/g)) {
    steps.push(m[1] !== undefined ? { field: Number(m[1]) } : { index: Number(m[2]) });
  }
  return steps;
}

/** `.1` → `.next`, `[3]` → `[3]`, `.1[3]` → `.next[3]`, using the object's own slot names. */
function pathLabel(slot: SlotView | undefined, path: string): string {
  if (path === "") return "";
  let out = "";
  let cur: SlotView | undefined = slot;
  for (const step of parsePath(path)) {
    if ("field" in step) {
      const next: SlotView | undefined = cur?.value.kind === "aggregate" ? cur.value.fields[step.field] : undefined;
      out += `.${next?.name ?? step.field}`;
      cur = next;
    } else {
      const next: SlotView | undefined = cur?.value.kind === "array" ? cur.value.elements[step.index] : undefined;
      out += `[${step.index}]`;
      cur = next;
    }
  }
  return out;
}

const objectLabel = (id: number) => `#${id}`;

function link(ix: Index, t: TargetView, reference: boolean): Link {
  if (t.kind === "null") return { reference, kind: "null", object: null, path: "", label: "null", dangling: false, entity: null };
  if (t.kind === "unresolved" || t.object === null) {
    return { reference, kind: "unresolved", object: null, path: "", label: "untracked", dangling: false, entity: null };
  }
  const known = ix.objects.get(t.object);
  const label = `${objectLabel(t.object)}${pathLabel(known?.slot, t.path)}`;
  return {
    reference,
    kind: "object",
    object: t.object,
    path: t.path,
    label,
    dangling: t.dangling,
    entity: { kind: "object", object: t.object, path: t.path },
  };
}

// ---- rows ------------------------------------------------------------------------

function pushRows(ix: Index, slot: SlotView, label: string, depth: number, path: string, rows: InspectorRow[]): void {
  const base = { depth, label, ty: slot.ty, path, link: null, muted: false };
  const v = slot.value;
  switch (v.kind) {
    case "scalar":
      rows.push({ ...base, text: v.text });
      break;
    case "unavailable":
      rows.push({ ...base, text: `‹${v.reason}›`, muted: true });
      break;
    case "pointer": {
      const l = link(ix, v.target, v.reference);
      rows.push(
        l.kind === "object"
          ? { ...base, text: null, link: l }
          : { ...base, text: l.label, muted: true },
      );
      break;
    }
    case "aggregate":
      rows.push({ ...base, text: null });
      v.fields.forEach((f, i) => pushRows(ix, f, f.name ?? `.${i}`, depth + 1, `${path}.${i}`, rows));
      break;
    case "array":
      rows.push({ ...base, text: null });
      v.elements.forEach((e, i) => pushRows(ix, e, e.name ?? `[${i}]`, depth + 1, `${path}[${i}]`, rows));
      if (v.omitted > 0) rows.push({ ...base, depth: depth + 1, label: `… ${v.omitted} more`, ty: "", text: null, muted: true, path: "" });
      break;
  }
}

/** The members of an object (its fields or elements); a scalar object is one unnamed row. */
function objectRows(ix: Index, slot: SlotView): InspectorRow[] {
  const rows: InspectorRow[] = [];
  const v = slot.value;
  if (v.kind === "aggregate") {
    v.fields.forEach((f, i) => pushRows(ix, f, f.name ?? `.${i}`, 0, `.${i}`, rows));
  } else if (v.kind === "array") {
    v.elements.forEach((e, i) => pushRows(ix, e, e.name ?? `[${i}]`, 0, `[${i}]`, rows));
    if (v.omitted > 0) rows.push({ depth: 0, label: `… ${v.omitted} more`, ty: "", text: null, link: null, muted: true, path: "" });
  } else {
    pushRows(ix, slot, "value", 0, "", rows);
  }
  return rows;
}

function summarize(ix: Index, slot: SlotView): { text: string; link: Link | null } {
  const v = slot.value;
  switch (v.kind) {
    case "scalar":
      return { text: v.text, link: null };
    case "unavailable":
      return { text: `‹${v.reason}›`, link: null };
    case "pointer": {
      const l = link(ix, v.target, v.reference);
      return { text: l.kind === "object" ? `→ ${l.label}${l.dangling ? " (freed)" : ""}` : l.label, link: l.kind === "object" ? l : null };
    }
    case "aggregate":
      return { text: "{…}", link: null };
    case "array":
      return { text: `[${v.elements.length + v.omitted}]`, link: null };
  }
}

// ---- links between places --------------------------------------------------------

interface RawLink {
  from: string;
  target: TargetView;
  reference: boolean;
}

/** Every pointer/reference inside `slot`, with where it sits (`list`, `#3.next`, ...). */
function linksIn(slot: SlotView, from: string, out: RawLink[]): void {
  const v = slot.value;
  switch (v.kind) {
    case "pointer":
      out.push({ from, target: v.target, reference: v.reference });
      break;
    case "aggregate":
      v.fields.forEach((f, i) => linksIn(f, `${from}.${f.name ?? i}`, out));
      break;
    case "array":
      v.elements.forEach((e, i) => linksIn(e, `${from}[${i}]`, out));
      break;
  }
}

function relationship(ix: Index, raw: RawLink): Relationship | null {
  const l = link(ix, raw.target, raw.reference);
  if (l.kind !== "object" || l.object === null || l.entity === null) return null;
  return { from: raw.from, to: l.label, toObject: l.object, dangling: l.dangling, reference: raw.reference, entity: l.entity };
}

/** The label a place goes by in a relationship: a variable's name, or `#id` for an object. */
function holderLabel(k: Known): string {
  return k.variable ? k.variable.name : objectLabel(k.id);
}

// ---- contexts --------------------------------------------------------------------

const MAX_REACHABLE = 64;
const MAX_REFERENCES = 50;
const MAX_TARGETS = 8;

function describeEnd(l: LifetimeView | null): string {
  if (l === null || l.endedStep === null) return "ended";
  const at = `at event #${l.endedStep}`;
  switch (l.endReason) {
    case "freed":
      return `freed ${at}`;
    case "scopeExit":
      return `went out of scope ${at}`;
    case "frameExit":
      return `destroyed when its function returned ${at}`;
    case "programExit":
      return `destroyed at program exit ${at}`;
    default:
      return `ended ${at}`;
  }
}

function statusOf(k: Known): string {
  switch (k.state) {
    case "alive":
      return "alive";
    case "allocated":
      return "allocated (not yet constructed)";
    case "destroyed":
      return describeEnd(k.lifetime);
    case "unknown":
      return "lifetime unknown";
  }
}

function summaryOf(ix: Index, k: Known): string {
  return summarize(ix, k.slot).text;
}

function objectContext(ix: Index, id: number, highlight: string, withReferences: boolean): ObjectContext | null {
  const k = ix.objects.get(id);
  if (!k) return null;
  const referencedBy: Relationship[] = [];
  if (withReferences) {
    const scan = (slot: SlotView, from: string) => {
      const raw: RawLink[] = [];
      linksIn(slot, from, raw);
      for (const r of raw) {
        if (r.target.object === id && referencedBy.length < MAX_REFERENCES) {
          const rel = relationship(ix, r);
          if (rel) referencedBy.push(rel);
        }
      }
    };
    for (const { v } of ix.variables) scan(v.slot, v.name);
    for (const other of ix.objects.values()) {
      // A variable's storage was already scanned under the variable's name.
      if (other.variable === null) scan(other.slot, objectLabel(other.id));
    }
  }
  return {
    kind: "object",
    id,
    ty: k.ty,
    state: k.state,
    storage: k.storage,
    address: k.address,
    alive: k.state !== "destroyed",
    status: statusOf(k),
    lifetime: k.lifetime,
    variable: k.variable?.name ?? null,
    rows: objectRows(ix, k.slot),
    highlight,
    referencedBy,
  };
}

function variableSummary(ix: Index, frame: FrameView | null, v: VariableView): VariableSummary {
  const s = summarize(ix, v.slot);
  return {
    name: v.name,
    ty: v.slot.ty,
    kind: v.kind,
    inBlock: v.inBlock,
    summary: s.text,
    link: s.link,
    entity: { kind: "variable", frameId: frame?.id ?? null, name: v.name, object: v.object },
  };
}

function frameContext(ix: Index, f: FrameView): FrameContext {
  const thread = ix.g.threads.find((t) => t.id === f.thread);
  const callerFrame = thread?.frames.find((x) => x.depth === f.depth - 1) ?? null;
  const summaries = f.variables.map((v) => variableSummary(ix, f, v));

  // What the frame can reach: breadth-first from its variables through pointers.
  const relationships: Relationship[] = [];
  const reached: number[] = [];
  const seen = new Set<number>();
  const queue: RawLink[] = [];
  for (const v of f.variables) linksIn(v.slot, v.name, queue);
  for (let i = 0; i < queue.length; i++) {
    const raw = queue[i];
    const rel = relationship(ix, raw);
    if (!rel) continue;
    relationships.push(rel);
    if (!seen.has(rel.toObject) && reached.length < MAX_REACHABLE) {
      seen.add(rel.toObject);
      reached.push(rel.toObject);
      const k = ix.objects.get(rel.toObject);
      if (k) linksIn(k.slot, holderLabel(k), queue);
    }
  }
  const objects: ObjectSummary[] = reached.flatMap((id) => {
    const k = ix.objects.get(id);
    if (!k) return [];
    return [
      {
        id,
        ty: k.ty,
        state: k.state,
        storage: k.storage,
        variable: k.variable?.name ?? null,
        summary: summaryOf(ix, k),
        entity: { kind: "object", object: id, path: "" } as SelectedRuntimeEntity,
      },
    ];
  });

  return {
    kind: "frame",
    frameId: f.id,
    function: f.function,
    source: f.file !== null && f.line !== null ? `${f.file}:${f.line}` : f.line !== null ? `line ${f.line}` : null,
    line: f.line,
    depth: f.depth,
    isTop: thread ? thread.frames[thread.frames.length - 1]?.id === f.id : false,
    caller: callerFrame
      ? {
          entity: { kind: "frame", frameId: callerFrame.id, function: callerFrame.function },
          function: callerFrame.function,
        }
      : null,
    parameters: summaries.filter((s) => s.kind === "parameter"),
    locals: summaries.filter((s) => s.kind !== "parameter"),
    objects,
    relationships,
  };
}

function findVariable(ix: Index, e: Extract<SelectedRuntimeEntity, { kind: "variable" }>) {
  const mine = ix.variables.filter((x) => (x.frame?.id ?? null) === e.frameId && x.v.name === e.name);
  // Shadowing: prefer the one still bound to the storage that was selected.
  return mine.find((x) => x.v.object === e.object) ?? mine[mine.length - 1] ?? null;
}

function variableContext(ix: Index, e: Extract<SelectedRuntimeEntity, { kind: "variable" }>): VariableContext | MissingContext {
  const found = findVariable(ix, e);
  if (!found) {
    const owner = e.frameId === null ? null : ix.frames.get(e.frameId);
    return {
      kind: "missing",
      title: e.name,
      reason:
        e.frameId !== null && !owner
          ? "The function this variable belongs to is not on the call stack at this step."
          : "This variable is not in scope at this step.",
    };
  }
  const { frame, v } = found;
  const raw: RawLink[] = [];
  linksIn(v.slot, v.name, raw);
  const targets: ObjectContext[] = [];
  for (const r of raw) {
    const id = r.target.object;
    if (id === null || targets.some((t) => t.id === id) || targets.length >= MAX_TARGETS) continue;
    const ctx = objectContext(ix, id, "", false);
    if (ctx) targets.push(ctx);
  }
  const rows: InspectorRow[] = [];
  pushRows(ix, v.slot, v.name, 0, "", rows);
  return {
    kind: "variable",
    name: v.name,
    ty: v.slot.ty,
    variableKind: v.kind,
    frame: frame ? { entity: { kind: "frame", frameId: frame.id, function: frame.function }, function: frame.function } : null,
    object: v.object,
    summary: summarize(ix, v.slot).text,
    rows,
    targets,
  };
}

function missingObject(ix: Index, e: Extract<SelectedRuntimeEntity, { kind: "object" }>): MissingContext {
  return {
    kind: "missing",
    title: objectLabel(e.object),
    reason: `Object ${objectLabel(e.object)} does not exist at event #${ix.g.step}: it has not been created yet.`,
  };
}

function bodyFor(ix: Index, e: SelectedRuntimeEntity): InspectorBody {
  switch (e.kind) {
    case "frame": {
      const f = ix.frames.get(e.frameId);
      return f
        ? frameContext(ix, f)
        : { kind: "missing", title: `${e.function}()`, reason: "This call is not on the stack at this step: it has not been entered yet, or has returned." };
    }
    case "variable":
      return variableContext(ix, e);
    case "object":
      return objectContext(ix, e.object, e.path, true) ?? missingObject(ix, e);
  }
}

function crumbFor(ix: Index, e: SelectedRuntimeEntity, body: InspectorBody): Crumb {
  const missing = body.kind === "missing";
  switch (e.kind) {
    case "frame":
      return { label: `${e.function}()`, entity: e, missing };
    case "variable":
      return { label: e.name, entity: e, missing };
    case "object": {
      const ty = ix.objects.get(e.object)?.ty;
      return { label: `${ty ? `${ty} ` : ""}${objectLabel(e.object)}${e.path !== "" ? pathLabel(ix.objects.get(e.object)?.slot, e.path) : ""}`, entity: e, missing };
    }
  }
}

/**
 * What the inspector shows for `trail` at the snapshot `g`; null when nothing is
 * selected. The breadcrumbs name every step of the trail (so the user can go back to any
 * of them); the body is the last one.
 */
export function buildInspector(g: GraphView, trail: Trail): InspectorModel | null {
  if (trail.length === 0) return null;
  const ix = indexSnapshot(g);
  const bodies = trail.map((e) => bodyFor(ix, e));
  const crumbs = trail.map((e, i) => crumbFor(ix, e, bodies[i]));
  return { step: g.step, total: g.total, crumbs, body: bodies[bodies.length - 1] };
}
