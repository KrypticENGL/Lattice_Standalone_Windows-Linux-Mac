# Runtime observation coverage

What the observation pipeline sees today, where it stops seeing, and what the generic
fix is. Evidence is the real pipeline (libclang → instrumented source → compiler →
running program → URR); the tests that pin each claim are in
`src-tauri/tests/observation_coverage.rs` (and `observation_multifile.rs` for headers).

Toolchain these findings were measured on: clang 23 targeting `x86_64-pc-windows-msvc`
(so the **MSVC STL**, not libstdc++).

Pipeline, for reference:

```
C++ source → libclang analysis → source rewrite → compile (+ runtime) → run
  → JSON events on a pipe → URR (RuntimeState/Timeline) → snapshot → viz::build → UI
```

## 1. Why `int* arr = new int[10]` showed `arr: untracked`

Measured before any change: the timeline contained `arr` (an `int*` variable), the loop
counter and its writes — and **no allocation, no array, no element writes**. The pointer's
value was `Target::Unresolved`, which the canvas prints as `untracked`.

**First point the information disappears: source analysis (`observe/analysis.rs`).**
`new_expr` returned early for any `new T[n]` ("arrays are a later milestone"), and
`delete_expr` returned early for `delete[]`. So the rewriter never tagged the allocation,
the runtime never registered the block, and everything downstream was correct but had
nothing to work with:

| Stage | Behaviour for `new int[10]` |
|---|---|
| analysis | skipped (the cause) |
| runtime | plain `operator new[]` → untracked `malloc` |
| pointer value | `locate()` finds no block → `unresolved` |
| `arr[i] = v` | `after_write` → `locate()` fails → silently dropped |
| URR / viz | correct, but empty |

The URR itself needed **no extension**: `TypeKind::Array { element, len }`,
`Value::Array`, `Step::Index` and "pointer to element = `Place{object,[Index(i)]}`" already
existed, and the runtime's address resolver already walked arrays.

### Stack array vs dynamic array

They differ only in how they are *discovered*, not in how they are represented:

* `int arr[10];` — **already fully observed**: one automatic `int[10]` object; elements
  `<uninitialized>` until written; `arr[i] = v` is `ValueChanged` at `(object, [Index(i)])`
  with the source line.
* `new int[10]` — now the same shape with `storage = heap`. Three distinct things:

| Thing | URR |
|---|---|
| pointer variable `arr` | automatic object, type `int*` |
| allocation | heap object, type `int[10]` (element `int`, length 10) |
| what `arr` holds | `Pointer → Place{array,[Index(0)]}` (an interior place, like C++) |

`int* p = arr + 2; *p = 9;` resolves to `[Index(2)]`; `a[3]` writes through any alias land
on the array object. The pointer and the array are never the same object.

### What was implemented (smallest generic extension)

* `NewTag` gained `elem_size`; the rewrite emits `new (tag_array<T>(SITE)) T[n]`.
* The length is only known to the allocator (`size / sizeof(T)`), so the runtime
  registers the array type `T[n]` at allocation time (`array_type_of`, one type per
  (element, length), shared with declared `T[n]`).
* `new T[n]` with no initializer of a scalar reports its elements as `<uninitialized>`
  (not garbage); `()`/`{}` are value-initialized and read.
* `delete[]` carries its source line like `delete`.
* Multi-dimensional (`new int[2][2]`) and arrays of records work through the same path.

### Deliberate limits (left untracked, never misreported)

* Types with a non-trivial destructor: the compiler prepends an array cookie and returns
  an interior pointer, so the block is not the array.
* More than 4096 elements: one event cannot carry them (the same limit stack arrays have;
  chunking is the missing piece).
* Placement/nothrow `new`, classes with their own `operator new[]`.
* A default-initialized array of a *record* with no constructor reads indeterminate
  bytes (`P* ps = new P[2]` shows garbage). Same pre-existing behaviour as `new P`.
* One-past-the-end pointers are `unresolved` (not a tracked place).

## 2. Library types (`std::vector` and friends)

### Why they used to fail

Traced through the whole pipeline with `std::vector<int> v; v.push_back(10); ...`, there were
three independent losses, in the order they occur:

1. **Type description (the first loss).** A record was described to the runtime only if the
   analyzer generated a `lattice_rt_describe` for it: namespace-scope, non-template, in a
   project file. Any library class fell back to `TypeKind::Opaque` with an unknown value.
2. **Allocation.** `std::allocator` calls the global `operator new`, which was replaced but
   only *tracked* for the tagged overload that rewritten user `new` expressions select.
3. **Mutation.** A change was learned only from syntactic assignments to scalar/pointer
   lvalues in user code. `push_back` is a library call, so nothing was emitted.

The compiler, meanwhile, knew everything: libclang reports the complete layout of
`std::vector<int>`, including private members and the instantiated template.

### What was built (no container is named anywhere)

| Loss | Mechanism |
|---|---|
| Type description | `observe/layout.rs` reads a type's layout from libclang (`clang_Type_visitFields`, which works on implicit template instantiations, unlike walking the declaration) and produces plain data: primitives, pointers, arrays, records with named fields and byte offsets. The rewriter emits it as a self-contained lambda at the place the type is used (a variable, a parameter, a `new`, a field of a user struct), so there is nothing to order or include-guard, and the runtime turns it into the same type description it builds for user records. Project structs that already have a descriptor are *referenced*, not copied, so a `std::vector<Node>` and a `Node*` agree on one `Node`. |
| Allocation | Plain `operator new` blocks made while observed code runs are remembered but **not announced**. A block becomes an object when a typed pointer to its *start* is seen (in a variable, a field, a write): it is announced as the array `T[n]` the pointer says it is. No new URR event type was needed, and storage nothing points at (iostream buffers, say) never clutters the picture. |
| Mutation | After each statement in a braced block, the runtime compares the program's tracked memory with a byte snapshot of what it last reported and emits `ValueChanged` for the differing field or element (not the whole object). This sees `push_back`, `std::fill`, `memset`, `a[1] = 5` on a `std::array`, constructors, anything. |

Pointers one past the end of an array (a container's `end`/`last`) resolve to the model's
existing "out of bounds" place, `[Index(len)]`, so the link is kept and not shown as `untracked`.

What you see for `std::vector<int>` with three `push_back`s: a record `std::vector<int>` whose
three pointer members lead to the live buffer (`int[3]`), the two earlier buffers freed. Programs
using vectors of structs, strings, maps, sets, lists, deques, smart pointers, `std::function`,
stringstreams, `memset`/`fill`/`sort` run unchanged and produce a consistent URR
(`a_program_using_many_library_types_stays_correct_and_consistent`).

### Limits (chosen, and what they look like)

* **Raw members.** The vector shows the standard library's own member names, which differ
  between implementations. A readable `[10, 20, 30]` is a *presentation* rule over this data
  (a record whose pointer pair delimits a tracked array), to be added by structure, not by
  name. The data it needs is now there.
* **Capacity.** A buffer is announced at its allocated size; elements beyond the container's
  size are uninitialized memory and show whatever it holds.
* **Timing.** Changes are noticed at the next statement boundary. A container that frees its
  old buffer *before* repointing itself shows, for those few events, a pointer to a freed
  buffer: faithful to what happened, if surprising. With a lot of tracked state the comparison
  runs every n-th statement (n grows with the amount tracked, up to 256) so its cost per
  statement stays bounded; a change is then seen slightly later, and changes made just before
  a variable's scope ends may be missed.
* **Opaque on purpose.** Types with virtual functions, virtual or several bases, bit-fields, or
  that are incomplete are sized, named, opaque nodes (`std::ostringstream`, `std::function`,
  `std::shared_ptr`'s control block). Their storage is not announced.
* **Where a layout is attached.** At a variable/parameter declaration, a `new`, or a field of a
  user struct, and only when it is the first description of that type. A library object reached
  only through a pointer from somewhere else is described if its type is, but is not given a
  layout of its own. Templates, lambdas and `constexpr` functions are not instrumented
  (unchanged).
* **Typed by the pointer.** Storage is typed by the first typed pointer seen at its start; a
  `char*` to an `int` buffer would make it a `char` array. Interior pointers do not type a block.
* **ABI assumption.** A single data base class is placed at offset 0 (checked against the first
  member's offset); an empty base is ignored.

## 3. Limitations found in the existing architecture

* **Call sites are not reported.** `FunctionEntered.call_site` is always `null`; a frame's
  `line` is the last location *it* reported, so for a frame that is mid-call it is the last
  observed line, not the line of the call. The inspector therefore does not claim "called
  from line N".
* **Frames are dropped from the model on exit** (objects are kept with their lifetime).
  A selected frame that has returned is reported as "not on the stack at this step"; it
  cannot say whether it has not been entered yet or has finished.
* Locals declared without hooks (static locals, structured bindings, lambdas' bodies)
  never appear; a function with a `constexpr`/`try` body is not framed.
* Single-threaded stack in the runtime (documented milestone).
* A type first described as opaque stays opaque: the model rejects redeclaring a type.

## 4. Recommended next steps

1. **A presentation layer for sequences and strings** (UI-side): render a record whose
   pointer pair delimits a tracked array as `[a, b, c]` (and a char buffer as text), selected
   by structure. Toolchain differences in member names live there and nowhere else.
2. **Layouts for objects reached through pointers**, so a library object only ever seen via a
   pointer is described as well as one a variable names.
3. **Dirty tracking for the memory comparison** (page protection or write barriers) to replace
   the adaptive stride, restoring statement-exact attribution on large state.
4. **Instrument template bodies** (instantiations are in the AST already), so user templates
   get frames and hooks like any function.
