import { useCallback, useMemo, useReducer } from "react";
import { focusObjects, inspectorReducer, type SelectedRuntimeEntity, type Trail } from "./selection";

export interface Inspector {
  /** Where the user is in the drill-down; empty when the inspector is closed. */
  trail: Trail;
  /** Object ids the recording should describe for it (`GraphView.focus`). */
  focus: number[];
  /** A pick on the canvas: starts a new trail. */
  open: (entity: SelectedRuntimeEntity) => void;
  /** A drill-down inside the panel. */
  drill: (entity: SelectedRuntimeEntity) => void;
  /** A breadcrumb. */
  goto: (index: number) => void;
  close: () => void;
}

const CLOSED: Trail = [];

/**
 * The inspector's selection. It is deliberately only the selection: what the panel shows
 * is computed from the recording's current snapshot (`buildInspector`), so it follows the
 * timeline without any state of its own, and closing it leaves the timeline untouched.
 */
export function useInspector(): Inspector {
  const [trail, dispatch] = useReducer(inspectorReducer, CLOSED);
  const focus = useMemo(() => focusObjects(trail), [trail]);
  const open = useCallback((entity: SelectedRuntimeEntity) => dispatch({ type: "open", entity }), []);
  const drill = useCallback((entity: SelectedRuntimeEntity) => dispatch({ type: "drill", entity }), []);
  const goto = useCallback((index: number) => dispatch({ type: "goto", index }), []);
  const close = useCallback(() => dispatch({ type: "close" }), []);
  return { trail, focus, open, drill, goto, close };
}
