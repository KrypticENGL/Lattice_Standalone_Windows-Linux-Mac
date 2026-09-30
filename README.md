# Lattice

Lattice is a desktop developer tool that will let you write C++ and see the
resulting data structures (arrays, linked lists, trees, graphs, hash tables, …)
rendered in 2D from the program's **runtime** behavior.

## Status

**Scaffold only.** The app opens a window with a placeholder layout:
toolbar, a Monaco C++ editor (with sample text), an empty visualization area and
Output / Errors / Runtime tabs.

The sidebar switches between Editor, Visualizer, Project, Plugins, Canvas, Posts and
Saved Posts (Ctrl+1–7). Editor and Visualizer show the placeholder layout; the
other five are empty "not implemented" pages.

**Not implemented yet:** running or compiling C++, runtime observation or
instrumentation, any data-structure visualization, file/project handling,
settings. The Run, project and settings controls are disabled placeholders.

## Technology stack

| Concern | Choice |
| --- | --- |
| Shell / native core | [Tauri 2](https://tauri.app) (Rust, WebView2) |
| UI | React 19 + TypeScript, bundled with Vite |
| Editor | Monaco (bundled locally, works offline) |
| Visualization renderer | **Undecided** |

See [docs/architecture.md](docs/architecture.md) for why, and the trade-offs.

## Project structure

```
src/                     Frontend (TypeScript)
  app/                   Root component, view registry (views.ts), editor view
  ui/                    Generic UI: toolbar, sidebar, panels, splitter, status bar, theme CSS
  editor/                Monaco integration
  visualization/         Future 2D rendering surface (placeholder)
  runtime/               Future frontend boundary for execution/observation
  toolchain/             Future frontend boundary for compiler integration
  model/                 Future shared data model
  config/                Frontend configuration
  utils/                 Helpers (native IPC wrapper)
src-tauri/               Native core (Rust)
  src/app/               IPC command surface
  src/runtime/           Future: run and observe user programs (empty)
  src/toolchain/         Future: C++ compiler discovery/integration (empty)
  src/model/             Future: shared data model (empty)
  src/config/            Configuration constants
  tauri.conf.json        Window, bundle and security configuration
docs/architecture.md     Stack decision and open questions
```

## Prerequisites

- Windows 10/11 with the WebView2 runtime (preinstalled on Windows 11)
- Node.js 20+ and npm
- Rust (stable, MSVC toolchain) via [rustup](https://rustup.rs)
- Visual Studio Build Tools with the "Desktop development with C++" workload

Everything else is installed locally by `npm install` and Cargo; the Tauri CLI
is a project dev dependency, not a global tool.

## Build and run

```powershell
npm install

# Development: hot-reloading UI, debug native build
npm run app:dev

# Debug executable only (no installer):  src-tauri\target\debug\lattice.exe
npm run app:build:debug

# Release executable only:               src-tauri\target\release\lattice.exe
npm run app:build:exe

# Release build + NSIS installer:        src-tauri\target\release\bundle\nsis\
npm run app:build
```

`app:build` downloads NSIS from GitHub on first use, so it needs internet access.
`app:build:exe` does not.

The first native build compiles all Rust dependencies and takes a few minutes.

Other scripts: `npm run dev` (UI only, in a browser at http://localhost:1420,
without the native shell), `npm run typecheck`.

## Development workflow

- UI work: `npm run app:dev`; frontend edits reload automatically, Rust edits
  trigger a rebuild.
- Keep UI code in `src/ui` and `src/app`. New execution/visualization
  functionality belongs in its own module (`runtime`, `visualization`,
  `toolchain`, `model`) on both the frontend and native side, not in the UI layer.
- Release profile (`src-tauri/Cargo.toml`) enables LTO, single codegen unit and
  symbol stripping.

## Planned high-level components

Not built; listed only to show intended module boundaries.

- **Editor**: C++ editing, diagnostics
- **Toolchain**: locate and drive a C++ compiler
- **Runtime**: run the user's program and observe its state
- **Model**: representation of observed data structures
- **Visualization**: interactive 2D rendering of that model
