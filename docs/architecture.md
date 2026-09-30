# Architecture notes

## Stack decision

Requirements that drove the choice: a high-quality code editor, interactive 2D
rendering, native process/toolchain control, good performance, and a normal
Windows executable.

| Option | Editor | 2D rendering | Native process control | Verdict |
| --- | --- | --- | --- | --- |
| **Tauri 2 (Rust + WebView2 + TS)** | Monaco | Canvas/SVG/WebGL, mature | Rust: strong | **Chosen** |
| Electron | Monaco | same | Node: adequate | Same UI story, much larger footprint, weaker native side |
| .NET (WPF/WinUI) | AvalonEdit / Scintilla: weaker | Skia/WPF: good | .NET: strong | Editor is the weak point; WinUI tooling is heavier |
| Qt (C++) | QScintilla / custom | QGraphicsView/QtQuick: good | C++: strong | Heavier build/licensing/dependency story, slower UI iteration |
| Dear ImGui / native C++ | Weak | Good | Strong | Editor and UI polish would be built from scratch |

**Why Tauri:** Monaco is the strongest editor component available, and the
web platform is the best-supported environment for interactive 2D graphics and
rich UI. Rust provides a robust native layer for spawning and supervising
processes and invoking compilers, without tying the app to any one execution
strategy. Output is a small native `.exe` plus an NSIS installer.

**Trade-offs:**
- The UI runs in WebView2 (Chromium), so rendering is bounded by the webview. Very
  large visualizations may eventually need WebGL/WASM, or a native renderer.
- UI and native code communicate over IPC; anything high-frequency (e.g. a stream
  of runtime events) will need deliberate design.
- Monaco is large (~4 MB main chunk). Acceptable for a local desktop app.
- Two toolchains (Node + Rust) are required to build.

## Deliberately undecided

Still not decided: how the user's program is *observed* (instrumentation,
debugger, interpreter, ...), the runtime-event wire format, the visualization
data model, and the renderer technology. `model` and `visualization` remain
placeholders. **Runtime data-structure visualization is NOT implemented yet.**
What exists is a real compile-and-run pipeline (below) with a marked seam where
observation will attach.

## Execution engine (current)

Lattice compiles and runs the user's real C++ with a real compiler. There is no
interpreter and no Lattice-specific API the user must write against.

### Layers

```
UI (React)            src/ui, src/app, src/runtime/useExecution.ts
  |  invoke / events  (Tauri IPC: run_program, stop_program, toolchain_status)
Application shell     src-tauri/src/app/commands.rs   (thin; no logic)
  |
runtime               sessions, workspace, ExecutionManager, instrumentation seam
  |            \
toolchain      process
Compiler trait   run_process: spawn, capture, timeout, tree-kill
+ discovery
```

Dependencies point downward only. `toolchain` and `process` know nothing about
sessions or the UI; the UI never spawns processes; the visualization layer has
no dependency on OS process APIs.

| Module | Responsibility |
| --- | --- |
| `process` | `run_process(spec, cancel)`: stdout/stderr capture (capped), wall-clock timeout, cancellation, whole-tree kill via a Windows Job Object, no console window |
| `toolchain` | `Compiler` trait, `CompileRequest`, `CompilerResult`, `CompilerDiagnostic`; `gnu.rs` drives Clang and GCC; `discovery.rs` finds them |
| `runtime::session` | `RuntimeSession` record, `SessionState` machine, `TerminationReason` |
| `runtime::workspace` | per-session directories, source validation, stale sweep |
| `runtime::manager` | `ExecutionManager`: the only orchestrator; UI-independent |
| `runtime::instrumentation` | `Instrumenter`, `BuildPlan`, `RuntimeEventStream`: the future seam (no-op today) |
| `config` | `ExecutionConfig`: timeouts, output cap, cleanup policy, runtime root, C++ standard |

### Compiler flow

1. Discovery (`Toolchain::discover`) at startup: `LATTICE_CXX` override, then
   `clang++`, then `g++`, each on `PATH` and in known install dirs (LLVM,
   scoop, MSYS2, MinGW). Each candidate is probed with `--version`; the first
   valid one is preferred. Nothing is hard-coded to one install.
2. `Compiler::compile` runs `<cxx> -std=c++20 -O0 -g -Wall -Wextra
   -fdiagnostics-color=never <sources> -o <exe>` in the workspace `source/` dir.
   Output is parsed into `CompilerDiagnostic`s (severity, file, line, column,
   message, context). Linker errors with no position are surfaced as a single
   error carrying the raw output.
3. A different compiler (MSVC `cl`, remote, ...) is a new `Compiler` impl; the
   runtime layer is unchanged.

### Execution flow

`Run` -> `run_program` (async command, work on a blocking thread) ->
`ExecutionManager::run`:

validate sources -> pick compiler -> create workspace -> write sources ->
`Instrumenter::prepare` (no-op) -> **compile** -> (fail: `CompilationFailed`) ->
**run** the exe with stdin closed, cwd = `executable/`, compiler dir prepended
to `PATH` (MinGW runtime DLLs) -> capture -> classify exit -> cleanup -> return
the final `RuntimeSession`. Every state change is also emitted as the
`lattice://session-state` event so the UI can show "Compiling..." / "Running...".
`stop_program` cancels the session's token; the process tree is killed.

### Temporary workspace strategy

```
%LOCALAPPDATA%\Lattice\Runtime\<session id>\
    source/  build/  executable/  output/
```

One isolated directory per session; nothing is written to the user's project or
the app's source tree. Cleanup policy (`CleanupPolicy`): `Always` (default),
`KeepOnFailure`, `Never`. Directories from crashed runs (`s-*`, older than 24 h)
are swept at startup. Tests assert isolation and cleanup.

### Execution lifecycle

```
Idle -> Compiling -> CompilationFailed
                  -> Ready -> Running -> Completed   (exit code 0)
                                      -> Failed      (non-zero exit, launch failure)
                                      -> TimedOut    (limit exceeded, killed)
                                      -> Terminated  (stopped by the user)
Idle/Compiling -> Failed   (no compiler, bad file name, workspace error)
Compiling/Ready -> Terminated (stopped before the program started)
```

Transitions are enforced by `SessionState::can_transition_to`; the terminal
states have no outgoing edges. `RuntimeSession` carries id, sources, compiler,
diagnostics, executable path, start/end/duration (plus compile and run
durations), stdout, stderr, exit code and `TerminationReason`.

### Process safety - read this

**The execution environment is NOT a security sandbox.** The user's program
runs as the current Windows user with full access to files, network and
environment. Only run code you trust. What is provided is *runaway protection*:
a wall-clock run timeout (default 10 s), a compile timeout (60 s), a per-stream
output cap (1 MiB), whole-process-tree termination (Job Object,
`KILL_ON_JOB_CLOSE`), stdin closed, and workspace cleanup. Not provided: memory
or CPU limits, file/network restrictions, privilege reduction.

### Logging

`tracing`, filter via `LATTICE_LOG` (default `lattice_lib=info`). Logged:
compiler discovery, session state changes, compilation start/end and
diagnostic messages, process start/kill/finish, timeouts, workspace cleanup.
Source code is never logged (only diagnostic message text).

### Testing

`cargo test` in `src-tauri` runs unit tests plus `tests/execution.rs`, which
drives `ExecutionManager` against the real compiler with no UI: hello/stdout,
stderr, compile error, non-zero exit, infinite loop -> timeout, no output,
user stop, workspace isolation/cleanup, state ordering, name validation.
Programs live in `src-tauri/tests/programs/`. Tests skip (loudly) if no compiler
is installed; set `LATTICE_REQUIRE_COMPILER=1` to fail instead.

### Current limitations

- No sandboxing (see above). No stdin support (closed). One file (`main.cpp`)
  from the UI, although the engine accepts multi-file projects.
- Requires a compiler on the machine (Clang preferred, GCC/MinGW accepted; no
  MSVC driver yet). No bundled toolchain. Clang on Windows also needs a C++
  standard library (MSVC build tools or MinGW) that it can find itself.
- Timeouts are wall-clock, not CPU time. A grandchild spawned in the instant
  between process start and job assignment could escape the tree kill.
- Output is collected and returned when the session ends, not streamed live.
- No runtime observation and no visualization: the Visualization panel is a placeholder.

### Planned runtime-instrumentation boundary

```
sources -> Instrumenter::prepare -> compile (+BuildPlan.extra_compile_args)
        -> executable -> run (+BuildPlan.run_env) -> RuntimeEventStream
        -> runtime data model -> visualization
```

`runtime::instrumentation` marks where this plugs in. `PassThrough` leaves the
program untouched. A future strategy can extend `BuildPlan` (rewrite sources,
add flags, link a runtime library, set environment) and provide a
`RuntimeEventStream`, which the manager already opens before the run and closes
after it. The event schema and the strategy are intentionally undefined; the
engine makes no assumption that the executable is uninstrumented.
