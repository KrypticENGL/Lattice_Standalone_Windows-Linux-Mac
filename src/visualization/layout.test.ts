import { describe, expect, it } from "vitest";
import { layoutGraph, metricsFor, sampleEdge, type BoxLayout } from "./layout";
import type { SlotView, VariableView } from "../runtime";
import { agg, frame, graph, node, obj, ptr, scalar, variable } from "./fixtures";

const m = metricsFor(7);

// ---- fixtures (shared builders live in ./fixtures) -----------------------------------

const head = (target: number) => variable("head", 1, ptr(null, target));
const mainWith = (...vars: VariableView[]) => [{ id: 0, frames: [frame(1, "main", vars)] }];
const box = (l: ReturnType<typeof layoutGraph>, key: string): BoxLayout => l.boxes.find((b) => b.key === key)!;
const rowCenter = (b: BoxLayout, i: number) => b.y + m.headerH + b.rows[i].y + m.rowH / 2;

// ---- tests ------------------------------------------------------------------

describe("layout", () => {
  it("runs a chain of objects left to right, level with the pointers that reach them", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [node(10, 1, ptr("next", 11)), node(11, 2, ptr("next", 12)), node(12, 3, ptr("next", null))],
    });
    const l = layoutGraph(g, m);
    const [a, b, c] = [box(l, "object:10"), box(l, "object:11"), box(l, "object:12")];
    expect(a.x).toBeGreaterThan(box(l, "frame:1").x + box(l, "frame:1").w);
    expect(b.x).toBeGreaterThan(a.x + a.w);
    expect(c.x).toBeGreaterThan(b.x + b.w);
    // Each box's header is level with the pointer row that reaches it, so the arrows run straight.
    const headerMid = (x: BoxLayout) => x.y + m.headerH / 2;
    expect(headerMid(a)).toBeCloseTo(rowCenter(box(l, "frame:1"), 0), 5);
    expect(headerMid(b)).toBeCloseTo(rowCenter(a, 1), 5); // row 1 of `a` is its `next`
    expect(headerMid(c)).toBeCloseTo(rowCenter(b, 1), 5);
    expect(l.edges).toHaveLength(3); // head->a, a->b, b->c
    expect(l.unplaced).toBe(0);
  });

  it("terminates on cycles and sharing, and places every object exactly once", () => {
    const g = graph({
      threads: mainWith(head(10), variable("other", 2, ptr(null, 10))),
      objects: [node(10, 1, ptr("next", 11)), node(11, 2, ptr("next", 10)), node(12, 3, ptr("next", 12))],
    });
    const l = layoutGraph(g, m);
    expect(l.boxes.filter((b) => b.kind === "object")).toHaveLength(3);
    // Node 12 points at itself and nothing reaches it: still placed, still drawn.
    expect(l.edges.some((e) => e.id.endsWith("->12"))).toBe(true);
    // Both stack variables point at the same object: one box, two arrows into it.
    expect(l.edges.filter((e) => e.id.endsWith("->10")).length).toBe(3); // head, other, and 11's back-pointer
  });

  it("stacks the children of one object in a single column", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [
        obj(10, agg("Tree", [scalar("v", "int", "1"), ptr("left", 11), ptr("right", 12)])),
        node(11, 2, ptr("next", null)),
        node(12, 3, ptr("next", null)),
      ],
    });
    const l = layoutGraph(g, m);
    const [c1, c2] = [box(l, "object:11"), box(l, "object:12")];
    expect(c1.x).toBe(c2.x);
    expect(c2.y).toBeGreaterThanOrEqual(c1.y + c1.h); // no overlap
    expect(c1.x).toBeGreaterThan(box(l, "object:10").x);
  });

  it("puts objects nothing reaches in a column after the reachable ones", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [node(10, 1, ptr("next", null)), node(99, 9, ptr("next", null))],
    });
    const l = layoutGraph(g, m);
    expect(box(l, "object:99").x).toBeGreaterThan(box(l, "object:10").x);
  });

  it("draws a heap pointer into a stack variable as a backward edge onto the frame", () => {
    const g = graph({
      threads: mainWith(head(10), variable("second", 2, scalar(null, "int", "5"))),
      objects: [node(10, 1, ptr("next", 2))],
    });
    const l = layoutGraph(g, m);
    const f = box(l, "frame:1");
    const e = l.edges.find((x) => x.id.endsWith("->2"))!;
    expect(e.to.x).toBe(f.x + f.w); // ends on the right side of the frame box
    expect(e.to.y).toBeCloseTo(rowCenter(f, 1), 5); // at the `second` row
    expect(e.from.x).toBeGreaterThan(e.to.x);
  });

  it("aims an arrow at the part of an object a pointer points at, not at its header", () => {
    const g = graph({
      threads: mainWith(variable("p", 1, ptr(null, 20, ".1"))),
      objects: [node(20, 1, ptr("next", null))],
    });
    const l = layoutGraph(g, m);
    const target = box(l, "object:20");
    expect(l.edges[0].to.y).toBeCloseTo(rowCenter(target, 1), 5);
    expect(l.edges[0].to.y).not.toBeCloseTo(target.y + m.headerH / 2, 1);
  });

  it("routes a pointer that goes back around its own box instead of across its text", () => {
    // head -> 10 -> 11 -> 12 -> 10, and p -> 12 puts 12 in the first column: 11's pointer to 12 goes back.
    const g = graph({
      threads: mainWith(head(10), variable("p", 2, ptr(null, 12))),
      objects: [node(10, 1, ptr("next", 11)), node(11, 2, ptr("next", 12)), node(12, 3, ptr("next", 10))],
    });
    const l = layoutGraph(g, m);
    const src = box(l, "object:11");
    const back = l.edges.find((e) => e.id.startsWith("11.1->12"))!;
    expect(back.to.x).toBeGreaterThan(back.from.x - 1000); // sanity: it exists and is a real path
    expect(box(l, "object:12").x).toBeLessThan(src.x); // really a back edge
    const inside = sampleEdge(back.path).slice(1).filter( // (the first point is the pointer's own dot)
      (pt) => pt.x > src.x + 1 && pt.x < src.x + src.w - 1 && pt.y > src.y && pt.y < src.y + src.h,
    );
    expect(inside).toEqual([]); // no point of the curve lies on the source box's body
  });

  it("draws a pointer to its own object as a small arc beside it", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [node(10, 1, ptr("next", 10))],
    });
    const l = layoutGraph(g, m);
    const b = box(l, "object:10");
    const self = l.edges.find((e) => e.id.startsWith("10.1->10"))!;
    const pts = sampleEdge(self.path);
    const maxX = Math.max(...pts.map((p) => p.x));
    expect(maxX).toBeGreaterThan(b.x + b.w); // bulges out to the right of the box
    expect(Math.max(...pts.map((p) => p.x))).toBeLessThan(b.x + b.w + 80);
  });

  it("sizes the drawing area to include every arrow, so loops are not clipped", () => {
    // Pointers inside one frame (q -> arr[2], pp -> pr) arc out to the right of it; a
    // back-edge from the first box can rise above the top margin.
    const arr: SlotView = {
      name: null,
      ty: "int[4]",
      value: { kind: "array", omitted: 0, elements: [scalar("[0]", "int", "5"), scalar("[1]", "int", "6"), scalar("[2]", "int", "7")] },
    };
    const same = graph({
      threads: mainWith(
        variable("arr", 1, arr),
        variable("pr", 2, agg("Pair", [scalar("a", "int", "1")])),
        variable("q", 3, ptr(null, 1, "[2]")),
        variable("pp", 4, ptr(null, 2)),
      ),
    });
    const cycle = graph({
      threads: mainWith(head(10), variable("p", 2, ptr(null, 12))),
      objects: [node(10, 1, ptr("next", 11)), node(11, 2, ptr("next", 12)), node(12, 3, ptr("next", 10))],
    });
    for (const g of [same, cycle]) {
      const l = layoutGraph(g, m);
      expect(l.edges.length).toBeGreaterThan(0);
      const v = l.viewBox;
      for (const e of l.edges) {
        for (const p of sampleEdge(e.path)) {
          expect(p.x).toBeGreaterThanOrEqual(v.x);
          expect(p.x).toBeLessThanOrEqual(v.x + v.w);
          expect(p.y).toBeGreaterThanOrEqual(v.y);
          expect(p.y).toBeLessThanOrEqual(v.y + v.h);
        }
      }
      for (const b of l.boxes) {
        expect(b.x).toBeGreaterThanOrEqual(v.x);
        expect(b.x + b.w).toBeLessThanOrEqual(v.x + v.w);
      }
      // The area is at least as big as the boxes need.
      expect(v.w).toBeGreaterThanOrEqual(l.width - 1e-9 - Math.abs(v.x));
    }
  });

  it("flags dangling arrows and freed objects", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [node(10, 1, ptr("next", null), "destroyed")],
    });
    // The pointer in `head` is dangling: the model says so in its target.
    g.threads[0].frames[0].variables[0].slot = ptr(null, 10, "", true);
    const l = layoutGraph(g, m);
    expect(l.edges[0].dangling).toBe(true);
    expect(box(l, "object:10").state).toBe("destroyed");
    expect(box(l, "object:10").subtitle).toContain("freed");
  });

  it("counts pointers it cannot draw instead of failing", () => {
    const g = graph({ threads: mainWith(head(777)), objects: [] });
    const l = layoutGraph(g, m);
    expect(l.edges).toHaveLength(0);
    expect(l.unplaced).toBe(1);
  });

  it("shows null and untracked pointers as text, without an arrow", () => {
    const g = graph({
      threads: mainWith(variable("a", 1, ptr(null, null)), variable("b", 2, ptr(null, null, "", false, "unresolved"))),
    });
    const l = layoutGraph(g, m);
    const rows = box(l, "frame:1").rows;
    expect(rows.map((r) => r.text)).toEqual(["null", "untracked"]);
    expect(l.edges).toHaveLength(0);
  });

  it("gives array elements their own addressable rows and says how many were left out", () => {
    const arr: SlotView = {
      name: null,
      ty: "int[100]",
      value: { kind: "array", omitted: 36, elements: [scalar("[0]", "int", "7"), scalar("[1]", "int", "8")] },
    };
    const g = graph({
      threads: mainWith(variable("q", 1, ptr(null, 30, "[1]"))),
      objects: [obj(30, arr)],
    });
    const l = layoutGraph(g, m);
    const b = box(l, "object:30");
    expect(b.rows.map((r) => r.label)).toEqual(["[0]", "[1]", "… 36 more"]);
    expect(b.rows.map((r) => r.anchor)).toEqual(["30[0]", "30[1]", null]);
    expect(l.edges[0].to.y).toBeCloseTo(rowCenter(b, 1), 5);
  });

  it("nests aggregates under a header row and addresses their members by path", () => {
    const inner = agg("Inner", [scalar("a", "int", "1"), scalar("b", "int", "2")], "in");
    const g = graph({
      threads: mainWith(variable("p", 1, ptr(null, 40, ".0.1"))),
      objects: [obj(40, agg("Outer", [inner, ptr("p", null)]))],
    });
    const l = layoutGraph(g, m);
    const b = box(l, "object:40");
    expect(b.rows.map((r) => [r.label, r.depth, r.anchor])).toEqual([
      ["in", 0, "40.0"],
      ["a", 1, "40.0.0"],
      ["b", 1, "40.0.1"],
      ["p", 0, "40.1"],
    ]);
    expect(l.edges[0].to.y).toBeCloseTo(rowCenter(b, 2), 5); // `.0.1` is the row for `b`
  });

  it("marks what changed at this step", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [node(10, 1, ptr("next", null))],
      changed: ["10.0", "1"],
    });
    const l = layoutGraph(g, m);
    expect(box(l, "object:10").rows[0].changed).toBe(true); // 10.0 = value
    expect(box(l, "object:10").rows[1].changed).toBe(false);
    expect(box(l, "frame:1").rows[0].changed).toBe(true); // anchor 1 = the `head` variable
    expect(l.edges[0].changed).toBe(true);
  });

  it("marks the innermost frame as the current one", () => {
    const g = graph({
      threads: [{ id: 0, frames: [frame(1, "main", []), frame(2, "foo", []), frame(3, "bar", [])] }],
    });
    const l = layoutGraph(g, m);
    expect(l.boxes.map((b) => [b.title, b.current])).toEqual([["main()", false], ["foo()", false], ["bar()", true]]);
    // outermost first, top to bottom
    expect(l.boxes[1].y).toBeGreaterThan(l.boxes[0].y);
  });

  it("is deterministic and sizes the canvas to its contents", () => {
    const g = graph({
      threads: mainWith(head(10)),
      objects: [node(10, 1, ptr("next", 11)), node(11, 2, ptr("next", null))],
    });
    const a = layoutGraph(g, m);
    expect(layoutGraph(g, m)).toEqual(a);
    for (const b of a.boxes) {
      expect(b.x + b.w).toBeLessThanOrEqual(a.width);
      expect(b.y + b.h).toBeLessThanOrEqual(a.height);
    }
  });

  it("makes boxes wide enough for their longest row", () => {
    const short = layoutGraph(graph({ threads: mainWith(variable("a", 1, scalar(null, "int", "1"))) }), m);
    const long = layoutGraph(
      graph({ threads: mainWith(variable("a_rather_long_variable_name", 1, scalar(null, "int", "12345678901234567890"))) }),
      m,
    );
    expect(long.boxes[0].w).toBeGreaterThan(short.boxes[0].w);
    expect(short.boxes[0].w).toBe(m.minBoxW);
  });

  it("lays out an empty program without failing", () => {
    const l = layoutGraph(graph({}), m);
    expect(l.boxes).toHaveLength(0);
    expect(l.width).toBeGreaterThan(0);
  });
});
