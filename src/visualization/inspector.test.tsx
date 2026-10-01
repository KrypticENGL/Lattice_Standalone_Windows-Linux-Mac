import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { GraphView } from "../runtime";
import { arr, frame, graph, life, node, obj, ptr, scalar, stackOf, variable } from "./fixtures";
import { buildInspector, type FrameContext, type InspectorModel, type ObjectContext, type VariableContext } from "./inspector";
import { InspectorPanel } from "./InspectorPanel";
import { layoutGraph, metricsFor } from "./layout";
import {
  boxIsSelected,
  entityForBox,
  focusObjects,
  inspectorReducer,
  MAX_TRAIL,
  type SelectedRuntimeEntity,
  type Trail,
} from "./selection";

// ---- a small program, at several moments ------------------------------------------
//
//   main()  (main.cpp:81)            list : Node*     -> Node #3
//   Node #3 { data = 10, next -> Node #7 }
//   Node #7 { data = 20, next -> null }
//
// `list` is a variable whose own storage is object 100.

const LIST_VAR = 100;
const mainFrame = (listTarget: number | null, extra: Partial<Parameters<typeof frame>[3]> = {}, dangling = false) =>
  frame(1, "main", [variable("list", LIST_VAR, ptr(null, listTarget, "", dangling))], { file: "main.cpp", line: 81, ...extra });

const n3 = (state: "alive" | "destroyed" = "alive") =>
  node(3, 10, ptr("next", 7), state, state === "destroyed" ? life(40, 106, "freed") : life(40));
const n7 = () => node(7, 20, ptr("next", null), "alive", life(60));

/** The program at one step. */
const at = (step: number, p: Partial<GraphView>): GraphView => graph({ step, total: 128, ...p });

const atEvent20 = at(20, { threads: stackOf(mainFrame(1)), objects: [node(1, 5, ptr("next", null), "alive", life(10))] });
const atEvent80 = at(80, { threads: stackOf(mainFrame(3)), objects: [n3(), n7()] });
const atEvent105 = at(105, { threads: stackOf(mainFrame(3)), objects: [n3(), n7()] });
// Event 106 frees Node #3: `list` still holds the pointer, which the backend now marks dangling
// (so #3 is still drawn, as freed).
const atEvent106 = at(106, { threads: stackOf(mainFrame(3, {}, true)), objects: [n3("destroyed"), n7()] });

const frameEntity = (id = 1, fn = "main"): SelectedRuntimeEntity => ({ kind: "frame", frameId: id, function: fn });
const objectEntity = (object: number, path = ""): SelectedRuntimeEntity => ({ kind: "object", object, path });
const listVar: SelectedRuntimeEntity = { kind: "variable", frameId: 1, name: "list", object: LIST_VAR };

const model = (g: GraphView, ...trail: SelectedRuntimeEntity[]): InspectorModel => buildInspector(g, trail)!;
const frameBody = (m: InspectorModel) => {
  expect(m.body.kind).toBe("frame");
  return m.body as FrameContext;
};
const objectBody = (m: InspectorModel) => {
  expect(m.body.kind).toBe("object");
  return m.body as ObjectContext;
};
const html = (m: InspectorModel, loading = false) =>
  renderToStaticMarkup(<InspectorPanel model={m} loading={loading} onDrill={() => {}} onGoto={() => {}} onClose={() => {}} />);

// ---- 1. clicking a stack frame opens its context -------------------------------------

describe("opening the inspector", () => {
  it("a click on a frame box selects that frame and opens its context", () => {
    const l = layoutGraph(atEvent80, metricsFor(7));
    const box = l.boxes.find((b) => b.kind === "frame")!;
    const entity = entityForBox(box);
    expect(entity).toEqual({ kind: "frame", frameId: 1, function: "main" });

    expect(buildInspector(atEvent80, [])).toBeNull(); // closed until something is picked
    const trail = inspectorReducer([], { type: "open", entity: entity! });
    expect(trail).toEqual([entity]);
    expect(buildInspector(atEvent80, trail)?.body.kind).toBe("frame");
  });

  it("objects can be picked on the canvas too; globals cannot", () => {
    const g = at(80, { threads: stackOf(mainFrame(3)), objects: [n3(), n7()], globals: [variable("g", 5, scalar(null, "int", "1"), "global")] });
    const l = layoutGraph(g, metricsFor(7));
    const kinds = l.boxes.map((b) => [b.kind, entityForBox(b)?.kind ?? null]);
    expect(kinds).toContainEqual(["frame", "frame"]);
    expect(kinds).toContainEqual(["object", "object"]);
    expect(kinds).toContainEqual(["globals", null]);
  });

  it("marks the selected box and only that one", () => {
    const l = layoutGraph(atEvent80, metricsFor(7));
    const selected = l.boxes.filter((b) => boxIsSelected(b, frameEntity()));
    expect(selected.map((b) => b.key)).toEqual(["frame:1"]);
    expect(l.boxes.filter((b) => boxIsSelected(b, objectEntity(7))).map((b) => b.key)).toEqual(["object:7"]);
    expect(l.boxes.some((b) => boxIsSelected(b, undefined))).toBe(false);
  });
});

// ---- 2-4. what the frame context shows -----------------------------------------------

describe("stack-frame context", () => {
  it("shows the function name, source location, frame number and who called it", () => {
    const g = at(30, {
      threads: stackOf(
        mainFrame(null, { line: 90 }),
        frame(2, "append", [variable("n", 200, scalar(null, "int", "4"), "parameter")], { line: 12 }),
      ),
    });
    const callee = frameBody(model(g, frameEntity(2, "append")));
    expect(callee.function).toBe("append");
    expect(callee.source).toBe("main.cpp:12");
    expect(callee.depth).toBe(1);
    expect(callee.isTop).toBe(true);
    expect(callee.caller).toMatchObject({ function: "main" });

    const outer = frameBody(model(g, frameEntity()));
    expect(outer.source).toBe("main.cpp:90");
    expect(outer.depth).toBe(0);
    expect(outer.isTop).toBe(false);
    expect(outer.caller).toBeNull();

    const text = html(model(g, frameEntity(2, "append")));
    expect(text).toContain("append()");
    expect(text).toContain("main.cpp:12");
    expect(text).toContain("#1"); // stack frame number
    expect(text).toContain("Called from");
  });

  it("separates parameters from locals and gives each its type and value", () => {
    const g = at(30, {
      threads: stackOf(
        frame(
          1,
          "f",
          [
            variable("a", 10, scalar(null, "int", "3"), "parameter"),
            variable("b", 11, scalar(null, "double", "2.5"), "parameter"),
            variable("sum", 12, scalar(null, "int", "5")),
            variable("p", 13, ptr(null, null, "", false, "null", "int*")),
          ],
        ),
      ),
    });
    const c = frameBody(model(g, frameEntity(1, "f")));
    expect(c.parameters.map((v) => [v.name, v.ty, v.summary])).toEqual([["a", "int", "3"], ["b", "double", "2.5"]]);
    expect(c.locals.map((v) => [v.name, v.ty, v.summary])).toEqual([["sum", "int", "5"], ["p", "int*", "null"]]);
  });

  it("renders the sections the user asked for", () => {
    const text = html(model(atEvent80, frameEntity()));
    for (const s of ["Function", "Source", "Stack frame", "Parameters", "Local variables", "Relevant runtime objects", "Relationships"]) {
      expect(text).toContain(s);
    }
    expect(text).toContain("main()");
    expect(text).toContain("main.cpp:81");
    expect(text).toContain("list");
  });
});

// ---- 5-6. runtime-aware: the state at THIS step, with resolved pointers ----------------

describe("runtime awareness", () => {
  it("shows the frame's variables as they are at the current step", () => {
    const early = frameBody(model(atEvent20, frameEntity()));
    const late = frameBody(model(atEvent80, frameEntity()));
    expect(early.locals[0].summary).toBe("→ #1");
    expect(late.locals[0].summary).toBe("→ #3");
    // Same selection, different step: the model remembers nothing but the selection.
    expect(model(atEvent20, frameEntity()).step).toBe(20);
    expect(model(atEvent80, frameEntity()).step).toBe(80);
  });

  it("resolves pointer relationships through variables and objects, by field name", () => {
    const c = frameBody(model(atEvent80, frameEntity()));
    expect(c.relationships.map((r) => `${r.from} → ${r.to}`)).toEqual(["list → #3", "#3.next → #7"]);
    // `null` next of #7 is not a relationship.
    expect(c.objects.map((o) => [o.id, o.ty])).toEqual([[3, "Node"], [7, "Node"]]);
    const text = html(model(atEvent80, frameEntity()));
    expect(text).toContain("list");
    expect(text).toContain("#3.next");
  });

  it("reaches objects only through the frame's own variables", () => {
    const g = at(80, { threads: stackOf(mainFrame(3)), objects: [n3(), n7(), node(9, 1, ptr("next", null), "alive", life(70))] });
    const c = frameBody(model(g, frameEntity()));
    expect(c.objects.map((o) => o.id)).toEqual([3, 7]); // #9 is somebody else's
  });

  it("terminates on a cycle", () => {
    const g = at(80, {
      threads: stackOf(mainFrame(3)),
      objects: [node(3, 10, ptr("next", 7)), node(7, 20, ptr("next", 3))],
    });
    const c = frameBody(model(g, frameEntity()));
    expect(c.relationships.map((r) => `${r.from} → ${r.to}`)).toEqual(["list → #3", "#3.next → #7", "#7.next → #3"]);
    expect(c.objects.map((o) => o.id)).toEqual([3, 7]);
  });

  it("addresses array elements and names the element a pointer designates", () => {
    const array = obj(1, arr("int[3]", [scalar("[0]", "int", "0"), scalar("[1]", "int", "10"), scalar("[2]", "int", "20")]));
    const g = at(9, {
      threads: stackOf(frame(1, "main", [variable("p", 50, ptr(null, 1, "[1]", false, "object", "int*"))])),
      objects: [array],
    });
    const c = frameBody(model(g, frameEntity()));
    expect(c.relationships.map((r) => `${r.from} → ${r.to}`)).toEqual(["p → #1[1]"]);
    // Following it selects the array with that element highlighted.
    const o = objectBody(model(g, frameEntity(), c.relationships[0].entity));
    expect(o.ty).toBe("int[3]");
    expect(o.highlight).toBe("[1]");
    expect(o.rows.filter((r) => r.path === o.highlight).map((r) => [r.label, r.text])).toEqual([["[1]", "10"]]);
  });
});

describe("past-the-end links", () => {
  it("names a pointer one past the end of an array as that index, not as freed or lost", () => {
    // A container's `end` pointer: the model calls the place out of bounds, which is not dangling.
    const array = obj(1, arr("int[2]", [scalar("[0]", "int", "10"), scalar("[1]", "int", "20")]));
    const g = at(9, {
      threads: stackOf(
        frame(1, "main", [
          variable("first", 50, ptr(null, 1, "[0]", false, "object", "int*")),
          variable("last", 51, ptr(null, 1, "[2]", false, "object", "int*")),
        ]),
      ),
      objects: [array],
    });
    const c = frameBody(model(g, frameEntity()));
    expect(c.relationships.map((r) => `${r.from} → ${r.to}`)).toEqual(["first → #1[0]", "last → #1[2]"]);
    expect(c.relationships.every((r) => !r.dangling)).toBe(true);
    // Following it selects the array with that index as the focus; nothing there to highlight, no crash.
    const o = objectBody(model(g, frameEntity(), c.relationships[1].entity));
    expect(o.highlight).toBe("[2]");
    expect(o.rows.some((r) => r.path === "[2]")).toBe(false);
    expect(o.referencedBy.map((r) => `${r.from} → ${r.to}`)).toEqual(["first → #1[0]", "last → #1[2]"]);
  });
});

// ---- variable and object drill-down ----------------------------------------------------

describe("variable and object drill-down", () => {
  it("a variable shows its type, value and what it points at", () => {
    const m = model(atEvent80, frameEntity(), listVar);
    const v = m.body as VariableContext;
    expect(v.kind).toBe("variable");
    expect([v.name, v.ty, v.summary, v.variableKind]).toEqual(["list", "Node*", "→ #3", "local"]);
    expect(v.frame?.function).toBe("main");
    expect(v.targets.map((t) => t.id)).toEqual([3]);
    expect(v.targets[0].rows.map((r) => [r.label, r.text ?? r.link?.label])).toEqual([["value", "10"], ["next", "#7"]]);
    const text = html(m);
    expect(text).toContain("Node*");
    expect(text).toContain("→ #3");
  });

  it("an object shows type, status, lifetime and fields, and the pointers that lead to it", () => {
    const o = objectBody(model(atEvent80, frameEntity(), listVar, objectEntity(3)));
    expect([o.ty, o.status, o.alive, o.storage]).toEqual(["Node", "alive", true, "heap"]);
    expect(o.lifetime?.allocatedStep).toBe(40);
    expect(o.rows.map((r) => [r.label, r.text ?? `→ ${r.link!.label}`])).toEqual([["value", "10"], ["next", "→ #7"]]);
    expect(o.referencedBy.map((r) => `${r.from} → ${r.to}`)).toEqual(["list → #3"]);
  });

  it("follows relationships: frame → variable → object → next object, one panel", () => {
    let trail: Trail = inspectorReducer([], { type: "open", entity: frameEntity() });
    trail = inspectorReducer(trail, { type: "drill", entity: listVar });
    trail = inspectorReducer(trail, { type: "drill", entity: objectEntity(3) });
    trail = inspectorReducer(trail, { type: "drill", entity: objectEntity(7) });
    const m = buildInspector(atEvent80, trail)!;
    expect(m.crumbs.map((c) => c.label)).toEqual(["main()", "list", "Node #3", "Node #7"]);
    expect(objectBody(m).id).toBe(7);
    expect(objectBody(m).referencedBy.map((r) => `${r.from} → ${r.to}`)).toEqual(["#3.next → #7"]);
    // A breadcrumb goes back, dropping what came after it.
    expect(inspectorReducer(trail, { type: "goto", index: 1 })).toEqual([frameEntity(), listVar]);
    expect(inspectorReducer(trail, { type: "goto", index: 99 })).toBe(trail);
  });

  it("following a pointer round a cycle goes back instead of growing the trail", () => {
    let trail: Trail = [frameEntity()];
    for (const id of [3, 7, 3, 7, 3]) trail = inspectorReducer(trail, { type: "drill", entity: objectEntity(id) });
    expect(trail).toEqual([frameEntity(), objectEntity(3)]);
  });

  it("keeps the trail bounded", () => {
    let trail: Trail = [];
    for (let i = 0; i < MAX_TRAIL + 20; i++) trail = inspectorReducer(trail, { type: "drill", entity: objectEntity(i) });
    expect(trail).toHaveLength(MAX_TRAIL);
    expect(trail[trail.length - 1]).toEqual(objectEntity(MAX_TRAIL + 19));
  });

  it("asks the backend to describe exactly the objects being inspected", () => {
    expect(focusObjects([])).toEqual([]);
    expect(focusObjects([frameEntity()])).toEqual([]); // a frame needs nothing extra
    expect(focusObjects([frameEntity(), listVar, objectEntity(3), objectEntity(3, ".1"), objectEntity(7)])).toEqual([LIST_VAR, 3, 7]);
  });
});

// ---- 7. lifetime ----------------------------------------------------------------------

describe("object lifetime", () => {
  it("says an object was freed, and at which event, using the lifetime the model keeps", () => {
    const o = objectBody(model(atEvent106, frameEntity(), objectEntity(3)));
    expect(o.alive).toBe(false);
    expect(o.status).toBe("freed at event #106");
    const text = html(model(atEvent106, frameEntity(), objectEntity(3)));
    expect(text).toContain("freed at event #106");
    expect(text).toContain("no longer exists");
    // Its last known contents are still shown, not hidden.
    expect(o.rows.map((r) => r.label)).toEqual(["value", "next"]);
  });

  it("shows links to a freed object as dangling, everywhere they appear", () => {
    const c = frameBody(model(atEvent106, frameEntity()));
    expect(c.locals[0].summary).toBe("→ #3 (freed)");
    expect(c.relationships[0]).toMatchObject({ from: "list", to: "#3", dangling: true });
    expect(c.objects.find((o) => o.id === 3)?.state).toBe("destroyed");
    expect(html(model(atEvent106, frameEntity()))).toContain("freed");
  });

  it("describes other ways an object can end", () => {
    const ended = (reason: "scopeExit" | "frameExit" | "programExit") => {
      const g = at(50, {
        threads: stackOf(mainFrame(null)),
        focus: [obj(5, scalar(null, "int", "1"), "destroyed", life(10, 49, reason))],
      });
      return objectBody(model(g, objectEntity(5))).status;
    };
    expect(ended("scopeExit")).toBe("went out of scope at event #49");
    expect(ended("frameExit")).toBe("destroyed when its function returned at event #49");
    expect(ended("programExit")).toBe("destroyed at program exit at event #49");
  });

  it("still describes a freed object nothing points at any more (it is only in `focus`)", () => {
    // After the last pointer is overwritten, the canvas stops drawing #3; the inspector keeps it.
    const g = at(110, { threads: stackOf(mainFrame(null)), objects: [n7()], focus: [n3("destroyed")] });
    const o = objectBody(model(g, frameEntity(), objectEntity(3)));
    expect(o.status).toBe("freed at event #106");
    expect(o.referencedBy).toEqual([]);
  });

  it("says when an object does not exist yet at this step", () => {
    const m = model(atEvent20, frameEntity(), objectEntity(18));
    expect(m.body).toMatchObject({ kind: "missing", title: "#18" });
    expect((m.body as { reason: string }).reason).toContain("event #20");
    expect(m.crumbs[1].missing).toBe(true);
  });
});

// ---- 8. the timeline moves, the context follows ----------------------------------------

describe("timeline synchronization", () => {
  it("the same selection is re-resolved at every step", () => {
    const trail: Trail = [frameEntity(), objectEntity(3)];
    const steps = [atEvent80, atEvent105, atEvent106].map((g) => buildInspector(g, trail)!);
    expect(steps.map((m) => m.step)).toEqual([80, 105, 106]);
    expect(steps.map((m) => (m.body as ObjectContext).status)).toEqual(["alive", "alive", "freed at event #106"]);
    expect(html(steps[2])).toContain("at event 106 / 128");
  });

  it("a frame that has returned is reported as gone, by name", () => {
    const callee = frame(2, "helper", [], { line: 5 });
    const during = at(30, { threads: stackOf(mainFrame(null), callee) });
    const after = at(31, { threads: stackOf(mainFrame(null)) });
    expect(frameBody(model(during, frameEntity(2, "helper"))).function).toBe("helper");
    const m = model(after, frameEntity(2, "helper"));
    expect(m.body).toMatchObject({ kind: "missing", title: "helper()" });
    expect(html(m)).toContain("not on the stack");
    expect(m.crumbs[0]).toMatchObject({ label: "helper()", missing: true });
  });

  it("a variable whose frame has gone says so", () => {
    const after = at(31, { threads: stackOf(mainFrame(null)) });
    const m = model(after, { kind: "variable", frameId: 2, name: "n", object: 200 });
    expect(m.body).toMatchObject({ kind: "missing", title: "n" });
    expect((m.body as { reason: string }).reason).toContain("not on the call stack");
  });

  it("a variable tracks its name across rebinding (a fresh `i` per loop iteration)", () => {
    const loop = (object: number, value: string) =>
      at(40, { threads: stackOf(frame(1, "main", [variable("i", object, scalar(null, "int", value))])) });
    const sel: SelectedRuntimeEntity = { kind: "variable", frameId: 1, name: "i", object: 10 };
    expect((model(loop(10, "1"), sel).body as VariableContext).summary).toBe("1");
    expect((model(loop(11, "2"), sel).body as VariableContext).summary).toBe("2");
  });

  it("waits for an object it has asked for rather than flashing 'does not exist'", () => {
    const m = model(atEvent80, frameEntity(), objectEntity(3_000));
    expect(html(m, true)).toContain("Loading");
    expect(html(m, false)).toContain("does not exist");
    // A frame never needs the extra fetch, so it is shown straight away.
    expect(html(model(atEvent80, frameEntity()), true)).toContain("main.cpp:81");
  });
});

// ---- 9-10. closing and switching -------------------------------------------------------

describe("closing and switching", () => {
  it("closing leaves the visualization exactly as it was", () => {
    const before = layoutGraph(atEvent80, metricsFor(7));
    const opened = inspectorReducer([], { type: "open", entity: frameEntity() });
    expect(layoutGraph(atEvent80, metricsFor(7))).toEqual(before); // opening does not move anything
    const closed = inspectorReducer(opened, { type: "close" });
    expect(closed).toEqual([]);
    expect(buildInspector(atEvent80, closed)).toBeNull();
    expect(layoutGraph(atEvent80, metricsFor(7))).toEqual(before);
    // The inspector never touches the position: it only ever reads `graph.step`.
    expect(buildInspector(atEvent80, opened)!.step).toBe(atEvent80.step);
    // Closing twice is a no-op (and does not even change identity, so nothing re-renders).
    expect(inspectorReducer(closed, { type: "close" })).toBe(closed);
  });

  it("clicking a different frame switches the context", () => {
    const g = at(30, {
      threads: stackOf(mainFrame(null), frame(2, "helper", [variable("n", 200, scalar(null, "int", "4"), "parameter")], { line: 5 })),
    });
    let trail: Trail = inspectorReducer([], { type: "open", entity: frameEntity(1, "main") });
    expect(frameBody(buildInspector(g, trail)!).function).toBe("main");
    trail = inspectorReducer(trail, { type: "open", entity: frameEntity(2, "helper") });
    expect(trail).toHaveLength(1); // a new trail, not a deeper one
    const m = buildInspector(g, trail)!;
    expect(frameBody(m).function).toBe("helper");
    expect(frameBody(m).parameters.map((p) => p.name)).toEqual(["n"]);
    expect(m.crumbs.map((c) => c.label)).toEqual(["helper()"]);
  });

  it("the panel renders breadcrumbs and a close control", () => {
    const text = html(model(atEvent80, frameEntity(), listVar));
    expect(text).toContain('aria-label="Close inspector"');
    expect(text).toContain("main()");
    expect(text).toContain('aria-current="true"'); // the current crumb is not a button
  });
});
