import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createDir, joinPath, listDir, places, type FsListing, type Place } from "../project/fs";
import { defaultParentFolder, solutionNameProblem } from "../project/solution";
import { FolderIcon } from "./icons";

/** What the app asks the user. Resolved with the answer, or null if cancelled. */
export type DialogSpec =
  | { kind: "text"; title: string; label: string; value: string; action: string; check: (v: string) => string | null }
  | { kind: "location"; title: string; name: string; parent: string; action: string }
  | { kind: "explorer"; mode: ExplorerMode; title: string; start?: string }
  | { kind: "confirm"; title: string; text: string; action: string }
  | { kind: "message"; title: string; text: string; tone: "info" | "warning" | "error" };

/** `solution`: pick a Lattice solution to open. `folder`: pick a folder. */
export type ExplorerMode = "solution" | "folder";

export type DialogAnswer = string | { name: string; parent: string };

/** Must match the exit animation in layout.css. */
const EXIT_MS = 140;

/** Run a closing animation before reporting the answer, so dialogs leave as smoothly as they arrive. */
function useExit(onDone: (answer: DialogAnswer | null) => void) {
  const [closing, setClosing] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const finish = useCallback(
    (answer: DialogAnswer | null) => {
      if (timer.current !== undefined) return;
      setClosing(true);
      timer.current = window.setTimeout(() => onDone(answer), EXIT_MS);
    },
    [onDone],
  );
  return { closing, finish };
}

function Backdrop({ closing, onCancel, children, layer = 0 }: { closing: boolean; onCancel: () => void; children: ReactNode; layer?: number }) {
  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onCancel();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [onCancel]);
  return (
    <div
      className={`modal-backdrop${closing ? " closing" : ""}`}
      style={{ zIndex: 50 + layer }}
      onMouseDown={(e) => e.target === e.currentTarget && onCancel()}
    >
      {children}
    </div>
  );
}

function Shell({ title, action, valid, problem, onSubmit, onCancel, closing, hideCancel, children }: {
  title: string;
  action: string;
  valid: boolean;
  problem: string | null;
  onSubmit: () => void;
  onCancel: () => void;
  closing: boolean;
  hideCancel?: boolean;
  children: ReactNode;
}) {
  return (
    <Backdrop closing={closing} onCancel={onCancel}>
      <form
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onSubmit={(e) => {
          e.preventDefault();
          if (valid) onSubmit();
        }}
      >
        <h2 className="modal-title">{title}</h2>
        {children}
        <p className="modal-problem">{problem ?? " "}</p>
        <div className="modal-actions">
          {!hideCancel && (
            <button type="button" className="btn" onClick={onCancel}>
              Cancel
            </button>
          )}
          <button type="submit" className="btn btn-run" disabled={!valid} autoFocus={hideCancel}>
            {action}
          </button>
        </div>
      </form>
    </Backdrop>
  );
}

interface Props {
  spec: DialogSpec;
  onDone: (answer: DialogAnswer | null) => void;
}

export function Dialog({ spec, onDone }: Props) {
  const { closing, finish } = useExit(onDone);
  switch (spec.kind) {
    case "text":
      return <TextDialog spec={spec} closing={closing} finish={finish} />;
    case "location":
      return <LocationDialog spec={spec} closing={closing} finish={finish} />;
    case "explorer":
      return <Explorer mode={spec.mode} title={spec.title} start={spec.start} closing={closing} onPick={finish} />;
    case "confirm":
      return (
        <Shell title={spec.title} action={spec.action} valid problem={null} closing={closing} onSubmit={() => finish("ok")} onCancel={() => finish(null)}>
          <p className="modal-text">{spec.text}</p>
        </Shell>
      );
    case "message":
      return (
        <Shell title={spec.title} action="OK" valid problem={null} closing={closing} hideCancel onSubmit={() => finish("ok")} onCancel={() => finish("ok")}>
          <p className={`modal-text tone-${spec.tone}`}>{spec.text}</p>
        </Shell>
      );
  }
}

type Finish = (answer: DialogAnswer | null) => void;

function TextDialog({ spec, closing, finish }: { spec: Extract<DialogSpec, { kind: "text" }>; closing: boolean; finish: Finish }) {
  const [value, setValue] = useState(spec.value);
  const problem = spec.check(value.trim());
  return (
    <Shell
      title={spec.title}
      action={spec.action}
      valid={problem === null}
      problem={value === "" || value === spec.value ? null : problem}
      closing={closing}
      onSubmit={() => finish(value.trim())}
      onCancel={() => finish(null)}
    >
      <label className="modal-field">
        <span>{spec.label}</span>
        <input autoFocus value={value} onChange={(e) => setValue(e.target.value)} onFocus={(e) => e.currentTarget.select()} />
      </label>
    </Shell>
  );
}

function LocationDialog({ spec, closing, finish }: { spec: Extract<DialogSpec, { kind: "location" }>; closing: boolean; finish: Finish }) {
  const [name, setName] = useState(spec.name);
  const [parent, setParent] = useState(spec.parent);
  const [browsing, setBrowsing] = useState(false);
  const nameProblem = solutionNameProblem(name);
  const problem = nameProblem ?? (parent === "" ? "Choose where to save it." : null);
  return (
    <>
      <Shell
        title={spec.title}
        action={spec.action}
        valid={problem === null}
        problem={name === spec.name && nameProblem === null && parent !== "" ? null : problem}
        closing={closing}
        onSubmit={() => finish({ name, parent })}
        onCancel={() => finish(null)}
      >
        <label className="modal-field">
          <span>Solution name</span>
          <input autoFocus value={name} onChange={(e) => setName(e.target.value)} onFocus={(e) => e.currentTarget.select()} />
        </label>
        <div className="modal-field">
          <span>Location</span>
          <div className="modal-row">
            <input value={parent} onChange={(e) => setParent(e.target.value)} placeholder="Choose a folder…" />
            <button type="button" className="btn" onClick={() => setBrowsing(true)}>
              Browse…
            </button>
          </div>
        </div>
        <p className="modal-note">
          Creates <b>{parent && !nameProblem ? joinPath(parent, name) : "…"}</b> with <code>lattice.sln</code>, a <code>src</code> folder for
          the sources and a <code>target</code> folder for built programs.
        </p>
      </Shell>
      {browsing && (
        <ExplorerLayer
          mode="folder"
          title="Choose a location"
          start={parent || undefined}
          onClose={(p) => {
            setBrowsing(false);
            if (p) setParent(p);
          }}
        />
      )}
    </>
  );
}

/** An explorer opened from inside another dialog; animates in and out on its own. */
function ExplorerLayer({ mode, title, start, onClose }: { mode: ExplorerMode; title: string; start?: string; onClose: (path: string | null) => void }) {
  const { closing, finish } = useExit((a) => onClose(typeof a === "string" ? a : null));
  return <Explorer mode={mode} title={title} start={start} closing={closing} onPick={finish} layer={1} />;
}

// ---- the file explorer -------------------------------------------------------

function Explorer({ mode, title, start, closing, onPick, layer = 0 }: {
  mode: ExplorerMode;
  title: string;
  start?: string;
  closing: boolean;
  onPick: (path: string | null) => void;
  layer?: number;
}) {
  const [quick, setQuick] = useState<Place[]>([]);
  const [listing, setListing] = useState<FsListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<string | null>(null);
  const [pathText, setPathText] = useState("");
  const [newFolder, setNewFolder] = useState<string | null>(null);
  const ticket = useRef(0);

  const go = useCallback(async (path: string) => {
    const mine = ++ticket.current;
    setLoading(true);
    try {
      const l = await listDir(path);
      if (mine !== ticket.current) return;
      setListing(l);
      setPathText(l.path);
      setSelected(null);
      setError(null);
    } catch (e) {
      if (mine === ticket.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (mine === ticket.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let live = true;
    void (async () => {
      const p = await places();
      if (!live) return;
      setQuick(p);
      await go(start || (await defaultParentFolder()) || p[0]?.path || "/");
    })();
    return () => {
      live = false;
    };
  }, [go, start]);

  const entries = useMemo(
    () => (listing?.entries ?? []).filter((e) => mode === "solution" || e.isDir),
    [listing, mode],
  );
  const sel = entries.find((e) => e.path === selected) ?? null;
  // The current folder is itself a solution when it lists a lattice.sln.
  const hereIsSolution = (listing?.entries ?? []).some((e) => !e.isDir && e.isSolution);

  const target: string | null =
    mode === "folder" ? (sel?.isDir ? sel.path : (listing?.path ?? null)) : sel?.isSolution ? sel.path : hereIsSolution ? (listing?.path ?? null) : null;
  const action = mode === "folder" ? (sel?.isDir ? "Select folder" : "Select this folder") : "Open";

  const activate = (e: (typeof entries)[number]) => {
    if (!e.isDir || (mode === "solution" && e.isSolution)) onPick(e.path);
    else void go(e.path);
  };

  const makeFolder = async () => {
    if (newFolder === null || !listing) return;
    const name = newFolder.trim();
    if (name === "" || /[\\/:*?"<>|]/.test(name)) {
      setError("Use a plain folder name.");
      return;
    }
    try {
      const path = joinPath(listing.path, name);
      await createDir(path);
      setNewFolder(null);
      await go(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <Backdrop closing={closing} onCancel={() => onPick(null)} layer={layer}>
      <div className="modal modal-wide explorer" role="dialog" aria-modal="true" aria-label={title}>
        <h2 className="modal-title">{title}</h2>
        <div className="explorer-bar">
          <button className="btn btn-icon" disabled={!listing?.parent} onClick={() => listing?.parent && void go(listing.parent)} title="Up one folder" aria-label="Up one folder">
            ↑
          </button>
          <input
            className="explorer-path"
            value={pathText}
            onChange={(e) => setPathText(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void go(pathText)}
            aria-label="Folder path"
            spellCheck={false}
          />
          <button className="btn" onClick={() => setNewFolder("")} title="Create a folder here">
            New folder
          </button>
        </div>
        <div className="explorer-main">
          <div className="explorer-places">
            {quick.map((p) => (
              <button key={p.path} className={`explorer-place${listing?.path === p.path ? " active" : ""}`} onClick={() => void go(p.path)} title={p.path}>
                {p.name}
              </button>
            ))}
          </div>
          <div className="explorer-list" role="listbox" aria-busy={loading}>
            {newFolder !== null && (
              <div className="explorer-row">
                <FolderIcon />
                <input
                  autoFocus
                  className="explorer-new"
                  placeholder="Folder name"
                  value={newFolder}
                  onChange={(e) => setNewFolder(e.target.value)}
                  onKeyDown={(e) => (e.key === "Enter" ? void makeFolder() : e.key === "Escape" ? (e.stopPropagation(), setNewFolder(null)) : undefined)}
                />
              </div>
            )}
            {entries.map((e) => (
              <div
                key={e.path}
                role="option"
                aria-selected={e.path === selected}
                tabIndex={0}
                className={`explorer-row${e.path === selected ? " selected" : ""}${e.isSolution ? " solution" : ""}`}
                onClick={() => setSelected(e.path)}
                onDoubleClick={() => activate(e)}
                onKeyDown={(ev) => ev.key === "Enter" && activate(e)}
              >
                <FolderIcon />
                <span className="explorer-name">{e.name}</span>
                {e.isSolution && <span className="explorer-badge">Lattice solution</span>}
              </div>
            ))}
            {!loading && entries.length === 0 && !error && (
              <p className="explorer-empty">{mode === "solution" ? "No folders here." : "No folders here. You can select this one."}</p>
            )}
          </div>
        </div>
        <p className="modal-problem">
          {error ?? (mode === "solution" && target === null ? "Pick a folder marked “Lattice solution”, or open a folder to find one." : " ")}
        </p>
        <div className="modal-actions">
          <button className="btn" onClick={() => onPick(null)}>
            Cancel
          </button>
          <button className="btn btn-run" disabled={target === null} onClick={() => target && onPick(target)}>
            {action}
          </button>
        </div>
      </div>
    </Backdrop>
  );
}
