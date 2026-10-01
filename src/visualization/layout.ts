/**
 * Layout of one program state. A pure function: `GraphView` in, boxes and arrows out.
 *
 * It is generic on purpose. It does not know about linked lists, trees or graphs;
 * it knows *frames hold variables*, *objects hold slots*, and *pointer slots point
 * at things*. Structure shows up on its own:
 *
 *   - The call stack is a column on the left, outermost frame first.
 *   - Heap objects go in columns by their distance from the stack: what a stack
 *     variable points at is column 0, what that points at is column 1, and so on
 *     (breadth-first, so cycles and sharing cannot loop). Objects nothing reaches
 *     go in a last column.
 *   - Within a column an object is placed level with the pointer that first reached
 *     it (so a chain of nodes runs straight), and pushed down to avoid overlap.
 *
 * Sizes come from fixed row heights and the width of a monospace character, so the
 * layout is deterministic and needs no measuring of the DOM.
 */
import type { GraphView, ObjectView, SlotView, TargetView, VariableView } from "../runtime";

export interface Metrics {
  charW: number;
  rowH: number;
  headerH: number;
  padX: number;
  indent: number;
  layerGap: number;
  boxGap: number;
  margin: number;
  minBoxW: number;
}

export const metricsFor = (charW: number): Metrics => ({
  charW,
  rowH: 22,
  headerH: 26,
  padX: 10,
  indent: 14,
  layerGap: 96,
  boxGap: 16,
  margin: 16,
  minBoxW: 120,
});

export interface RowLayout {
  /** Where arrows can attach to this row (null for rows that are only text). */
  anchor: string | null;
  depth: number;
  /** Top of the row, relative to the top of the box body. */
  y: number;
  label: string;
  ty: string;
  /** What to print after the label; null for a header row or a drawn pointer. */
  text: string | null;
  /** A pointer to draw an arrow for. */
  pointer: TargetView | null;
  reference: boolean;
  muted: boolean;
  changed: boolean;
}

export interface BoxLayout {
  key: string;
  kind: "frame" | "globals" | "object";
  title: string;
  subtitle: string | null;
  x: number;
  y: number;
  w: number;
  h: number;
  rows: RowLayout[];
  /** Objects: alive / allocated / destroyed / unknown. */
  state: string | null;
  /** The whole box is new, gone, or otherwise the subject of this step. */
  changed: boolean;
  /** The frame that is executing. */
  current: boolean;
  objectId: number | null;
  /** Frames: which frame this box is, and its function (what a click selects). */
  frameId: number | null;
  function: string | null;
}

export interface Point {
  x: number;
  y: number;
}

export interface EdgeLayout {
  id: string;
  path: string;
  from: Point;
  to: Point;
  dangling: boolean;
  reference: boolean;
  changed: boolean;
}

export interface Layout {
  boxes: BoxLayout[];
  edges: EdgeLayout[];
  /** Extent of the boxes (and the margin around them). */
  width: number;
  height: number;
  /**
   * What must be visible: the boxes *and* every arrow. Arrows that loop back bulge
   * outside the boxes (to the right, or above the first one), so the drawing area is
   * sized from them too or they would be clipped.
   */
  viewBox: { x: number; y: number; w: number; h: number };
  /** Pointers whose target is not drawn (omitted objects, untracked parts). */
  unplaced: number;
}

interface Draft {
  box: BoxLayout;
  pointers: { row: RowLayout; target: TargetView }[];
}

// ---- rows ------------------------------------------------------------------

function pointerText(t: TargetView): string | null {
  if (t.kind === "null") return "null";
  if (t.kind === "unresolved") return "untracked";
  return null;
}

function flatten(
  slot: SlotView,
  anchor: string,
  depth: number,
  label: string,
  rows: RowLayout[],
  changed: Set<string>,
): void {
  const base = { anchor, depth, y: 0, label, ty: slot.ty, pointer: null, reference: false, muted: false, changed: changed.has(anchor) };
  const v = slot.value;
  switch (v.kind) {
    case "scalar":
      rows.push({ ...base, text: v.text });
      break;
    case "unavailable":
      rows.push({ ...base, text: `‹${v.reason}›`, muted: true });
      break;
    case "pointer":
      rows.push({
        ...base,
        text: pointerText(v.target),
        pointer: v.target.kind === "object" ? v.target : null,
        reference: v.reference,
        muted: v.target.kind !== "object",
      });
      break;
    case "aggregate":
      rows.push({ ...base, text: null });
      v.fields.forEach((f, i) => flatten(f, `${anchor}.${i}`, depth + 1, f.name ?? `.${i}`, rows, changed));
      break;
    case "array":
      rows.push({ ...base, text: null });
      v.elements.forEach((e, i) => flatten(e, `${anchor}[${i}]`, depth + 1, e.name ?? `[${i}]`, rows, changed));
      if (v.omitted > 0) {
        rows.push({ ...base, anchor: null, depth: depth + 1, label: `… ${v.omitted} more`, text: null, muted: true, changed: false });
      }
      break;
  }
}

/** The rows of an object's box: its members directly (the header already names its type). */
function objectRows(o: ObjectView, changed: Set<string>): RowLayout[] {
  const id = String(o.id);
  const rows: RowLayout[] = [];
  const v = o.slot.value;
  if (v.kind === "aggregate") {
    v.fields.forEach((f, i) => flatten(f, `${id}.${i}`, 0, f.name ?? `.${i}`, rows, changed));
  } else if (v.kind === "array") {
    v.elements.forEach((e, i) => flatten(e, `${id}[${i}]`, 0, e.name ?? `[${i}]`, rows, changed));
    if (v.omitted > 0) {
      rows.push({ anchor: null, depth: 0, y: 0, label: `… ${v.omitted} more`, ty: "", text: null, pointer: null, reference: false, muted: true, changed: false });
    }
  } else {
    flatten(o.slot, id, 0, "", rows, changed);
  }
  return rows;
}

function variableRows(v: VariableView, changed: Set<string>): RowLayout[] {
  const rows: RowLayout[] = [];
  flatten(v.slot, String(v.object), 0, v.name, rows, changed);
  return rows;
}

function rowWidth(r: RowLayout, m: Metrics): number {
  const chars = r.label.length + (r.text !== null ? 2 + r.text.length : 0) + (r.pointer ? 2 : 0);
  return m.padX * 2 + r.depth * m.indent + chars * m.charW + (r.pointer ? 14 : 0);
}

function finish(draft: Draft, m: Metrics, titleChars: number): void {
  const { box } = draft;
  box.rows.forEach((r, i) => (r.y = i * m.rowH));
  box.w = Math.max(m.minBoxW, m.padX * 2 + titleChars * m.charW, ...box.rows.map((r) => rowWidth(r, m)));
  box.h = m.headerH + box.rows.length * m.rowH + 6;
  draft.pointers = box.rows.filter((r) => r.pointer).map((r) => ({ row: r, target: r.pointer! }));
}

function emptyBox(key: string, kind: BoxLayout["kind"], title: string, subtitle: string | null): BoxLayout {
  return {
    key, kind, title, subtitle, x: 0, y: 0, w: 0, h: 0, rows: [], state: null, changed: false, current: false,
    objectId: null, frameId: null, function: null,
  };
}

// ---- layout ------------------------------------------------------------------

export function layoutGraph(g: GraphView, m: Metrics): Layout {
  const changed = new Set(g.changed);

  // Stack: one box per frame (outermost first), plus a box for globals.
  const stack: Draft[] = [];
  if (g.globals.length > 0) {
    const box = emptyBox("globals", "globals", "globals", null);
    box.rows = g.globals.flatMap((v) => variableRows(v, changed));
    const d: Draft = { box, pointers: [] };
    finish(d, m, 7);
    stack.push(d);
  }
  for (const t of g.threads) {
    t.frames.forEach((f, i) => {
      const title = `${f.function}()`;
      const box = emptyBox(`frame:${f.id}`, "frame", title, f.line !== null ? `line ${f.line}` : null);
      box.rows = f.variables.flatMap((v) => variableRows(v, changed));
      box.current = i === t.frames.length - 1;
      box.changed = changed.has(`frame:${f.id}`);
      box.frameId = f.id;
      box.function = f.function;
      const d: Draft = { box, pointers: [] };
      finish(d, m, title.length + (box.subtitle ? box.subtitle.length + 2 : 0));
      stack.push(d);
    });
  }

  // Heap objects.
  const heap = new Map<number, Draft>();
  for (const o of g.objects) {
    const box = emptyBox(`object:${o.id}`, "object", o.slot.ty, `#${o.id}${o.state === "destroyed" ? " freed" : ""}`);
    box.rows = objectRows(o, changed);
    box.state = o.state;
    box.objectId = o.id;
    box.changed = changed.has(String(o.id));
    const d: Draft = { box, pointers: [] };
    finish(d, m, box.title.length + (box.subtitle?.length ?? 0) + 2);
    heap.set(o.id, d);
  }

  // Stack column.
  let y = m.margin;
  const stackW = Math.max(0, ...stack.map((d) => d.box.w));
  for (const d of stack) {
    d.box.x = m.margin;
    d.box.y = y;
    y += d.box.h + m.boxGap;
  }
  const stackBottom = y;
  const heapX = stack.length > 0 ? m.margin + stackW + m.layerGap : m.margin;

  // Layers: breadth-first from what the stack points at.
  const layer = new Map<number, number>();
  const parentRow = new Map<number, { draft: Draft; row: RowLayout }>();
  const order: number[] = [];
  const reach = (from: Draft, ownLayer: number) => {
    for (const p of from.pointers) {
      const t = p.target.object;
      if (t !== null && heap.has(t) && !layer.has(t)) {
        layer.set(t, ownLayer + 1);
        parentRow.set(t, { draft: from, row: p.row });
        order.push(t);
      }
    }
  };
  stack.forEach((d) => reach(d, -1));
  for (let i = 0; i < order.length; i++) reach(heap.get(order[i])!, layer.get(order[i])!);
  const maxLayer = Math.max(-1, ...layer.values());
  for (const id of [...heap.keys()].sort((a, b) => a - b)) {
    if (!layer.has(id)) {
      layer.set(id, maxLayer + 1);
      order.push(id);
    }
  }

  // Columns, left to right; each object level with the pointer that reached it.
  const columns = new Map<number, number[]>();
  for (const id of order) {
    const l = layer.get(id)!;
    columns.set(l, [...(columns.get(l) ?? []), id]);
  }
  let x = heapX;
  let heapBottom = 0;
  for (const l of [...columns.keys()].sort((a, b) => a - b)) {
    const ids = columns.get(l)!;
    const colW = Math.max(...ids.map((id) => heap.get(id)!.box.w));
    let cursor = m.margin;
    for (const id of ids) {
      const d = heap.get(id)!;
      const parent = parentRow.get(id);
      const wanted = parent ? parent.draft.box.y + m.headerH + parent.row.y + m.rowH / 2 - m.headerH / 2 : cursor;
      d.box.x = x;
      d.box.y = Math.max(cursor, wanted);
      cursor = d.box.y + d.box.h + m.boxGap;
    }
    heapBottom = Math.max(heapBottom, cursor);
    x += colW + m.layerGap;
  }

  // Arrows.
  const boxes = [...stack.map((d) => d.box), ...[...heap.values()].map((d) => d.box)];
  const anchorAt = new Map<string, { box: BoxLayout; y: number }>();
  for (const b of boxes) {
    if (b.objectId !== null) anchorAt.set(String(b.objectId), { box: b, y: b.y + m.headerH / 2 });
    for (const r of b.rows) if (r.anchor) anchorAt.set(r.anchor, { box: b, y: b.y + m.headerH + r.y + m.rowH / 2 });
  }
  const edges: EdgeLayout[] = [];
  let unplaced = 0;
  for (const d of [...stack, ...heap.values()]) {
    for (const p of d.pointers) {
      const key = `${p.target.object}${p.target.path}`;
      const dest = anchorAt.get(key) ?? anchorAt.get(String(p.target.object));
      if (!dest || !p.row.anchor) {
        unplaced++;
        continue;
      }
      const from: Point = { x: d.box.x + d.box.w - 8, y: d.box.y + m.headerH + p.row.y + m.rowH / 2 };
      const forward = dest.box.x >= from.x + 20;
      const to: Point = { x: forward ? dest.box.x : dest.box.x + dest.box.w, y: dest.y };
      edges.push({
        id: `${p.row.anchor}->${key}`,
        from,
        to,
        path: edgePath(from, to, forward, d.box === dest.box ? null : { top: d.box.y, bottom: d.box.y + d.box.h }),
        dangling: p.target.dangling,
        reference: p.row.reference,
        changed: p.row.changed,
      });
    }
  }

  const width = Math.max(m.margin * 2 + stackW, ...boxes.map((b) => b.x + b.w + m.margin));
  const height = Math.max(stackBottom, heapBottom) + m.margin;
  let [x0, y0, x1, y1] = [0, 0, width, height];
  for (const e of edges) {
    for (const p of sampleEdge(e.path, 12)) {
      x0 = Math.min(x0, p.x - 6);
      y0 = Math.min(y0, p.y - 6);
      x1 = Math.max(x1, p.x + m.margin);
      y1 = Math.max(y1, p.y + m.margin);
    }
  }
  return { boxes, edges, width, height, viewBox: { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }, unplaced };
}

export interface Bounds {
  top: number;
  bottom: number;
}

/**
 * A smooth curve out of the right edge of a slot into the side of its target.
 *
 * Going forward it is a plain S-curve. Going back (the target is at or behind the
 * pointer), the curve leaves the source box to the right and travels *around* it,
 * above or below depending on where the target is, so it never runs across the
 * box's own text. `src` is the source box's vertical extent (null when the target is
 * in the same box: a small arc on its right side is enough).
 */
export function edgePath(from: Point, to: Point, forward: boolean, src: Bounds | null = null): string {
  if (forward) {
    const dx = Math.max(40, (to.x - from.x) / 2);
    return `M ${from.x} ${from.y} C ${from.x + dx} ${from.y}, ${to.x - dx} ${to.y}, ${to.x} ${to.y}`;
  }
  if (src === null) {
    const bulge = Math.max(from.x, to.x) + 44;
    return `M ${from.x} ${from.y} C ${bulge} ${from.y}, ${bulge} ${to.y}, ${to.x} ${to.y}`;
  }
  // The curve's midpoint sits at (from.y + to.y)/8 + 0.75 * controlY: pick controlY so
  // that midpoint clears the box by a margin.
  const margin = 18;
  const below = to.y >= from.y;
  const clear = below ? Math.max(src.bottom, to.y) + margin : Math.min(src.top, to.y) - margin;
  const controlY = (clear - (from.y + to.y) / 8) / 0.75;
  const reach = Math.max(from.x, to.x) + 70;
  return `M ${from.x} ${from.y} C ${reach} ${controlY}, ${reach} ${controlY}, ${to.x} ${to.y}`;
}

/** Points along an edge's path (for tests and hit-testing). */
export function sampleEdge(path: string, steps = 20): Point[] {
  const n = path.match(/-?\d+(\.\d+)?/g)?.map(Number) ?? [];
  if (n.length < 8) return [];
  const [x0, y0, x1, y1, x2, y2, x3, y3] = n;
  const at = (t: number): Point => {
    const u = 1 - t;
    return {
      x: u * u * u * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x3,
      y: u * u * u * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t * y3,
    };
  };
  return Array.from({ length: steps + 1 }, (_, i) => at(i / steps));
}
