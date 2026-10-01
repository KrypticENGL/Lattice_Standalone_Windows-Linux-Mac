import { useEffect, useState } from "react";
import { joinPath, listDir, type FsEntry } from "../project/fs";
import { MANIFEST_NAME } from "../project/solution";
import type { Project } from "../project/useProject";
import { saveItems, useContextMenu, type MenuItem } from "./ContextMenu";
import { isNative } from "../utils/native";

interface Props {
  project: Project;
  open: boolean;
  onClose: () => void;
  /** Changes when a run finishes, so the `target` folder is listed again. */
  refreshKey: string | null;
}

const kind = (name: string) => (/\.(h|hpp|hh)$/i.test(name) ? "h" : /\.(cpp|cc|cxx)$/i.test(name) ? "c" : /\.exe$/i.test(name) ? "x" : "f");

function Row({ depth, label, badge, active, onClick, muted, title, onContextMenu }: {
  depth: number;
  label: string;
  badge?: string;
  active?: boolean;
  onClick?: () => void;
  muted?: boolean;
  title?: string;
  onContextMenu?: (e: React.MouseEvent) => void;
}) {
  const content = (
    <>
      <span className={`tree-icon tree-${badge ?? "f"}`} aria-hidden>
        {badge === "dir" ? "▾" : ""}
      </span>
      <span className="tree-label">{label}</span>
    </>
  );
  const cls = `tree-row${active ? " active" : ""}${muted ? " muted" : ""}`;
  const style = { paddingLeft: 10 + depth * 14 };
  return onClick ? (
    <button className={cls} style={style} onClick={onClick} title={title} onContextMenu={onContextMenu}>
      {content}
    </button>
  ) : (
    <div className={cls} style={style} title={title} onContextMenu={onContextMenu}>
      {content}
    </div>
  );
}

/**
 * The solution explorer: a drawer that slides in beside the sidebar (not a page of its own).
 * Shows `lattice.sln`, the source files (live: unsaved additions and renames included; click
 * to open a file's tab) and what the runs left in `target`.
 */
export function SolutionExplorer({ project, open, onClose, refreshKey }: Props) {
  const [built, setBuilt] = useState<FsEntry[]>([]);
  /** The folder row last clicked; New File goes there. */
  const [folder, setFolder] = useState<"src" | "target">("src");
  const show = useContextMenu();

  const folderItems = (where: "src" | "target" = folder): MenuItem[] => [
    {
      label: where === "target" ? "New File… (not in target)" : "New File…",
      disabled: where === "target",
      action: () => void project.newFile(),
    },
    { separator: true },
    ...saveItems(project),
  ];
  const fileItems = (f: string): MenuItem[] => [
    { label: "Open", action: () => project.select(f) },
    { label: "Rename…", action: () => void project.renameFile(f) },
    { label: "Delete", disabled: project.files.length <= 1, action: () => void project.deleteFile(f) },
    { separator: true },
    ...folderItems("src"),
  ];

  useEffect(() => {
    if (!open || !project.path || !isNative()) {
      setBuilt([]);
      return;
    }
    let live = true;
    listDir(joinPath(project.path, "target"), true)
      .then((l) => live && setBuilt(l.entries.filter((e) => !e.isDir)))
      .catch(() => live && setBuilt([]));
    return () => {
      live = false;
    };
  }, [open, project.path, refreshKey]);

  useEffect(() => {
    if (!open) return;
    const esc = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [open, onClose]);

  return (
    <>
      <div className={`drawer-scrim${open ? " open" : ""}`} onMouseDown={onClose} aria-hidden />
      <div className={`drawer-clip${open ? " open" : ""}`}>
      <aside className={`drawer${open ? " open" : ""}`} aria-label="Solution explorer" aria-hidden={!open} inert={!open}>
        <header className="drawer-header">
          <span>Solution Explorer</span>
          <button className="btn btn-icon drawer-close" onClick={onClose} aria-label="Close solution explorer" title="Close (Esc)">
            ×
          </button>
        </header>
        <div className="drawer-body" role="tree" onContextMenu={(e) => show(e, folderItems())}>
          <Row depth={0} badge="sln" label={`Solution '${project.title}'${project.dirty ? " ●" : ""}`} title={project.path ?? "Not saved yet"} />
          <Row depth={1} badge="manifest" label={MANIFEST_NAME} muted={!project.path} title={project.path ? undefined : "Created when the solution is saved"} />
          <Row depth={1} badge="dir" label="src" active={folder === "src"} onClick={() => setFolder("src")} onContextMenu={(e) => { setFolder("src"); show(e, folderItems("src")); }} />
          {project.files.map((f) => (
            <Row key={f} depth={2} badge={kind(f)} label={f} active={f === project.active && folder === "src"} onClick={() => { setFolder("src"); project.select(f); }} title={`Open ${f}`} onContextMenu={(e) => show(e, fileItems(f))} />
          ))}
          <Row depth={1} badge="dir" label="target" active={folder === "target"} muted={!project.path} title="Build output; source files cannot be created here" onClick={() => setFolder("target")} onContextMenu={(e) => { setFolder("target"); show(e, folderItems("target")); }} />
          {built.map((e) => (
            <Row key={e.path} depth={2} badge={kind(e.name)} label={e.name} muted />
          ))}
          {project.path && built.length === 0 && <Row depth={2} label="nothing built yet" muted />}
        </div>
        {!project.path && <p className="drawer-note">This solution is not saved yet. Use File → Save to create it on disk.</p>}
      </aside>
      </div>
    </>
  );
}
