import { useEffect, useState } from "react";
import { clangd } from "../lsp/controller";
import { useClangd } from "../lsp/useClangd";

interface Props {
  onClose: () => void;
}

const SOURCE_LABEL = {
  settings: "saved setting",
  environment: "LATTICE_CLANGD",
  path: "PATH",
  knownLocation: "known install location",
} as const;

/** Where the clangd executable is chosen and where its status is explained. */
export function ClangdDialog({ onClose }: Props) {
  const language = useClangd();
  const [path, setPath] = useState(language.status?.configuredPath ?? "");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const apply = async (value: string | null) => {
    setBusy(true);
    setError(null);
    try {
      await clangd.configure(value);
      if (value === null) setPath("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const found = language.status?.clangd;
  const failed = language.phase === "crashed" || language.phase === "error";
  return (
    <div className="dialog-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="clangd settings">
        <h2>clangd (C++ code intelligence)</h2>
        <p className={found ? "" : "dialog-problem"}>
          {found
            ? <>Using <code>{found.path}</code> — {found.version} ({SOURCE_LABEL[found.source]}).</>
            : language.message ?? "Checking for clangd…"}
        </p>
        {failed && <p className="dialog-problem">{language.message}</p>}
        <label className="dialog-field">
          <span>Path to clangd executable</span>
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="Leave empty to auto-detect (PATH, LLVM, MSYS2, JetBrains IDEs)"
            spellCheck={false}
          />
        </label>
        {error && <p className="dialog-problem">{error}</p>}
        {language.status && !found && language.status.searched.length > 0 && (
          <details>
            <summary>Where Lattice looked</summary>
            <ul>{language.status.searched.map((s) => <li key={s}>{s}</li>)}</ul>
          </details>
        )}
        <div className="dialog-actions">
          <button className="btn" disabled={busy} onClick={() => void apply(null)}>Use auto-detect</button>
          <button
            className="btn"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void clangd.rescan().finally(() => setBusy(false));
            }}
          >
            Rescan
          </button>
          <span className="toolbar-spacer" />
          <button className="btn" onClick={onClose}>Close</button>
          <button className="btn btn-run" disabled={busy || !path.trim()} onClick={() => void apply(path)}>Save</button>
        </div>
      </div>
    </div>
  );
}
