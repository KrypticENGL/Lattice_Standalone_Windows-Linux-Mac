# Lattice solutions

A solution is a **folder**, opened through its `lattice.sln`. Code: `src-tauri/src/solution/mod.rs`.

```
MyProject/
  lattice.sln          manifest (JSON); opens the solution
  src/                 source files, one real file each (.cpp .cc .cxx .h .hpp .hh; flat)
  target/              executables kept from runs
  .lattice/urr.json    URR state of the last observed run (optional)
```

`lattice.sln`:

```json
{ "format": "lattice-solution", "version": 2, "name": "MyProject",
  "createdAtMs": 0, "updatedAtMs": 0,
  "files": ["main.cpp", "list.h"], "activeFile": "main.cpp",
  "settings": { "observe": true }, "hasUrr": true }
```

- `files` is the tab order; sources found in `src/` that it does not list (added by hand) are appended on open.
- `urr.json` holds `{schema, sourceHash, events}`: the URR as its events only. Snapshots, timeline steps and
  visualization data are rebuilt by replaying them, so they cannot disagree. `sourceHash` flags a recording made
  from older sources. A damaged `urr.json` loses the recording, never the sources.
- Saving writes atomically (temp file + rename), writes the manifest last, removes only source files an earlier
  save listed, and never touches other files in the folder. A newer manifest `version` is refused.
- Running with a saved solution also copies the built executable into `target/`.

In the app: **File** menu (New Solution Ctrl+N, Open Solution… Ctrl+O, Save Ctrl+S, Save As… Ctrl+Shift+S, New File,
Rename File, Delete File). A new solution starts as Hello World and asks where to save it. Editor tabs only switch
between files. The last solution is reopened on start.
