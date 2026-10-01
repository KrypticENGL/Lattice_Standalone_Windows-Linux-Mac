/**
 * What the user has selected in the runtime visualization, as data.
 *
 * One selection concept for everything inspectable: a stack frame, a variable, a
 * runtime object (or a field/element inside it). It names the thing by *identity* (frame
 * id, object id), never by a snapshot of its contents, so it stays meaningful as the
 * timeline moves: the inspector re-resolves it against whatever step is showing.
 *
 * The inspector keeps a `trail` of selections (Frame → Variable → Object → Object ...):
 * a single panel whose contents change as the user drills down, not a stack of windows.
 */

import type { BoxLayout } from "./layout";

export type SelectedRuntimeEntity =
  /** A call-stack frame. `function` is remembered so a frame that has since returned can still be named. */
  | { kind: "frame"; frameId: number; function: string }
  /** A variable: of a frame, or a global (`frameId` null). `object` is the storage it was bound to when selected. */
  | { kind: "variable"; frameId: number | null; name: string; object: number }
  /** A runtime object; `path` (`.1`, `[3]`, `.1[3]`) narrows it to a field or element. */
  | { kind: "object"; object: number; path: string };

/** Where the user is in the drill-down; the last entry is what the panel shows. Empty = closed. */
export type Trail = readonly SelectedRuntimeEntity[];

/** Deepest drill-down kept; beyond it the oldest entries fall off. */
export const MAX_TRAIL = 12;

export function sameEntity(a: SelectedRuntimeEntity, b: SelectedRuntimeEntity): boolean {
  if (a.kind !== b.kind) return false;
  switch (a.kind) {
    case "frame":
      return a.frameId === (b as typeof a).frameId;
    case "variable": {
      const v = b as typeof a;
      return a.frameId === v.frameId && a.name === v.name;
    }
    case "object": {
      const o = b as typeof a;
      return a.object === o.object && a.path === o.path;
    }
  }
}

/** What clicking a box on the canvas selects: a frame, or an object. Globals are not an entity. */
export function entityForBox(b: Pick<BoxLayout, "kind" | "frameId" | "function" | "objectId">): SelectedRuntimeEntity | null {
  if (b.kind === "frame" && b.frameId !== null) return { kind: "frame", frameId: b.frameId, function: b.function ?? "?" };
  if (b.kind === "object" && b.objectId !== null) return { kind: "object", object: b.objectId, path: "" };
  return null;
}

/** Is this box the thing currently shown in the inspector? */
export function boxIsSelected(b: Pick<BoxLayout, "kind" | "frameId" | "objectId">, current: SelectedRuntimeEntity | undefined): boolean {
  if (!current) return false;
  if (b.kind === "frame") return current.kind === "frame" && current.frameId === b.frameId;
  if (b.kind === "object") return current.kind === "object" && current.object === b.objectId;
  return false;
}

export type InspectorAction =
  /** A pick on the canvas: starts a new trail. */
  | { type: "open"; entity: SelectedRuntimeEntity }
  /** A drill-down inside the panel: extends the trail (or, if already on it, returns to it). */
  | { type: "drill"; entity: SelectedRuntimeEntity }
  /** A breadcrumb: back to the `index`th entry. */
  | { type: "goto"; index: number }
  | { type: "close" };

export function inspectorReducer(trail: Trail, action: InspectorAction): Trail {
  switch (action.type) {
    case "open":
      return [action.entity];
    case "drill": {
      if (trail.length === 0) return [action.entity];
      // Following a pointer around a cycle must not grow the trail forever: arriving at
      // something already on it is going back to it.
      const seen = trail.findIndex((e) => sameEntity(e, action.entity));
      if (seen >= 0) return trail.slice(0, seen + 1);
      const next = [...trail, action.entity];
      return next.length > MAX_TRAIL ? next.slice(next.length - MAX_TRAIL) : next;
    }
    case "goto":
      return action.index >= 0 && action.index < trail.length ? trail.slice(0, action.index + 1) : trail;
    case "close":
      return trail.length === 0 ? trail : [];
  }
}

/**
 * The object ids the backend should describe for this trail (`GraphView.focus`): the
 * objects the user is looking at, so that one that has been freed, or that lives only as
 * a variable's storage, is still shown with its lifetime.
 */
export function focusObjects(trail: Trail): number[] {
  const ids: number[] = [];
  for (const e of trail) {
    if ((e.kind === "object" || e.kind === "variable") && !ids.includes(e.object)) ids.push(e.object);
  }
  return ids;
}
