# Universal runtime model

Code: `src-tauri/src/model/`. Tests: `src-tauri/tests/runtime_model.rs`.
Status: **the data model only.** Nothing produces events yet: there is no
instrumentation, no renderer, no layout and no data-structure recognition.

## 1. Why a universal model

Lattice must visualize *arbitrary* C++ without being told what the program is.
A `LinkedListVisualizer` / `TreeVisualizer` per textbook structure cannot
handle a user's own `struct Employee { Employee* boss; Employee* reports[8]; }`.
So the core does not describe data structures. It describes what a C++ runtime
actually contains: **objects with typed values, variables that name them,
pointers/references between them, frames, lifetimes.** A "linked list" is just
`Object#1.next -> Object#2`. Any recognition or presentation is a later,
optional layer on top.

## 2. Architecture

```
 C++ program
     |   (future) instrumentation: src-tauri/src/runtime/instrumentation.rs
     v
 RuntimeEvent stream ................ serde data; the ONLY way state enters
     |
     v
 RuntimeState  <----- apply(event) (validated, all-or-nothing)
     |   ^
     |   '-- Timeline: event log + periodic checkpoints
     v
 RuntimeSnapshot ..... immutable, cheap to clone, "state after event N"
     |
     v   (future) visualization model -> layout -> renderer      (UI side)
```

Dependency rule, enforced by a test: `model` depends on `std` and `serde` only.
It knows nothing of Tauri, processes, the toolchain, sessions, colors,
positions or layout. Downstream code needs nothing of processes, compilers or
raw memory: it sees ids, types, values and links.

Responsibilities of the model: hold a consistent picture of one run, accept
events, reject inconsistent ones, answer structural queries (what does this
object point to / who points to it / is that pointer dangling), and produce
snapshots at any step. It does **not** observe programs, read memory, decide
what is interesting, or draw anything.

## 3. Object identity

`ObjectId` is a logical id, unique within a run and never reused. **The
observer assigns it**, because only it sees addresses and can tell "same
object" from "a new object that got a recycled address". The model never maps
addresses to objects and never dereferences anything. `address` and `size` are
optional metadata on `Object`. Consequence: after `delete p; q = new Node;`
(same address), `p` still designates the old, destroyed `ObjectId`, so it stays
dangling, and `q` designates a different one (test
`address_reuse_does_not_resurrect_a_dangling_pointer`).

An `ObjectId` names a **root storage object** (a variable's storage, a heap
allocation, a global, a temporary). Fields and array elements are *not* given
ids; they are addressed by a **`Place`** = root object + path
(`Field(i)` / `Index(i)`). That is what lets `&node.next` and `&arr[2]` be
expressed, and keeps a million-element array from costing a million ids.

Other ids (`VariableId`, `FrameId`, `ScopeId`, `TypeId`) follow the same rules.
Inline nested members (A contains B contains C) are part of A's value tree, not
separate objects.

## 4. Types

`TypeTable` holds `TypeDef`s (id, compiler-spelled display name, size,
cv-qualifiers, `TypeKind`): primitives, records (struct/class/union, fields
with bases first, template arguments), enums, pointers, lvalue/rvalue
references, arrays, function types, opaque. The observer supplies them from the
compiler's own type information; Lattice never parses a type string. Types are
declared by `TypeDeclared` events, may reference each other in any order
(recursion works), and are stored once; objects carry only a `TypeId`. STL
types are ordinary records with `template_args`; no STL special-casing exists.

## 5. Values

`Value` is a tree: int, uint, float, bool, char, string, enum, pointer,
reference, aggregate (positional fields), array, union. Values carry no type
and no field names (those come from the type table), so nothing is duplicated
per element. **Absence is its own variant**, `Unavailable { Unknown,
Uninitialized, OptimizedAway, Unreadable, OutOfScope, Invalid{raw} }`, which
can never compare equal to `0`, `nullptr`, `""` or any real value.

## 6. Variables vs objects

A `Variable` is a *name* bound to a storage `Object`; it never holds the value.
`int x` names an `int` object, and `&x` can point at that object. So
`Node* head` is: variable `head` -> object (type `Node*`, pointer value) ->
`Object#1` (the node). Two variables (or ten pointers) can designate one object
and aliasing is preserved by construction, because they hold the same
`ObjectId`. Cost: one extra hop from a variable to the thing its pointer
targets, available as `variable_object` / `variable_value` / `outgoing`.

## 7. Pointers, references, links

A pointer is `PointerValue { address?, target }`, a reference is
`Reference { target }`. `Target` is `Null`, `Place(place)` or `Unresolved`
(non-null but untracked: function pointers, one-past-the-end, integers cast to
pointers). Links are structured, never strings.

Queries: `outgoing(object)` walks the object's value (O(object size));
`incoming(object)` uses a reverse index maintained incrementally
(O(result), no global scan). Both return `Edge { source: Place, kind:
Pointer|Reference, target: Place }`. Two incoming edges with one `target` are
aliases of each other; there is no separate "alias" relation.

**Relationships are derived from values, never stored separately**, so they
cannot disagree with them. Deliberately *not* modelled now: `owns` (needs
smart-pointer semantics; infer later), `member_of`/`contains` (a `Place` path
already is containment), `aliases` (derivable as above).

**Dangling is derived, not stored.** `target_status(target)` returns `Null`,
`Live`, `Dangling` (root object destroyed), `OutOfBounds` (alive, path no
longer resolves) or `Unresolved`. Freeing an object therefore rewrites nothing.

## 8. Events

`RuntimeEvent { seq, thread, timestamp_ns?, location?, kind }`. Kinds:
`TypeDeclared`, `FunctionEntered/Exited`, `ScopeEntered/Exited`,
`ObjectAllocated`, `ObjectConstructed`, `ObjectDestroyed`, `VariableCreated`,
`VariableDestroyed`, `ValueChanged`, and `ObservationTruncated`: the observer's last
event when it hit its event budget and stopped. The program ran on unobserved, so
the state stops being "as far as we know" there (`truncated_at()`), and a UI must
not present it as the end of the run.

Decisions to note:
- **One write event.** Field / pointer / array-element / variable changes are
  all `ValueChanged{place, value}`; they differ only in path and value shape.
- **New state only.** The old state is the state at `seq-1`; carrying both
  doubles size and can disagree.
- No `ReferenceCreated`: creating a reference = creating an object whose value
  is `Reference`.
- Events are validated completely before any mutation; a rejected event leaves
  the state unchanged (`ApplyError`). Unknown ids, unknown types, pointers to
  never-reported objects, bad paths, duplicate ids, out-of-order frames/scopes,
  double-destroy and writes after destruction are all errors. Untracked memory
  must be reported as `Unresolved`, which keeps observer bugs loud.
- Serialization: plain `serde` (JSON in tests), no Qt/Tauri types. `RuntimeState`
  itself is deliberately *not* serializable; it is derived from events.
  Float NaN/infinity cannot travel in JSON; a JSON transport must map them.

## 9. Snapshots and the timeline

`RuntimeSnapshot` is an immutable, shared (`Arc`) view of a `RuntimeState`;
`snapshot.sequence()` is the last applied event. `Timeline` keeps the event log
plus full state copies as checkpoints, taken once at least
`max(N, object count)` events have passed since the last one (`N` defaults to
256), so a copy is always paid for by at least a state's worth of events
(amortized O(1) per event). `snapshot_after(seq)` clones the nearest earlier
checkpoint and replays the rest, O(state), regardless of run length; `latest()`
is maintained incrementally. (A fixed interval was tried first and made ingesting
300k events with 20k live objects take two minutes; found by the observation
spike.) `initial()` is the empty state. Sequence numbers in a
`Timeline` must be `0,1,2,...`. Persistent (structurally shared) maps are the
next optimization if checkpoint cost matters; the API hides the representation.

## 10. Lifetime

`Lifetime { state: Allocated | Alive | Destroyed | Unknown, allocated_at,
ended_at?, end_reason? }` in event time, so a timeline can place "created at 14,
destroyed at 38" without replaying. `Allocated` = raw storage, constructor not
finished. `Destroyed` objects keep their last value (needed for dangling
pointers and history) but must not be shown as current; `end_reason` is
`Freed | ScopeExit | FrameExit | ProgramExit | Other`. Ending a scope or frame
ends its variables and any still-alive *automatic* objects bound to them; heap
objects outlive frames. Destroyed objects stay in the state for the rest of the
run (see limitations).

## 11. Stack frames and scopes

`Frame { function, caller, call_site, location, variables, open_scopes }`;
per-thread stacks, outermost first. `FunctionExited` must be the top frame;
`ScopeExited` the innermost scope. A frame's `location` tracks the most recent
event location while it is on top, so "what line is each frame on" is available.
No debugger functionality: frames are reported by the observer.

## 12. Source locations

`SourceLocation { file, line, column?, function?, instruction? }` (1-based;
strings are `Arc<str>` and shared). Attached to events and frames; `Object`
records its allocation site. The model never reads the file.

## 13. Threads

Every event carries a `ThreadId`; stacks are per thread; `seq` is a single total
observation order (not happens-before). No locking or race modelling.

## 14. Future instrumentation boundary

The instrumentation seam in `runtime/instrumentation.rs` (`Instrumenter`,
`BuildPlan`, `RuntimeEventStream`) is unchanged. `observe::ObservingInstrumenter`
is the first implementation that feeds this model (the trait itself still does
not mention it; the implementation hands events over through the stream's
`close()`). The contract for whatever fills it: produce `RuntimeEvent`s with
strictly increasing `seq`; assign logical ids; resolve addresses to `Place`s
itself; declare types before use; report untracked pointees as `Unresolved`;
emit destructor/`delete` as `ObjectDestroyed`. A transport (pipe/socket) only
needs to carry serde JSON events; `RuntimeEventStream` will gain a
`next_event() -> Option<RuntimeEvent>`-style method when a strategy exists.
Because the model never touches memory, instrumentation cannot make the model
unsafe.

## 15. Future visualization boundary

A visualization layer consumes `RuntimeSnapshot` (`objects`, `variables`,
`frames`, `outgoing`, `incoming`, `target_status`, `types`) and produces its own
model (nodes, edges, layout). It must not need addresses or process details.
Export of a snapshot to the frontend needs its own DTO (not built yet).

## Performance notes

- Entities live in `BTreeMap`s keyed by id (deterministic order); strings are
  shared (`Arc<str>`) or held once (type table, field names).
- `incoming` is an incrementally maintained index; `write` adjusts it by the
  diff of the written subtree only.
- Graph traversal uses `outgoing`, O(size of each visited object); a full walk
  is O(V+E).
- `Value` is an owned tree, so a write clones the new value, and a snapshot
  clone copies every object. Fine for thousands of objects; millions want
  persistent maps and chunked/lazy arrays.
- Arrays are `Vec<Value>`: a 1M-element array is ~1M `Value`s per copy.

## Limitations and decisions to review before instrumentation

1. **Variables bind to storage objects** (extra hop). Alternative: inline scalars
   in the variable. Kept for `&x` correctness.
2. **Values are untyped trees validated only structurally**: the model does not
   check a value against its `TypeId` (e.g. an `int` object holding a string).
3. **Destroyed objects are never evicted**: memory grows with total allocations.
   Needs a retention policy for long runs.
4. **Checkpoints are full copies** (adaptively spaced); see Snapshots.
5. **Function pointers, one-past-the-end and bit-cast pointers** are
   `Unresolved`; there is no `Function` target or past-the-end place.
6. **Arrays are fixed-size after allocation** except by replacing the whole value;
   `std::vector` growth (reallocation) will be modelled as buffer object +
   `ValueChanged`, decided when STL support is designed.
7. **Union** active-member tracking is observer-provided only.
8. **Strict frame/scope nesting**: `longjmp`, exceptions unwinding many frames,
   and coroutines need explicit events per frame/scope exited.
9. **No per-thread or per-allocation-site id ranges**; the observer must keep ids
   unique across threads.
10. **Wire format** is JSON-shaped serde; a denser encoding can be chosen later.
    `RuntimeEventStream` does not yet carry `RuntimeEvent`.
