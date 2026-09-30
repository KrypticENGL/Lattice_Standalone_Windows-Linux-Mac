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

Not committed to in this scaffold: how the user's program is executed and
observed (instrumentation, debugger, interpreter, …), the wire format between
runtime and UI, the visualization data model, and the renderer technology.
`runtime`, `toolchain`, `model` and `visualization` are placeholders for those
decisions.
