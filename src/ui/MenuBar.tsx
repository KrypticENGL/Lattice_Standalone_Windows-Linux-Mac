import { useEffect, useRef, useState } from "react";
import type { Project } from "../project/useProject";

/**
 * The application menu bar. Everything that acts on solutions and files lives here, not in
 * the editor's tab strip. Only a File menu for now.
 */
export function MenuBar({ project, enabled }: { project: Project; enabled: boolean }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", away);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", away);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const item = (label: string, keys: string, action: () => void) => (
    <button
      role="menuitem"
      className="menu-item"
      onClick={() => {
        setOpen(false);
        action();
      }}
    >
      <span>{label}</span>
      <span className="muted">{keys}</span>
    </button>
  );

  return (
    <nav className="menubar" aria-label="Application menu">
      <div className="menubar-menu" ref={root}>
        <button
          className={`menubar-button${open ? " open" : ""}`}
          aria-haspopup="menu"
          aria-expanded={open}
          disabled={!enabled}
          title={enabled ? undefined : "Solutions need the Lattice desktop app"}
          onClick={() => setOpen((o) => !o)}
        >
          File
        </button>
        {open && (
          <div className="menu" role="menu">
            {item("New Solution", "Ctrl+N", () => void project.newSolution())}
            {item("Open Solution…", "Ctrl+O", () => void project.openFile())}
            <div className="menu-sep" role="separator" />
            {item("Save", "Ctrl+S", () => void project.save())}
            {item("Save As…", "Ctrl+Shift+S", () => void project.saveAs())}
            <div className="menu-sep" role="separator" />
            {item("New File…", "", () => void project.newFile())}
            {item("Rename File…", "", () => void project.renameFile())}
            {item("Delete File", "", () => void project.deleteFile())}
          </div>
        )}
      </div>
    </nav>
  );
}
