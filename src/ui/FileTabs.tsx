import type { Project } from "../project/useProject";

/**
 * One tab per open source file; a tab only shows its file's content. Closing a tab (the ×)
 * only hides it - the file stays in the solution and reopens from the solution explorer.
 * Adding, renaming and deleting files are File-menu operations, not done here.
 */
export function FileTabs({ project }: { project: Project }) {
  const closable = project.tabs.length > 1;
  return (
    <div className="file-tabs" role="tablist">
      {project.tabs.map((f) => (
        <div key={f} className={`file-tab${f === project.active ? " active" : ""}`}>
          <button role="tab" aria-selected={f === project.active} className="file-tab-name" onClick={() => project.select(f)}>
            {f}
          </button>
          {closable && (
            <button className="file-tab-close" aria-label={`Close ${f}`} title="Close" onClick={() => project.closeTab(f)}>
              ×
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
