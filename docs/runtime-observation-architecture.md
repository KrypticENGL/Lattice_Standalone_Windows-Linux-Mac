# Runtime observation architecture

Status: **architecture spike, with the first three implementation stages done.**
This document compares the ways to observe a running C++ program, recommends
one, and describes what validates it. Code: `src-tauri/src/observe/`. Tests:
`src-tauri/tests/observation.rs` (heap objects, transport) and
`src-tauri/tests/observation_frames.rs` (functions, scopes, variables). The model
it feeds is the existing Universal Runtime Representation (URR):
`docs/runtime-model.md`.

- **Stage 1 (heap):** `new`/`delete`, field writes, pointers, lifetimes.
- **Stage 2:** a live **named-pipe** transport with batching and crash-safe
  flushing; **function frames, lexical scopes, parameters and local variables**
  (including loop variables, references, uninitialized variables, arrays);
  compound assignment and `++`/`--`.
- **Stage 3:** observation **in the application**: an Observe
  toggle, a per-run opt-in, a clear message when libclang is missing, an **event
  budget with a truncation marker in the URR**, and a Runtime tab that steps
  through the recording as text. Verified by driving the real app (section 12).
- **Stage 4 (this update):** the **visualization**: a layout-free view model built
  from a URR snapshot (`viz`), a generic layered layout and an SVG renderer, a
  step slider with playback and keyboard control, and the current source line
  highlighted in the editor. Verified by driving the real app and looking at it
  (section 12).

How to read the evidence. Statements tagged **[verified]** were checked on this
machine during the spike (the check is named). Statements tagged **[knowledge]**
are background knowledge that was *not* verified here and should be re-checked
before anything depends on them. Nothing is claimed without one of the two.

---

## 1. Summary

**Recommendation.** Observe at the **source level, at compile time**, with a
small **in-process runtime library**:

```
libclang (analysis)  ->  insertion-only source rewrite  ->  the EXISTING compiler
                                                            + lattice-runtime
```

- **libclang** understands the user's C++ (types, fields, `new`/`delete`,
  assignments, source ranges). It only *analyses*; it never compiles.
- The rewrite **only inserts text** (no deletion, no moved code, no added
  lines), so semantics are preserved by construction and line numbers stay the
  user's line numbers.
- The instrumented program is built by **whatever compiler the execution
  manager already uses** (verified: GCC 15, Clang 18, MSVC 2022).
- **lattice-runtime** (plain C++, no Lattice or UI dependencies) owns logical
  object identity, event generation, buffering and transport, and writes events
  in **the URR's own serde JSON shape**, so the Rust side deserializes straight
  into `RuntimeEvent`. There is no second event model.
- **Transport:** a Windows **named pipe**, live, implemented and tested (the
  default on Windows); a file transport remains as a fallback behind the same
  interface. Events are batched in the runtime and flushed by a small background
  thread, at exit, and on crashes.
- A **debugger** is *not* part of the core. It is a later complement (verifier,
  un-rebuildable binaries, post-mortem), not the primary mechanism.
- **LLVM IR instrumentation** is the planned fallback if source-level blind
  spots prove too costly; it would reuse the same runtime library and events.

**What was built.** Ordinary C++ (your Node sample, unmodified) goes in; the
existing URR comes out: real ordering, source locations, logical ids, pointer
relationships (including the cycle), lifetimes, **and now the program around the
heap**: the call stack with each frame's parameters and locals, block scopes
that begin and end, loop counters that count, references, and pointers from the
heap into the stack and back. In the app, ticking **Observe** and pressing Run
records the run; the Visualization panel draws the program's state (call stack,
heap objects, pointers as arrows) at any step, with the matching line highlighted in
the editor. 144 tests pass across the project (section 12).

**What was not built** (by design): arrays allocated with `new[]`, STL
containers, placement new, threads, optimized-build support, destructors as
separate events, lambdas' own frames, `static` locals, visualization. Section 9
is an honest account of what each needs.

**Real findings worth reading first:**
1. Stock `clang -ast-dump=json` does not scale (233 MB for a program that
   includes `<iostream>`), which is why libclang is the frontend (section 3.1).
2. The first spike exposed a quadratic-cost bug in the URR's snapshot
   checkpointing (126 s to ingest 300k events), fixed (section 13).
3. libclang cannot parse GCC's intrinsics headers (pulled in by `<windows.h>`),
   which used to silently disable observation for such programs; errors inside
   *system* headers no longer do (section 12).
4. A live pipe makes the program's speed depend on how fast Lattice ingests:
   about 367k events/s in a release build, but only ~63k in an unoptimized one
   (section 10).
5. The model costs about **1.1 KB of peak memory per event** (1.07 GB for the
   940k-event loop), so an unbounded recording is not safe: runs now have an event
   budget (default 250,000) and the URR records where a recording was cut off
   (section 12).

---

## 2. What the URR needs from an observer

The observer's job is to report *facts* in URR terms. Mapping (existing types):

| URR concept | Fact the observer must supply | Prototype source of the fact |
| --- | --- | --- |
| `TypeDeclared` / `TypeDef` | name, kind, size, fields (name, type, offset) | compiler, via generated `sizeof`/`offsetof`/`decltype` |
| `ObjectAllocated` | logical id, type, storage class, address, size | the tagged allocation function + `new` expression type |
| `ObjectConstructed` | construction finished | wrapper around the `new` expression |
| `ValueChanged{place, value}` | which field/element changed, new value | wrapper around each assignment + post-construction read |
| `ObjectDestroyed{Freed}` | lifetime end | replaced `operator delete` |
| `SourceLocation` | file, line, column, function | libclang ranges baked into call sites |
| `FunctionEntered/Exited` | function boundaries | RAII frame guard at the top of each body |
| `ScopeEntered/Exited` | block boundaries | RAII scope guard at the top of each block that declares variables |
| `VariableCreated` + `ObjectAllocated(Automatic)` | a named local/parameter and its storage | hook after each declaration statement; parameters at function entry |
| variable/object end | end of scope/function | **implied**: the URR ends a scope's variables and their automatic objects itself, so the runtime emits only the scope/frame exit |

Your conceptual target events map as follows: `OBJECT_ALLOCATED` ->
`ObjectAllocated`; `FIELD_CHANGED` and `POINTER_CHANGED` -> `ValueChanged`
(the place is the field path, the value's shape says whether it is a pointer);
`OBJECT_DESTROYED` + `OBJECT_DEALLOCATED` -> one `ObjectDestroyed{reason:
Freed}`. The URR has a single lifetime-end event. The destructor-ran vs
storage-released distinction only exists for non-trivial destructors; see the
decision list in section 14.

---

## 3. The approaches

### 3.1 Clang AST / source instrumentation

Parse with Clang, rewrite the source to call into a runtime library, compile the
rewritten source normally.

| Aspect | Assessment |
| --- | --- |
| Observes | Anything expressible as a source construct: declarations, scopes, `new`/`delete`, assignments, calls, constructors/destructors |
| Types | Exact and semantic (templates, typedefs, qualified names) straight from the compiler frontend |
| Source locations | Exact, including column, function; no debug info needed |
| Variables / fields | Variables: yes (declaration sites, scope via RAII guards). Fields: static identity from `FieldDecl`, runtime identity by address containment (section 6.B) |
| Pointers / references | Values read at write sites through compiler-produced layouts; references are a declaration fact |
| Allocations | `new`: type and site known exactly. `malloc`/`free`: via call-site wrappers |
| Lifetimes / ctor / dtor | Ctor: bracketed by the `new` wrapper or declaration hook. Dtor: user destructor bodies are instrumented source; end-of-life from RAII guards/`operator delete` |
| Stack variables | Yes (declaration hooks + scope guards); implemented in stage 2 |
| Arrays | Element writes yes; bulk writes (`memcpy`, `std::fill`) need call-site handling |
| Aliasing | Natural: pointer values resolve to logical ids |
| Optimized builds | Source-level facts do not change with `-O`; see section 8 |
| STL / custom types | Custom: descriptors generated from the AST. STL: semantic adapters written once in the runtime against public APIs (implementation-independent) |
| Performance | Per-event cost only where instrumented; no single-stepping |
| Complexity | Medium-high: a C++ rewriter must be conservative (macros, templates, evaluation order) |
| Windows | libclang ships in official LLVM Windows releases **[knowledge]**; **[verified]** a libclang 18.1.7 loads and parses MinGW libstdc++ 15 headers here |
| Scalability | Good: same machinery extends to every construct; compiler-independent back end |

Weaknesses, honestly: blind to writes made by code it does not see (library
internals, `memcpy`, other modules); every rewrite rule is a chance to change a
program, so ambiguity resolves to "skip, don't guess"; macros and templates are
skipped today.

**Frontend choice inside this approach (measured).** The cheapest frontend is
stock `clang -Xclang -ast-dump=json` (no library needed). **[verified]** It
works for the Node sample (47 KB) but a program containing only
`#include <iostream> <vector> <cstdio>` produced a **233 MB** JSON dump, because
headers dominate and the dump cannot be limited to the main file. Reading that
is not viable, and the schema is not a stable interface **[knowledge]**.
**libclang** is a stable C ABI, can skip header subtrees in-process, and was
verified here (version 18.1.7, all needed entry points present, including
`clang_getCursorBinaryOperatorKind`). A purpose-built libTooling tool is the most
powerful option (macro-aware `Rewriter`, AST matchers) but needs a custom LLVM
build to ship; it is the long-term upgrade path behind the same analysis
interface.

### 3.2 LLVM IR instrumentation

Compile to LLVM IR with Clang, run an instrumentation pass (every store, call to
allocation functions, ...), then code-generate.

| Aspect | Assessment |
| --- | --- |
| Observes | *Every* load/store in the translation unit, including instantiated STL templates and inlined code; allocations as calls to `operator new` |
| Types | Struct layouts in IR; **field names and C++ types only from debug-info metadata** (`DICompositeType`), so `-g` becomes load-bearing |
| Source locations | Line/column from `!dbg` metadata (needs `-g`) |
| Variables | `alloca` + `llvm.dbg.declare` at `-O0`; largely lost at `-O1+` |
| Fields | GEP indices into a named struct plus debug info to name them. **[verified]** `-O0` IR for the Node sample shows `%struct.Node = type { i32, ptr }`, `getelementptr inbounds %struct.Node, ptr %4, i32 0, i32 1`, `store ptr null, ...`, and `call ... @_Znwy(i64 16) ... !heapallocsite` |
| Pointers / aliasing | Observable as raw stores of pointer values; logical meaning needs the same registry a source-level design uses |
| Allocations / lifetimes | Allocation calls yes; object *lifetime* (constructor completion, scope end) only via lifetime markers and cleanups, which Clang emits at `-O0` only in special modes **[knowledge]** |
| Ctor / dtor | Visible as ordinary calls |
| Optimized builds | Pass placement decides what survives; transformations already applied to IR |
| STL | Sees raw internals (`_M_start`/`_M_finish`), noisy and implementation-specific |
| Performance | Instruments every store: heavy; filtering by tracked address needs a runtime check per store |
| Complexity | High: needs an LLVM pass **built into a Clang that Lattice ships** |
| Windows | Out-of-tree pass plugins are not usable with official Windows LLVM builds (plugin symbol export is off) **[knowledge]**, so this implies a custom Clang build/bundle tied to one LLVM version. **[verified]** `clang -S -emit-llvm` works with the Clang 18 found here |
| Scalability | Strongest coverage of raw memory writes; weakest at *semantic* facts (scopes, logical objects, containers) |

### 3.3 Debugger-based observation

Run the program under a debugger (GDB, LLDB, or the Windows debug API), read
state with its type information.

| Aspect | Assessment |
| --- | --- |
| Observes | State at stop points: frames, locals, arguments, anything reachable, with no modification of the program |
| Types | Complete, from debug info (DWARF or PDB) via the debugger |
| Source locations | Exact via line tables |
| Variables / stack | The strongest of all approaches: real frames, real locals, real scopes (`-O0`) |
| Fields | Read by walking typed values |
| Allocations | Breakpoints on `operator new`/`delete` (and return-address handling) |
| Lifetimes | Stack: from scope ranges; heap: from breakpoints; type of a heap block known only via a typed pointer that reaches it |
| Writes | **Not observable individually.** Hardware watchpoints: 4 x86 debug registers, 8 bytes each **[knowledge]**; otherwise single-stepping. Practical model: stop at each source line and *diff* reachable state |
| Granularity | Line-level, not write-level; "what happened between stops" is inferred |
| Optimized builds | Debug info at `-O2` exists but variables are frequently optimized out or live in registers |
| STL | Needs a pretty-printer per standard library implementation (libstdc++, libc++, MSVC STL), three separate ecosystems |
| Performance | Round trip per step (milliseconds); a million-iteration loop is out of reach without pruning |
| Complexity | High; two debug-info formats on Windows (see below) mean two debuggers |
| Windows | **[verified here]** GDB 16.3 (MinGW) is present; `lldb.exe` is present as JetBrains' bundle (9.0.0) and inside the Android NDK (version not established). GDB Python scripting availability was **not established** (my probe hung and was stopped). **[knowledge]** GDB reads DWARF only; MSVC/clang-cl output is PDB/CodeView, which LLDB (via DIA/native PDB reader) or DbgEng read |
| Scalability | Good for *state at a point*, poor for *every change* |

Where it shines, and why it is kept as a complement: it needs no rebuild, sees
real stack frames, and is independent of the compiler. The published
"typed reachability + line stepping" designs for C/C++ visualizers (Python Tutor
style) **[knowledge]** are this approach. They get away with it because their
programs are tiny and their step counts small.

### 3.4 Runtime support library alone

Interpose allocation (`operator new/delete`, `malloc`, IAT hooks) inside the
process.

- Sees allocation address, size, call stack, and deallocation. **Nothing about
  types or fields.** A call stack can be mapped to a source line, but not
  reliably to "the type being constructed".
- Essential as a *component* of every other approach (identity, buffering,
  transport); insufficient alone.
- Windows: `operator new` replacement works in an executable **[verified]**
  (the prototype replaces the global allocation functions and was built with
  three compilers). Hooking a DLL's own allocator is a separate problem
  (section 9).

### 3.5 Hybrids

| Hybrid | Verdict |
| --- | --- |
| **AST rewrite + runtime library** (recommended) | Semantic facts from the source, identity/transport from the runtime. The prototype. |
| AST rewrite + **debugger verifier** | Debugger reads real memory at checkpoints and compares with the URR: an excellent *test oracle* for missed writes. Not needed for correctness of the core. |
| **IR pass** + runtime library | Same runtime and events; a different front end. Adopt if source-level blind spots dominate (triggers in section 5.3). |
| **Dynamic binary instrumentation** (DynamoRIO-style) | Observes every write in any binary without rebuild, including optimized builds. Windows x64 support exists **[knowledge]**, but it yields raw memory events and needs DWARF/PDB parsing for types; heavy. A serious future option for "observe a binary we did not build". |
| Debugger as primary + runtime library | Gets locals for free but loses write-level ordering; see 3.3. |

### 3.6 Comparison at a glance

| Criterion | AST + runtime (chosen) | IR pass | Debugger | Runtime lib alone |
| --- | --- | --- | --- | --- |
| Type info quality | Excellent, semantic | Needs `-g` | Excellent (debug info) | None |
| Source locations | Exact | Needs `-g` | Exact | Weak |
| Write-level ordering | Yes (instrumented sites) | Yes (all stores) | No (line-level) | No |
| Stack variables / frames | Yes (to build) | Partial at `-O0` | Best | No |
| Heap objects / identity | Yes | Yes | Via breakpoints | Yes (untyped) |
| Ctor/dtor/scope boundaries | Natural (RAII guards) | Hard | From scopes | No |
| STL | Semantic adapters | Raw noise | Per-stdlib printers | No |
| Optimized builds | Source facts unchanged | Pass-dependent | Degraded | n/a |
| Coverage of uninstrumented writes | **Blind** | Strong (TU only) | Diff-based | Blind |
| Per-event overhead | Low-moderate | Moderate-high | Very high | Low |
| Implementation complexity | Medium-high | High (custom Clang) | High (2 debuggers) | Low |
| Ships with stock tools? | Yes (libclang) | No (custom build) | Mostly | Yes |
| Compiler-independent back end | **Yes** | No (Clang only) | DWARF vs PDB split | Yes |

---

## 4. Windows compatibility notes

| Topic | State |
| --- | --- |
| Clang / LLVM on Windows | Official builds exist for x64/ARM64 **[knowledge]**. A Clang still needs a C++ standard library and CRT from MinGW or MSVC (already noted in `docs/architecture.md`). **[verified]** a Clang 18.0.3 with default target `x86_64-w64-windows-gnu` exists here (Android NDK) and compiles the instrumented output |
| clang-cl | MSVC-compatible driver, emits CodeView/PDB **[knowledge]**. **[verified]** not installed: Visual Studio's `VC\Tools\Llvm\bin` contains only `clang-format`/`clang-tidy`. The source-level design does not need debug info, so it is unaffected |
| MinGW | The configured compiler here: GCC 15.2 (x86_64-posix-seh). **[verified]** it builds and runs the instrumented programs |
| MSVC | **[verified]** the runtime and an instrumented program build and run under MSVC 2022 `cl` (15 events, exit 0). Lattice's execution engine has no MSVC driver yet, so this is portability evidence only |
| LLDB | **[verified]** present here as JetBrains' 9.0.0 bundle and the NDK's. Not needed by the recommended design |
| GDB | **[verified]** 16.3 MinGW present. DWARF only **[knowledge]** |
| PDB/CodeView vs DWARF | Two formats in play on Windows (MSVC/clang-cl vs MinGW/Clang-gnu). A design that needs neither (the recommendation) avoids the split |
| Process APIs | The existing `process` module (Job Object, tree kill) is unchanged and reused. A debugger would add `DebugActiveProcess`/debug events on top of it |
| IPC | Named pipes, TCP loopback, files, shared memory all available; compared in section 6.D |
| Pass plugins | Not usable with official Windows LLVM **[knowledge]** (see 3.2) |

---

## 5. Recommended architecture

### 5.1 Components

```
Lattice Desktop (Tauri)                               Instrumented user process
+------------------------------------------+          +--------------------------------+
| Editor (Monaco + clangd)                 |          | user code (rewritten, same     |
|                                          |          |   lines, same semantics)       |
| Execution Manager        [existing]      |  compile | + lattice-runtime              |
|   workspace, compile, run, timeout, kill |--------->|     * logical object identity  |
|   Instrumenter seam      [existing]      |   run    |     * type registry            |
|                                          |          |     * event generation (URR    |
| Instrumentation Manager  [observe/]      |          |       JSON)                    |
|   libclang analysis -> rewrite ->        |          |     * buffering                |
|   BuildPlan (flags, runtime, sink path)  |          |     * transport (path)         |
|                                          |          |   no Qt, no UI, no Lattice     |
| Runtime Event Receiver   [observe/]      |<---------|   host types                   |
|   named pipe / file -> RuntimeEvent      | events   +--------------------------------+
|                                          | (JSON lines)
| URR                      [model/]        |
|   RuntimeState, Timeline, Snapshots      |
|                                          |
| Snapshot Manager  (Timeline checkpoints) |
| Visualization     (future; reads URR     |
|   snapshots only)                        |
+------------------------------------------+
```

Dependency rules: `model` depends on nothing (enforced by a test that scans its
sources); `observe` depends on `model`, the `runtime` seam and `process`; nothing
in `observe` or `model` depends on UI; the instrumented process depends on nothing
of Lattice; visualization depends on URR snapshots only, never on instrumentation.

### 5.2 Event flow

```
build:  user source --libclang--> analysis (records, new, delete, assigns)
                    --rewrite---> instrumented source (workspace copy)
        BuildPlan: -I rt -include lattice_runtime.h  lattice_runtime.cpp
                   env LATTICE_EVENT_SINK=<path>
        existing compiler builds it
run:    program executes; runtime writes one URR JSON event per line to the sink
        (process still subject to existing timeout / cancel / tree-kill)
receive: sink -> RuntimeEvent -> Timeline::push (validated, all-or-nothing)
         -> RuntimeSnapshot at any step
```

### 5.3 Decision record

| Question | Decision | Why |
| --- | --- | --- |
| Where to instrument | Source level, via libclang | Semantic facts (types, scopes, objects) are first-class; compiler-independent back end; stock, shippable tool; preserves lines |
| Why not IR first | Needs a custom Clang (no pass plugins on official Windows builds); ties to one LLVM and to Clang-only builds; semantic facts still need a registry; debug info becomes load-bearing | |
| Why not debugger first | Line-granular, not write-granular; three STL printer ecosystems; two debug formats on Windows; per-step round trips | |
| Runtime library | Yes, in-process | Identity and buffering must live where addresses are visible |
| Type/layout metadata | Generated `sizeof`/`offsetof`/`decltype` code compiled with the program | The layout is whatever the building compiler produced; no DWARF/PDB parser in Lattice |
| Wire format | URR's own serde JSON | No parallel event model; Rust deserializes directly; replaceable by a denser encoding without changing semantics |

**Triggers to revisit** (move toward IR, or add the debugger):
1. More than ~a few percent of real user programs lose important state to
   uninstrumented writes (library code, macros, templates) that cannot be fixed
   with call-site wrappers: add an IR pass behind the same runtime/events.
2. Users need to visualize programs they cannot rebuild, or optimized builds
   faithfully: add debugger/DBI observation as a *separate source of the same
   events*.
3. Rewrite-induced behaviour changes show up (evaluation-order, overload
   resolution): move the rewrite into libTooling (macro-aware `Rewriter`).

---

## 6. The five questions

### A. How is object creation detected?

`new Node{10, nullptr}` is rewritten to

```cpp
::lattice::rt::constructed(new (::lattice::rt::tag<Node>(SITE)) Node{10, nullptr})
```

- **Type:** known exactly from the AST (`CXXNewExpr`), carried by the tag.
- **Allocation, in real order:** the tag selects a *placement allocation
  function* (`operator new(size_t, const NewTag&)`) which runs **before** the
  constructor arguments are evaluated (guaranteed since C++17) and registers the
  block: address, size, a **fresh logical id**, then emits `ObjectAllocated`
  (state `Allocated`, value = correctly shaped but uninitialized).
- **Constructor/lifetime:** `constructed(...)` wraps the whole expression and
  runs when the constructor finishes: emits `ObjectConstructed` and the
  resulting field values. Constructor-body assignments in user code are
  instrumented source and appear live in between.
- **Address:** recorded as metadata on the object, never as its identity.
- **Coverage honesty:** only `new` expressions the rewriter understood become
  tracked objects. Placement new, `new[]`, class-specific `operator new`, macros
  and templates are skipped (left unobserved, not guessed).

### B. How are fields identified (vs. a generic memory write)?

Two cooperating facts:

1. **Static** (compile time): for each record, libclang lists its data members;
   generated code registers `{name, type, offsetof}` for each. (`Node::next` is
   a real field with a real offset, from the compiler that built the program.)
2. **Dynamic** (run time): each scalar/pointer assignment `lhs = rhs` is
   rewritten to `::lattice::rt::written(&(lhs = rhs), SITE)`. The wrapper
   returns the same lvalue, so the expression keeps its type and value category.
   The runtime takes the written address, finds the tracked object containing
   it (ordered map by address), and resolves the address to a **field path** by
   walking the registered layout: nested records and arrays give paths such as
   `Field(0).Field(1)` and `Field(1).Index(1)` (tested).

A write outside any tracked object (today: stack variables) is ignored, not
misattributed. A pointer's *static* type disambiguates "pointer to the object"
from "pointer to its first member" when both have the same address (`&o->in` vs
`o`); tested.

Debug metadata is **not** needed: the compiler's layout comes from
`offsetof`/`sizeof` compiled into the program itself. (The IR and debugger
approaches *do* depend on debug info for names and types.)

### C. How is logical identity preserved across address reuse?

- The **runtime** assigns `ObjectId` from a monotonic counter at allocation and
  never reuses it. Only the in-process runtime can tell "same object" from "new
  object at a recycled address", so identity is assigned there.
- A registry maps live address ranges to `(id, type)`. `delete` removes the entry
  and emits `ObjectDestroyed`; a later allocation at the same address gets a
  **new** id.
- Pointer *values* are resolved to ids **at the moment they are observed**
  (write or construction). A pointer stored while its target was alive keeps
  designating the old id after the target is freed, which the URR turns into
  "dangling" (`target_status`). It is never silently re-pointed at the new
  occupant of that address.
- **[verified]** 300 allocate/free rounds produced 300 objects over 48 distinct
  addresses, 44 of them reused, and 300 distinct ids; values stayed with their
  object (`every_allocation_gets_a_fresh_logical_id`). The URR-level rule is
  separately unit-tested.
- Limitation: a pointer value *read back later* that points at freed memory
  resolves to "untracked" (the registry no longer knows the address). Adding
  tombstones for recently freed ranges is a small future change.

### D. How do events reach Lattice?

| Transport | Live? | Survives a crash | Complexity | Security / environment | Runtime dependency |
| --- | --- | --- | --- | --- | --- |
| **Named pipe** (`\\.\pipe\lattice-<session>`) | Yes | Yes if flushed before death | Medium (server in Rust, unblock on early exit) | Local only; `REJECT_REMOTE_CLIENTS`; per-session unguessable name | `fopen` a path: none |
| Localhost TCP | Yes | Yes | Medium | Firewall prompts possible; any local process can connect | socket library (winsock) |
| Temporary file | No (read after exit, or poll) | Yes | Lowest | Disk I/O; nothing extra | `fopen` a path |
| Shared-memory ring | Yes, fastest | Partial | High (sync, resize, cleanup) | Local | Win32 APIs |
| Child's stdout/stderr | Yes | Yes | Low but collides with the program's own output | Shared with user output | none |

**Selected and implemented (stage 2): the named pipe**, the default on Windows
(`observe/pipe.rs`). The runtime only knows "write lines to the path in
`LATTICE_EVENT_SINK`"; Lattice decides whether that path is a pipe or a file, so
the file transport remains available (`Transport::File`) and is tested to give
the same result.

How the pipe server behaves, and the test that shows it:

| Property | How | Test |
| --- | --- | --- |
| **Live**: events are usable while the program runs | reader thread feeds an incremental decoder; `progress()` is readable mid-run | `live::events_arrive_while_the_program_is_still_running`: a program sleeps 2.5 s after its last event; Lattice had seen them before it exited |
| **Never hangs Lattice** if the program never connects (the runtime connects lazily, at its first event) | after the run, a throwaway client is connected to release the pending `ConnectNamedPipe`; the reader is then awaited only for a bounded time (3 s) | `live::a_program_that_never_emits_does_not_hang_lattice` |
| **No leaked thread** if the build fails and the program never runs | `PipeServer` ends its reader on `Drop` | `live::a_failed_build_does_not_leak_the_pipe_reader` |
| **Local only** | `PIPE_REJECT_REMOTE_CLIENTS`, one instance, a per-session name | (by construction) |
| **A bad event never blocks the program** | after the first rejected event the reader keeps *draining* (it stops applying, not reading) | `receiver` unit tests |
| **Hang or kill loses (almost) nothing** | the runtime's flusher thread writes pending events every 20 ms | `live::a_hung_program_keeps_its_events_after_a_timeout` |

**Backpressure** is deliberate: if Lattice reads more slowly than the program
writes, the program's writes block (no silent loss). It matters in practice:
ingest speed (parse + apply) is about **367k events/s in a release build** but
only ~63k/s in an unoptimized build, so in a debug build an event-heavy program is
slowed by the receiver (section 10). The reader thread does nothing but read and
decode; a larger queue between a reader and a decoder thread is the next step if
a release build ever becomes the bottleneck.

**Batching and crash safety (runtime side).** Events accumulate in a buffer that
is written when it reaches 64 KB, every 20 ms by a background flusher thread, at
normal exit (`atexit`), and on `abort()` and unhandled exceptions (access
violations). **[verified]** with a hand-instrumented program: `abort()`, a null
dereference and a hung program killed from outside all left every event on disk
(13 of 13). **Known loss:** `_Exit()` and `TerminateProcess` within ~20 ms of the
last event skip every flush point. The flusher is a Win32 thread on purpose:
`std::thread` would add a pthread link dependency that not every toolchain
satisfies implicitly (seen with the NDK's Clang here); on non-Windows builds the
runtime writes every event immediately instead.

### E. Where does instrumentation happen?

At **source level, at compile time**, via libclang analysis and an insertion-only
rewrite (sections 3.1 and 5.3). The IR route and the debugger route are kept as
explicit, separately-triggered fallbacks, not parallel implementations. Both
would feed the same runtime events, so the URR and everything above it do not
change if they are added.

---

## 7. Identity model and metadata strategy

**Identity.** `ObjectId` (runtime counter) names a *root storage object*;
sub-objects are `Place` paths. Stack objects, once observed, get ids the same
way (id at declaration). Variables and frames get their own ids. Addresses are
metadata. Ids are unique across the whole run (across threads too, once
threading exists: the counter must become atomic or per-thread ranges).

**Type metadata.** Names come from libclang (spelling, as written); layout comes
from code the *building compiler* compiles (`sizeof`, `offsetof`,
`decltype(T::member)`), so it is correct for that compiler, target and options
by construction. Primitives, pointers and arrays are described by C++ templates
in the runtime header (no generation needed); class types by a generated
function found by argument-dependent lookup; anything undescribed degrades to an
**opaque** type whose value is `Unavailable::Unknown`, never guessed.

**Source locations.** libclang ranges give offset/line/column; the rewrite
bakes them into call-site literals (`Site{"main.cpp",10,5,"main"}`).
Because no newline is ever inserted, the original line numbers are preserved
everywhere: runtime event locations, and compiler diagnostics of the instrumented
build (**[verified]**: `diagnostics_of_instrumented_builds_still_point_at_the_users_lines`).

---

## 8. Debug vs optimized builds

The prototype targets **`-O0` + debug info + instrumentation**, which is what the
execution manager already produces (`-O0 -g`). Reasons:

- At `-O0` every variable has a home in memory, every statement executes, and
  the source-to-behaviour correspondence is one-to-one. The first milestone is
  about proving the pipeline, not about surviving the optimizer.
- The educational use case wants the program the user *wrote*, not what the
  optimizer turned it into.

What optimization does to observation, per technique:

| Effect | Consequence |
| --- | --- |
| **Dead-store elimination** | A store whose value is never read may vanish. Debugger/IR/DBI observe that it did not happen; source-level hooks that take the address of the written object (`&(x = y)`) force the store to exist, so source-level events *do* survive (see below) but then the program is no longer the optimized one |
| **Inlining** | Frames disappear: a debugger shows an inlined frame only through debug info; IR sees the merged body; source-level frame events (RAII guards) still fire because they are real objects, at the cost of inhibiting some inlining |
| **Optimized-out variables** | Debuggers report `<optimized out>`; the URR has `Unavailable::OptimizedAway` for exactly this |
| **Register allocation** | Values live in registers with no address: invisible to address-based observation (runtime, watchpoints); visible to debuggers only via location lists |
| **Aliasing assumptions** | The optimizer may reorder/cache reads and writes assuming no alias; observed order may differ from source order for non-instrumented access |
| **Other transformations** (loop unrolling, vectorization, SROA, copy elision) | Objects may be split into scalars, merged, or never materialized; "object" stops being a stable concept |

**[verified]** for the Node sample, the hand-written equivalent of the
instrumented program compiled with `g++ -O2` produced the same 12 non-type
events, in the same order, as `-O0`. That is a *small* datum: the sample has
nothing to optimize away, and the instrumentation itself pins the stores. It
supports "source-level events are insensitive to optimization *for the constructs
we instrument*", not "optimized builds are fully observable". Full support is a
separate design (probably DBI or debugger-based, section 5.3).

---

## 9. Edge cases: how the recommended design handles them eventually

"Status" is what the prototype does today. "Eventually" is the plan, with the
honest limitation.

| Case | Status now | Eventually / limitation |
| --- | --- | --- |
| **Stack variables, frames, scopes** | **Observed** (tested): parameters and locals with values, per-call frames, block scopes that end, loop variables, `auto`, class-typed locals | Not yet: `static` locals and `thread_local` (skipped), structured bindings, variables declared in `if`/`while`/`switch` conditions, VLAs, anything inside lambdas (they get no frame of their own), `for` loops with a missing part, function-try-blocks, `constexpr` and template functions (skipped, never half-instrumented). A block holding a `case` label, and every `switch` body, gets no scope guard (a guard before a label is ill-formed), so variables there live until the function exits |
| **Uninitialized variables** | Observed: `int u;` is `Unavailable::Uninitialized` until first written (tested) | Only scalars/pointers/arrays of them are classed "uninitialized"; class objects are always read after construction |
| **Arrays** | Stack and member arrays observed, element writes by path (tested). `new T[n]` skipped | `new[]` with element type + count; bulk writes need call-site wrappers. Large arrays need chunking (today capped at 4096 elements, then `Unknown`) |
| **Nested objects** | Works (tested: paths `1.0.1`, `1.1[1]`) | Base-class subobjects (`is_base` fields) not yet described |
| **References** | Reference locals and parameters observed: the value is what it is bound to; writes through them land on the referent (tested). Reference *members* skipped | Rvalue references are modelled like lvalue ones |
| **Aliasing** | Natural: same `ObjectId` | Interior pointers resolve through `Place` paths (tested) |
| **Pointer arithmetic** | Result resolved at write time by address containment and static type | One-past-the-end and mid-element addresses become `Unresolved`; wild pointers honest `Unresolved` |
| **Cycles** | Works (tested, including the self-referencing sample) | Nothing special needed: ids |
| **Constructors** | Get their own frame (name `Class::Class`, parameters observed); heap objects: post-construction values reported, body assignments live | Member-initializer-list writes are initializations (not assignments) and are seen only as the post-construction value; intermediate states unobserved. A stack object is registered *after* its constructor returns, so the constructor's own writes to `this` are not attributed to the new variable |
| **Destructors** | Get their own frame (`Class::~Class`) and run while the owner's frame is live; heap objects: `ObjectDestroyed` at `operator delete` (after the destructor) | A stack object's end is the scope exit (the URR ends it). Explicit destructor events need a rewrite of destructor calls and (for heap) separating "destructed" from "freed"; URR decision (section 14) |
| **Placement new** | Skipped | Requires a URR extension: an object constructed *inside* another object's storage overlaps it, and the model has no overlapping-object concept |
| **memcpy / memmove / memset** | Not observed (writes in library code are invisible) | Call-site wrappers that re-read the destination range and emit `ValueChanged`; wrappers for known functions only |
| **Unions** | Not described (opaque) | Source level knows which member was assigned, so `Value::Union{active}` can be tracked; a debugger cannot know this |
| **Polymorphism** | Static type at `new` is the dynamic type, so layouts are right for `Base* p = new Derived` | Base/vptr subobjects; virtual bases are hard; dynamic type for objects not created by an instrumented `new` needs RTTI |
| **Templates** | Class/function templates skipped | Generate descriptors per instantiation (templated `lattice_rt_describe`); bodies need care with dependent types |
| **STL containers** | Not modelled; internals are not instrumented | Semantic adapters in the runtime using each container's *public* API (`size()`, iterators): implementation-independent. Resync after calls that may mutate. Significant work; node-based containers' internal pointers are not shown. |
| **Exceptions** | Observed: frame and scope guards are real objects, so unwinding ends them in order (tested: a throw through three frames, then a catch) | `terminate` = crash (events flushed: tested with `abort()`). `catch` parameters are not reported as variables yet |
| **goto / early return / break** | Same mechanism; a `goto` out of nested loops (tested in the kitchen-sink program) | A jump *into* a block past its guard is ill-formed C++ and cannot occur in a program that compiles |
| **Multithreading** | Single-threaded. The runtime's spinlock makes it memory-safe under threads, but nothing is thread-aware | Thread id per event (the URR already has it), per-thread buffers merged by sequence, atomic ids. The lock distorts interleavings (observer effect), and the URR's `seq` is an observation order, not happens-before |
| **Dynamic libraries** | Only the executable's own translation units are instrumented | Objects allocated by DLL code are invisible. A shared runtime DLL (single registry) and building user DLLs with Lattice are needed; hooks are per-module (`operator new` replacement is per image) |
| **Optimized builds** | `-O0` only | Section 8 |

Things the approach **cannot** promise, stated plainly: it observes writes made
by instrumented code, not by the program as a whole. Library code, inline
assembly, other modules, and compiler-generated copies are outside what the
rewrite sees unless handled by a specific wrapper or a different technique.
Arbitrary C++ runtime state is not perfectly observable by any single
technology described here.

---

## 10. Performance

**Measured** (`overhead_probe`, an `#[ignore]` test; this machine, GCC 15,
`-O0 -g`). The probe: 20,000 `new`, a 200,000-iteration loop writing a field, and
20,000 `delete`. It is a worst case: every loop counter increment is an event.

| | Stage 1 (heap only) | Stage 2 (+ locals, frames, `++`) |
| --- | --- | --- |
| Plain run | 27-33 ms | 23-28 ms |
| Events | 300,003 | **940,015** |
| Observed run, release build of Lattice | 1.33 s | **3.66 s** (about 257k events/s) |
| Observed run, debug build of Lattice | 1.75 s (file transport) | 11.2 s (pipe; the receiver is the bottleneck) |
| Compile time (includes the runtime) | ~2.4 s | ~3.3 s |

So observing this loop costs roughly **130x** in a release build. That is the
price of write-level fidelity in a tight loop and is the reason the mechanisms
below matter; typical teaching programs emit orders of magnitude fewer events.

**Receiver speed** (`ingest_throughput_probe`, same 300k-event stream, 80 MB of
JSON): parse and apply at about **367k events/s in a release build** (parse 0.44 s,
apply 0.10 s, total 0.82 s) versus **63k/s in an unoptimized build** (parse alone
3.6 s). With a live pipe the program is throttled by this number (section 6.D), so
`serde`/`serde_json` are built optimized even in the dev profile (a modest gain:
the model crate itself is still unoptimized there).

**Volume will dominate** for loop-heavy programs: one event per instrumented
write. Mechanisms, in rough order of value (batching is done):

1. ~~**Batching**~~ **Done (stage 2):** 64 KB buffer, 20 ms flusher thread,
   `atexit`/`abort`/exception flush (section 6.D).
2. **Not emitting what nobody sees**: untracked writes already emit nothing;
   coalescing repeated writes to one place between "observable points"; not
   reporting loop counters of loops the user did not ask about.
3. **Event budgets and sampling**: cap events per run; for long loops, keep the
   first and last N iterations (or take state snapshots instead of per-write
   events).
4. **Denser encoding** (MessagePack/bincode) and timestamp deltas.
5. **Snapshot checkpoints** in the URR (exist; section 13).
6. **Lighter per-call work**: avoid taking the lock for untracked addresses
   (a read-mostly map), move JSON formatting to a background writer.

Memory: the URR keeps every event and never evicts destroyed objects; very long
runs need a retention policy (already listed in `docs/runtime-model.md`).
Large arrays are capped per value today (4096 elements).

---

## 11. Distribution: bundle, discover, install

Not solved in this spike; this is the classification the design implies.

| Component | Bundle with Lattice | Discover on the machine | Realistically installed separately |
| --- | --- | --- | --- |
| **lattice-runtime** (C++ source) | **Yes**: embedded in the Lattice binary and written into each session workspace (this is how the prototype ships it); compiled with the user's compiler | n/a | no |
| **libclang.dll** (analysis) | **Yes, recommended** for a pinned, tested version (official LLVM releases ship it; Apache-2.0 with LLVM exceptions) **[knowledge]** | `LATTICE_LIBCLANG`, `PATH`, `Program Files\LLVM\bin` (implemented). **[verified]** one exists here under Qt/Houdini; such copies are accidental and version-uncontrolled. Needs >= 17 for the operator-kind API | no |
| **C++ compiler** | Possible (a portable MinGW-w64 or llvm-mingw distribution, hundreds of MB) but not decided | MinGW/MSYS2, LLVM, scoop (existing discovery) | **MSVC cannot be redistributed**: Build Tools must be installed by the user |
| Clang compiler binary | Not needed by the recommended design | | |
| Custom Clang with an IR pass | **Only** bundle-able (you build it); needed only if the IR fallback is adopted | no | no |
| Debugger (LLDB/GDB) | Optional future; LLVM ships `lldb` **[knowledge]** | MinGW GDB (**[verified]** present) | |
| Windows SDK / CRT | n/a | needed only for an MSVC target | yes (MSVC route) |

Behaviour today when tools are missing: no libclang -> the observing
instrumenter returns an explicit error (session `Failed`, "instrumentation
failed: ..."); source that does not compile -> left untouched so the *compiler*
reports the user's error (tested).

---

## 12. The prototype

### What it is

```
src-tauri/src/observe/
  libclang.rs            dynamic libclang binding (no link-time LLVM); ~30 entry points
  discovery.rs           find libclang; ask the real compiler for target + include dirs
  analysis.rs            walk main-file AST: records, new/delete, writes, functions,
                         blocks, local declarations, for / range-for loops
  rewrite.rs             analysis -> insertion-only instrumented source
  runtime_support/
    lattice_runtime.h    generated-code surface, type registry, Frame/Scope/local/param
    lattice_runtime.cpp  identity, registry, call-stack model, JSON events, batching,
                         flusher thread, crash flush, allocation functions
  instrumenter.rs        ObservingInstrumenter: implements the EXISTING Instrumenter seam
  pipe.rs                named-pipe server (live transport)
  receiver.rs            incremental event decoder (Ingest) -> URR Timeline
  report.rs              plain-text rendering of events and state (call stack + heap)

src-tauri/tests/
  common/mod.rs          shared helpers (manager with instrumenter, snapshots, histories)
  observation.rs         heap objects, identity, transport, robustness
  observation_frames.rs  functions, scopes, variables
```

Integration with existing code is three hooks, none changed in behaviour:
`Instrumenter::prepare` (already in the execution manager; `PassThrough` stays
the default), `BuildPlan.extra_compile_args`/`run_env` (already existed), and
`RuntimeEventStream::close` (already called after the run). The execution
manager, compiler driver, process layer, UI, clangd integration and every
existing test are untouched. Observation is opt-in
(`ExecutionManager::with_instrumenter`); the app does not turn it on yet.

**How frames, scopes and variables are instrumented** (stage 2). Every rule is
insertion-only, so the program is otherwise byte-for-byte the user's:

| Construct | Inserted |
| --- | --- |
| function body `{` | `Frame __lattice_frame(SITE, "name");` then one `param(...)` / `param_ref(...)` per named parameter |
| a `{` block that declares variables | `Scope __lattice_scope(SITE);` |
| after a declaration statement | `local(x, "x", SITE);` (`local_uninit` for an unset scalar, `local_ref` for a reference) |
| `for (int i = 0; cond; inc) body` | the whole loop goes in `{ Scope ...; for (...) }`, and `cond` becomes `(local(i, "i", SITE), cond)` so the variable is reported once, on the first test |
| `for (T e : range) { body }` | the body gets a scope and `local(e, ...)` at its top, so each iteration has a fresh variable |
| `x++` (postfix) | `post_written(x++, &(x), SITE)`: both arguments are evaluated before the call, so the new value is observed while the old one is still the result |
| `x += y`, `++x`, ... | `written(&(x += y), SITE)`, same as `=` |

Why guards rather than explicit exit calls: a guard is an object, so the exit
runs on `return`, `break`, `goto`, and exceptions. The model ends a scope's
variables and their automatic objects itself when the scope or frame exits, so
the runtime never has to report a stack object's death separately.

Rules that keep it from breaking programs (each is tested or checked by the
kitchen-sink program): nothing is hooked inside a function that cannot be given a
frame; `switch` bodies and blocks with labels get no scope guard; only the
complete `for` form is handled; a postfix operand with side effects is left
alone; writes to `volatile` and to bit-fields are not wrapped; `constexpr`
functions and templates are skipped.

### What it observes

For your Node sample, unmodified:

```
#0   FunctionEntered   main.cpp:6:1   main (FrameId#1)
#1   TypeDeclared      -              int (type 2)
#2   TypeDeclared      -              Node* (type 3)
#3   TypeDeclared      -              Node (type 1)
#4   ObjectAllocated   main.cpp:7:15  Object#1 : Node (Heap, Allocated, 0x<addr>)
#5   ObjectConstructed main.cpp:7:15  Object#1
#6   ValueChanged      main.cpp:7:15  Object#1.value = 10
#7   ValueChanged      main.cpp:7:15  Object#1.next = nullptr
#8   ObjectAllocated   main.cpp:7:5   Object#2 : Node* (Automatic, Alive, 0x<addr>)
#9   VariableCreated   main.cpp:7:5   a (Local) -> Object#2
#10  ObjectAllocated   main.cpp:8:15  Object#3 : Node (Heap, Allocated, 0x<addr>)
  ...
#16  ValueChanged      main.cpp:10:5  Object#1.next = → Object#3
#17  ValueChanged      main.cpp:11:5  Object#3.next = → Object#1
#18  ObjectDestroyed   main.cpp:13:5  Object#3 (Freed)
#19  ObjectDestroyed   main.cpp:14:5  Object#1 (Freed)
#20  FunctionExited    -              FrameId#1
```
(The local pointer variables `a` and `b` are objects of their own, `Object#2` and
`Object#4`, which is why the heap nodes are `Object#1` and `Object#3`: the heap
and the stack share one id space.) The URR state after #17:

```
Runtime Snapshot after event #17 (main.cpp:11):
  Call stack (innermost first):
    main()  at main.cpp:11
        Node* a → Object#1
        Node* b → Object#3
  Heap:
    Object #1: Node [Heap, alive]
        value = 10
        next → Object#3
    Object #3: Node [Heap, alive]
        value = 20
        next → Object#1
```
and after the deletes both are `destroyed`, with the two links reported
`(dangling)`.

A program with calls, a loop and a block (`tests/programs/frames.cpp`), caught
inside `square(3)`'s first call:

```
Runtime Snapshot after event #22 (main.cpp:7):
  Call stack (innermost first):
    square()  at main.cpp:7
        int n = 1
        int result = 1
    main()  at main.cpp:19
        Node* first → Object#1
        Node second = {2, nullptr}
        int total = 0
        int i = 1 (block)
  Heap:
    Object #1: Node [Heap, alive]
        value = 1
        next = nullptr
```
and later, after `link(first, &second)` and `delete first`, where the heap node
points *into the stack* and `first` is dangling:

```
Runtime Snapshot after event #69 (main.cpp:28):
  Call stack (innermost first):
    main()  at main.cpp:28
        Node* first → Object#1 (dangling)
        Node second = {2, nullptr}
        int total = 14
  Heap:
    Object #1: Node [Heap, destroyed at #69]
        value = 1
        next → second#3
```

The rewrite of the sample (insertions only; the descriptor after the struct is
one line in reality):

```cpp
Node* a = ::lattice::rt::constructed(new (::lattice::rt::tag<Node>(::lattice::rt::Site{"main.cpp",7,15,"main"})) Node{10, nullptr});
::lattice::rt::written(&(a->next = b), ::lattice::rt::Site{"main.cpp",10,5,"main"});
delete ::lattice::rt::del_hint( b, ::lattice::rt::Site{"main.cpp",13,5,"main"});
```

### What it cannot observe (today)

`new[]`; placement new; STL containers (internals and contents: an `std::vector`
local shows up as a variable with an unknown value); types defined in user headers
(treated as opaque); private fields, bit-fields, base-class subobjects, unions;
class-type assignment (`operator=`); writes performed by library code (`memcpy`,
`std::fill`); member-initializer-list initialization as individual events;
destructors as separate events; `delete[]`; `static` and `thread_local` locals;
lambdas' own frames and locals; `catch` parameters; variables declared in
`if`/`while`/`switch` conditions; `for` loops with a missing part; threads;
macros and templates; programs libclang cannot parse in the user's own code
(those are left uninstrumented). Section 9 covers the path for each.

### The visualization (stage 4)

**What the user sees.** In the Visualization panel (beside the editor, and as the
Visualizer view): the **call stack** as a column of boxes on the left (outermost
frame first; the executing one outlined), each listing its variables; **heap
objects** as boxes to its right; **pointers as arrows** from the slot that holds
them to what they point at, including into the middle of an object and from the
heap back into a stack variable. Freed objects stay visible, faded and dashed,
while a pointer still points at them, and that arrow is dashed red (**a dangling
pointer**). What changed at this step is emphasized (a lit row, a bright arrow, an
outlined box), and the line of source that caused it is highlighted in the editor.
Controls: slider, start / previous / play-pause / next / end, and the keyboard
(left and right arrows, Home, End, Space). The Runtime tab and the visualization
share one position, so they never disagree. The view **opens just before `main`
returns** (`ObservationSummary.final_step`), because the very last state, with
every frame popped, is an empty picture.

**Three layers, each ignorant of the others' concerns:**

| Layer | Where | Knows | Does not know |
| --- | --- | --- | --- |
| View model | `src-tauri/src/viz` | frames, variables, objects as typed slot trees, pointer targets (null / object-or-part / untracked / dangling), what changed, the source line | instrumentation, processes, the app shell, **and any layout vocabulary** (a test fails if it appears) |
| Layout | `src/visualization/layout.ts` | the view model; fixed row heights and the width of a monospace character | what the program *is* |
| Renderer | `src/visualization/VisualizationCanvas.tsx` | the layout | how any of it was computed |

The view model is **generic by construction**: there is no list, tree or graph in
it. A frame has variables; an object is a tree of typed slots; a pointer slot has a
target. Every slot has an *anchor* (`12`, `12.1`, `12.1[3]`: the object id, then
the path inside it), and a pointer's target uses the same, so an arrow is just
"from this anchor to that anchor". It also protects the picture: automatic
storage that no variable names yet (a local's storage is announced one event
before its name is bound) is never drawn as an object; a freed object is shown
only while something visible points at it; at most 300 objects and 64 elements per
array are listed, and the view says how many it left out.

**The layout is one generic algorithm.** Roots are the stack's pointers. Heap
objects go in columns by distance from them (breadth-first, so cycles and sharing
cannot loop); an object is placed level with the pointer that first reached it (so
a chain of nodes runs straight) and pushed down to avoid overlap; objects nothing
reaches go in a last column. A forward pointer is an S-curve; a pointer that goes
back to something at or behind it leaves its box on the right and travels
*around* it (never across its own text); a pointer into its own box is a small
arc. Nothing recognizes data structures: a linked list comes out as a row, a tree
as a fan, a cycle as a loop, because that is what the pointers say.

**How it was verified.** The view model by `visualization.rs` (below); the layout by
19 `vitest` tests; the whole by **driving the real application and looking at
screenshots** (a throwaway DevTools-enabled build, as in stage 3). That caught
bugs no assertion did. Fixed, each with a regression test where one is possible:
- arrows that loop out to the right were **clipped** by the SVG's edge (the
  drawing area is now sized from the arrows as well as the boxes: `viewBox`);
- a back-pointer was drawn **through its own box's text** (it now routes around);
- for one step a local's storage looked like an anonymous heap object (a phantom
  `Node*` box);
- with recursion the executing frame was **off screen** (the changed box, else the
  current frame, is scrolled into view each step);
- the view opened on the empty end state.

Programs looked at: a list built in a loop, closed into a cycle, with a node then
freed (the cycle, the dangling arrow, the freed box, stepping, play and pause, the
editor line); and recursion four frames deep with an array, a struct, and pointers
into both. Also checked in the real app: the Runtime tab follows and drives the
same position; the Visualizer view shows the same recording; the keyboard works.

**Known limitations of the picture** (not bugs in the data): arrays and structs
are listed vertically, never as a row of cells; arrows can cross each other and
other boxes (no routing around third parties, no crossing minimization); box
positions are recomputed per step, so adding an object can shift its neighbours
(boxes glide, arrows jump); one run's recording is kept; there is no zoom (the
panel scrolls); a union is shown as text.

### In the application (stage 3)

**What the user sees.** An **Observe** toggle in the toolbar (remembered between
sessions). With it on, Run records the run; the program's own output is unchanged
and the Output tab adds one line, "Observed: N events recorded. See the Runtime
tab." The **Runtime** tab then shows a slider, Start/previous/next/End buttons,
the event that led to the current state, and the state as text (call stack with
each frame's variables, then the heap). It is the textual URR, not a
visualization. If libclang is missing the toggle is disabled and its tooltip says
what to install or set; the status is re-checked when the window regains focus,
so installing libclang does not need a restart.

**How it is wired** (each piece is small and replaceable):

| Piece | Where | Role |
| --- | --- | --- |
| `ObserverProvider` trait | `runtime/instrumentation.rs` | The seam's way to ask "give me an instrumenter for *this* build"; `runtime` still knows nothing about `observe` |
| `RunRequest.observe` | `runtime/manager.rs` | Per-run opt-in. Asking for observation and not getting it **fails the run with the reason**; it never quietly runs unobserved |
| `ObservationService` | `observe/service.rs` | Availability (`status`), the provider implementation, result storage, `summarize`, and `view_at` (a step rendered as text). No Tauri types, so all of it is tested directly |
| `ObservationSummary` | `runtime/session.rs` | Numbers the UI needs (events, truncated, skipped, issues), added to `RuntimeSession` |
| Commands | `app/commands.rs` | `run_program` takes `observe`; new `observation_status`, `observation_step(session, step)`. Only the **latest** observed run's recording is kept (a recording holds every event and object in memory) |
| UI | `src/ui/ObservationView.tsx`, `Toolbar.tsx`, `OutputPanel.tsx` | The backend renders each step as text; the UI only asks for step N |

**The event budget.** The model costs about **1.1 KB of peak memory per event**
(measured: 1.07 GB for the 940,000-event loop of `overhead_probe`, release build),
so recording cannot be unbounded. Each observed run has a budget (default
**250,000 events**, overridable with `LATTICE_EVENT_LIMIT`; 0 = unlimited).
When the runtime has sent that many events it sends one last event,
`ObservationTruncated`, and stops observing (every hook returns early, so the
program also stops paying for observation, and it runs on unchanged). The URR
gained that event (the one change to its schema in this stage): the state
exposes `truncated_at()`, and the UI shows "Recording stopped after N events. The
program kept running, so the later part of the run, including its end, is not
shown", and again on the last step. A partial recording is never presented as a
whole run. Not yet attributed: where the 1.1 KB goes (an event is 208 bytes
inline; checkpoint copies and vector growth are the suspects).

**How it was verified.** The Rust side by `observation_app.rs` (below). The UI by
driving the **real application**: a throwaway build (`TAURI_CONFIG` override
adding `--remote-debugging-port` to the WebView2 arguments, built into a separate
target directory so no tracked file changed) was launched and controlled over the
DevTools protocol with a small Node script. Observed in the real window:
- Toggle on, Run: output `10 20 30 40 50` unchanged, "Observed: 20 events
  recorded", the Runtime tab at step 11 reads `int[5] arr = [10, 20, 30, 40, 50]`
  and `int i = 2 (block)`; Start, End and the slider move through it.
- With `LATTICE_EVENT_LIMIT=8`: "Observed: 9 events recorded (stopped at the
  limit)", the truncation banner on every step, the marker as the last event.
- With no libclang found: the toggle is disabled and its tooltip carries the hint.
- Six alternating toggle/Run cycles in one session (some with an immediate click
  after the toggle): every run matched the toggle state, no mismatches.
One early run did show an observed run right after the toggle had read "off"; it
did not reproduce in the cycles above, the preference-off launch, or a repeat of
the exact sequence, and I could not explain it. The driver is not committed.

What changed in how it *fails*, worth knowing: libclang errors located inside a
**system header** are now set aside instead of disabling observation. Before,
`#include <windows.h>` (which pulls in GCC's intrinsics headers that libclang
cannot parse) silently turned observation off. The analysis also runs with no
error limit (`-ferror-limit=0`): at the default limit of 20, such errors stop the
parse early and truncate the AST of the user's own code.

### Tests

**`observation.rs`** (heap objects and transport; all run the real pipeline,
over the named pipe unless stated):

| Test | What it proves |
| --- | --- |
| `node_sample_reaches_the_urr` | The spec'd program: event order, ids, types, field values, both links, cycle, lifetimes, locations (line/column/function), snapshot before/after, dangling after free |
| `instrumented_program_behaves_exactly_like_the_original` | Same stdout/stderr/exit code/state with and without instrumentation, with `#include <cstdio>` |
| `every_allocation_gets_a_fresh_logical_id` | 300 objects, 48 addresses (44 reused), 300 ids, disjoint lifetimes |
| `a_crash_does_not_lose_the_events_before_it` | `abort()`: events before the crash are all there |
| `programs_that_do_not_compile_are_left_to_the_compiler` | Compile errors reported by the compiler, on the user's line, nothing instrumented |
| `diagnostics_of_instrumented_builds_still_point_at_the_users_lines` | A compiler-only error in an instrumented build lands on the right line |
| `programs_using_the_standard_library_keep_working` | `<iostream> <string> <vector>`: libclang 18 + MinGW libstdc++ 15, identical behaviour |
| `nested_objects_arrays_and_interior_pointers` | Paths like `1.0.1`, `1.1[1]`; `&o->in` resolves to the member |
| `rewriting_only_inserts_text` | Golden: every original character survives in order; line count unchanged |
| `runtime_events_are_plain_urr_json` | What the C++ runtime wrote round-trips through the URR's serde types unchanged |
| `the_file_transport_gives_the_same_result_as_the_pipe` | The fallback transport yields the same events |
| `headers_libclang_cannot_digest_do_not_stop_instrumentation` | `#include <windows.h>` still instruments (regression for the system-header fix) |
| `live::events_arrive_while_the_program_is_still_running` | Events seen by Lattice during the program's 2.5 s sleep |
| `live::a_hung_program_keeps_its_events_after_a_timeout` | Infinite loop killed by the timeout: nothing lost to buffering |
| `live::a_program_that_never_emits_does_not_hang_lattice` | The lazily-connecting pipe does not block the run's end |
| `live::a_failed_build_does_not_leak_the_pipe_reader` | The pipe exists from `prepare`; a failed build releases it |
| `overhead_probe`, `ingest_throughput_probe` (`#[ignore]`) | Section 10 numbers |

**`observation_frames.rs`** (functions, scopes and variables):

| Test | What it proves |
| --- | --- |
| `functions_locals_and_scopes_reach_the_urr` | main -> square x3, main -> link; parameters vs locals; per-call objects; a loop counter counting 1..4 then gone; a block's variable with one contiguous lifetime and `ScopeExit`; heap and stack pointing at each other; empty at the end |
| `recursion_builds_one_frame_per_call` | `fact(4)`: five distinct frames alive at once, `n` = 4,3,2,1, then `rest` = 1,2,6 as calls return |
| `references_are_bindings_to_the_thing_they_name` | `int& r = a;` and a reference parameter: one `a` changed through both; references are `Reference` links |
| `uninitialized_variables_arrays_and_range_for` | `int u;` is `Uninitialized`, not 0; one array element changes; range-for gives a fresh variable per iteration |
| `early_returns_and_exceptions_unwind_frames_in_order` | A throw through three frames; every frame entered is left |
| `methods_constructors_and_destructors_get_frames` | `Counter::Counter`, `bump`, `~Counter`; the object's field changes in place |
| `compound_assignment_and_increments_are_observed` | `+=`, `*=`, `++x`, `x--`, `y = x++`, `<<=` |
| `functions_that_cannot_be_framed_are_left_alone` | A function-try-block: nothing inside attaches to the caller |
| `loops_with_missing_parts_behave_identically` | Loop forms that are not instrumented still compile and behave the same |
| `a_kitchen_sink_of_constructs_behaves_identically` | Nested loops, while/do-while, switch with a braced case, `goto` out of loops, lambdas, statics, globals, bit-fields, `volatile`, `constexpr`, templates, STL locals, methods: same stdout/exit code as the plain build; every frame returned; model consistent |

**`observation_app.rs`** (what the commands rely on):

| Test | What it proves |
| --- | --- |
| `status_explains_what_is_missing_and_what_to_do` | No libclang, or a path that is not one: `available: false` with a hint naming `LATTICE_LIBCLANG` |
| `status_reports_the_libclang_that_will_be_used` | Version, path, event limit; camelCase for the UI |
| `asking_for_observation_that_is_not_possible_fails_clearly` | No observer / an observer that refuses: the run fails with the reason, nothing is compiled or run; the same manager still runs plain programs |
| `observed_and_plain_runs_share_one_manager` | `observe: false` leaves no recording and the program's output is identical |
| `the_step_view_shows_the_program_at_any_point` | Step 0, a mid-run step with `square()` on top of `main()`, the last step, clamping past the end |
| `a_run_over_its_event_budget_is_truncated_and_says_so` | Limit 150: 150 events + the marker while the loop ran 1000 times; same output; summary and views say so; the frozen state is consistent |
| `a_limit_of_zero_means_no_limit` | |

**`visualization.rs`** (the view model; hand-built model state, plus one real run):

| Test | What it proves |
| --- | --- |
| `a_linked_structure_is_just_slots_and_pointers` | head -> A -> B -> C -> A: a frame variable, three records of `value`/`next`, the cycle closing; no "list" anywhere; a variable's own storage is not listed as a heap object |
| `dangling_targets_stay_visible_only_while_something_points_at_them` | Freed B stays (A.next points at it, `dangling: true`); freed, unreferenced C is gone |
| `interior_pointers_name_the_part_they_point_at` | Target object 10, path `.1` |
| `null_and_untracked_pointers_are_distinct_from_object_pointers` | |
| `arrays_are_capped_and_say_how_much_was_left_out` | 100 elements: 64 listed, 36 counted |
| `values_are_formatted_and_absence_is_explained` | `1.0`, `2.5`, a character, `true`; uninitialized / optimized away / invalid |
| `the_view_says_what_changed_at_this_step` | Per event kind; a field write names the field (`10.1`); the line for the editor |
| `too_many_objects_are_counted_not_dropped_silently` | 300 shown, 5 counted |
| `nested_block_variables_and_globals_are_reported` | |
| `the_end_of_a_truncated_recording_is_flagged` | |
| `storage_announced_before_its_variable_is_not_drawn_as_an_object` | The phantom-box bug |
| `the_view_serializes_for_the_ui_in_a_stable_shape` | The camelCase shape the TypeScript mirrors |
| `the_view_model_stays_independent_of_everything_but_the_model` | No `observe`/`runtime`/`tauri` references, and no width/height/colour/position vocabulary |
| `a_real_run_reaches_the_view_model` | `frames.cpp`: a step inside `square`, and a heap node pointing at a stack variable, expressed as nothing but a target |

**`src/visualization/layout.test.ts`** (`npm test`, 19 tests): a chain runs left to
right, level with its pointers; cycles and sharing terminate and place every object
once; children of one node share a column; unreachable objects go last; a heap
pointer into a stack variable is a backward edge onto the frame; arrows land on
the part of an object pointed at (a field, an element, a nested member); dangling
and freed are flagged; undrawable pointers are counted; null and untracked draw no
arrow; array elements and nested members are addressable rows; changed rows and
edges; the innermost frame is current; determinism; box widths; an empty program;
back-edges avoid their own box (the curve is sampled); self-pointers arc beside
the box; and **every arrow lies inside the drawing area**.

Plus `truncation_marks_where_the_record_ends` in `runtime_model.rs` (the URR
side, including the wire form), and unit tests: `rewrite.rs` (nesting, line preservation, token preservation,
guard ordering, postfix, `for`) and the incremental decoder in `receiver.rs`
(split chunks, truncated tail, continue-after-error, progress visible across
threads). **The whole suite: 29 unit + 12 execution + 16 + 7 + 10 observation
(+2 ignored probes) + 37 runtime model (+1 ignored probe) + 14 visualization =
125 Rust tests, plus 19 frontend tests (144), all passing.** The
observation tests need a libclang (see below) and skip loudly without one.

Additional portability checks, run by hand with the hand-written equivalent of
the instrumented sample: **[verified]** builds and produces identical event
sequences with GCC 15, Clang 18 (no extra flags after replacing `std::mutex`
with a spinlock) and MSVC 2022.

### Running it

```
# libclang >= 17 (official LLVM releases ship bin/libclang.dll)
set LATTICE_LIBCLANG=C:\path\to\libclang.dll
cargo test --test observation -- --nocapture
cargo test --test observation overhead_probe -- --ignored --nocapture
```
On this machine a git-ignored `src-tauri/.cargo/config.toml` points at the copy
found under Houdini/Qt, so plain `cargo test` runs them. `LATTICE_REQUIRE_LIBCLANG=1`
turns a skip into a failure.

---

## 13. Changes to existing code

Smallest justified set. None changes behaviour when observation is off.

| Change | Why |
| --- | --- |
| `model/timeline.rs`: **adaptive checkpointing** (a checkpoint after at least `max(interval, object count)` events instead of every `interval`); new `checkpoint_count()` accessor | Found by the spike: the fixed-interval policy made total cost O(events x state). A 300k-event run with 20k live objects took **126 s** to ingest, **9.9 s** after. Public API otherwise unchanged; covered by a new test |
| `Cargo.toml`: `windows-sys` features `Win32_System_LibraryLoader`, `Win32_System_Pipes`, `Win32_Storage_FileSystem`, `Win32_System_IO` | To load libclang dynamically and to serve the named pipe |
| `Cargo.toml`: `serde` and `serde_json` built with `opt-level = 3` in the dev profile | Event ingest is 8x slower unoptimized (section 10) |
| `lib.rs`: `pub mod observe`; the app state gets an `ObservationService`, registered as the manager's observer; two new commands | New module and wiring (stage 3) |
| `model`: new event `ObservationTruncated`, `RuntimeState::truncated_at()` | The one schema change in stage 3: a recording cut off by its budget must say so |
| `runtime/manager.rs`: `RunRequest.observe`, `with_observer`; `runtime/instrumentation.rs`: `ObserverProvider`; `runtime/session.rs`: `RuntimeSession.observation` | Per-run opt-in without `runtime` depending on `observe`. Existing behaviour is unchanged when `observe` is false (every existing `RunRequest` literal gained `observe: false`) |
| Frontend: `runtime/index.ts`, `useExecution.ts`, `App.tsx`, `Toolbar.tsx`, `OutputPanel.tsx`, `ObservationView.tsx`, `layout.css` | The toggle and the Runtime tab; no existing screen was rearranged |
| Stage 4: new `viz` module; `observation_graph` command; `ObservationSummary.final_step`; `observe::graph_at`. Frontend: `VisualizationCanvas.tsx` (was a placeholder), `visualization/layout.ts`, `runtime/useRecording.ts` (one shared position), `CodeEditor.tsx` (current-line highlight), `EditorView.tsx`, `ObservationView.tsx` and `OutputPanel.tsx` (take the shared position) | The visualization. No URR change; no change to how programs are built or run |
| `package.json`: `vitest` (dev dependency) and `npm test` | First frontend tests (the layout is algorithmic). `npm audit` reports 2 low findings in `monaco-editor`'s `dompurify`; they predate this and are unchanged |
| `runtime/instrumentation.rs`: doc comment only | Points at the model |
| `.gitignore`: `src-tauri/.cargo/` | Local cargo env file |

**No change to the URR's types, events or snapshot API was needed** in either
stage; the existing model (frames, scopes, variables, automatic storage,
implicit end-of-scope) was sufficient, and in every tested program the model
accepted the observer's whole event stream (its strict validation rejected
nothing). `RuntimeEventStream` still has only `close()`; the pipe
transport feeds the model continuously and publishes the result in `close()`.

---

## 14. Decisions to review, limitations, next milestones

### Open decisions

1. **Event volume policy: partly decided.** A fixed budget (default 250,000) with
   a truncation marker exists. Still open: whether the user should be able to
   change it (it is an environment variable today), a smarter policy than "stop"
   (keep the first and last N iterations, coalesce repeated writes), and a
   retention policy for the model (events and destroyed objects are never evicted;
   ~1.1 KB/event peak, not yet attributed to its causes).
2. **Observed stack objects are registered after their constructor returns**, so
   the constructor's writes to the new object are not attributed to the variable
   (heap objects do not have this gap). Fixing it means registering before
   construction, which needs a different rewrite of declarations; worth it only
   if constructor bodies turn out to matter to users.
3. **`ObjectDestroyed` semantics:** today one event at `operator delete`. With
   destructor instrumentation, should the URR separate "destructor ran" from
   "storage released"? Affects `LifeState`.
4. **Frontend hardening:** libclang with bundled version vs discovered; whether
   to move to libTooling for macro-aware rewriting. Concretely, libclang 18 cannot
   parse parts of GCC 15's own headers (system-header errors are now tolerated,
   but the AST of those headers is incomplete).
5. **URR extension needs ahead:** overlapping objects (placement new), function
   pointer targets, bulk-write events, tombstones for freed ranges, a
   "truncated" marker (decision 1).
6. **Where observation lives in the UI.** Stage 3 added a toolbar toggle and put
   the step view in the existing Runtime tab (no layout was changed). Whether
   that is the right home, versus the Visualization view once it exists, is open.
   Observation status is re-checked on window focus; there is no in-app way to
   point at a libclang (only `LATTICE_LIBCLANG` and discovery).
7. **Instrumenter trait** takes only the workspace; the prototype builds the
   instrumenter with the compiler path up front. Passing the chosen compiler to
   `prepare` is the cleaner long-term contract.

### Limitations (beyond section 12)

- Analysis uses libclang 18 against the real compiler's headers. A newer
  standard library than libclang understands yields analysis errors, in which
  case nothing is instrumented (safe, but silent to the user unless surfaced).
- The runtime serializes JSON by hand; the wire shape is verified against the
  Rust types by round-trip tests, but a schema change in `model` must be
  mirrored in `lattice_runtime.cpp` (the tests will catch a mismatch).
- `float` values use `%.17g`; NaN/infinity become `Unavailable::Invalid`.
- Single-threaded: the runtime keeps one call stack for the whole process.
  One event per instrumented write.
- Observed programs no longer get "unused variable" warnings for the variables the
  instrumentation mentions (it reads them); diagnostics stay on the right lines.
- `_Exit`/`TerminateProcess` within ~20 ms of the last event loses the tail.
- Generated `offsetof` use needs public members and a complete type; other
  records fall back to opaque.

### Next milestones (in order)

Done: ~~named pipe receiver with batching and crash-safe flush~~; ~~stack
variables, parameters, frames and scopes~~; ~~compound assignment and
`++`/`--`~~; ~~stack arrays~~; ~~wiring observation into the app (toggle,
availability message, event budget, Runtime tab)~~; ~~the visualization (view
model, layout, renderer, playback, editor line highlight)~~.

1. **Polish the picture** (known limits in section 12): arrays as a row of cells;
   arrow routing that avoids third-party boxes and reduces crossings; stable
   positions across steps (keep an object where it was when the structure grows);
   zoom and pan; hover to trace an arrow.
2. **Reduce the cost per event** (memory first: attribute the ~1.1 KB, then
   compact events / checkpoints), since the budget is a blunt tool.
3. **`new[]`, `static` locals, lambdas' frames, `catch` parameters, `if`/`while`
   condition variables, loops with a missing part**: the remaining gaps in "the
   program around the heap".
4. **Destructors and base classes** (descriptors with `is_base`; explicit
   destructor events).
5. **Container adapters** for `std::vector`, `std::string`, `std::unique_ptr`,
   then maps/lists, implemented in the runtime against public APIs.
6. **Templates** (per-instantiation descriptors) and user headers.
7. **Robustness pass** on the rewriter (macros, evaluation-order corner cases)
   and the decision whether to move to libTooling.
8. **Threads** (per-thread stacks, atomic ids, per-thread buffers), then
   **optimized builds** and **pre-built binaries** as a separate observation
   source (debugger/DBI feeding the same events).

### Environment note

This machine's toolchain, for reproducibility: GCC 15.2.0 MinGW-w64 (the
execution engine's compiler), GDB 16.3, MSVC 2022 (`cl`, not on `PATH`), a Clang
18.0.3 (Android NDK, default target `x86_64-w64-windows-gnu`), libclang 18.1.7
(Qt build, shipped with Houdini), JetBrains' LLDB 9.0.0. No clang, clangd, clang-cl
or LLVM is on `PATH`.
