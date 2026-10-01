import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import type { Project } from "../project/useProject";

export type MenuItem =
  | { separator: true }
  | { label: string; keys?: string; disabled?: boolean; action: () => void };

interface Open {
  x: number;
  y: number;
  items: MenuItem[];
}

type Show = (e: ReactMouseEvent | MouseEvent, items: MenuItem[]) => void;

const Ctx = createContext<Show>(() => {});

/** Shows the app's own context menu at the pointer. Closes on outside click, Esc, scroll, resize or focus loss. */
export function ContextMenuProvider({ children }: { children: ReactNode }) {
  const [open, setOpen] = useState<Open | null>(null);
  const box = useRef<HTMLDivElement | null>(null);
  const close = useCallback(() => setOpen(null), []);

  const show = useCallback<Show>((e, items) => {
    e.preventDefault();
    e.stopPropagation();
    setOpen({ x: e.clientX, y: e.clientY, items });
  }, []);

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    const el = box.current;
    if (!open || !el) return;
    const r = el.getBoundingClientRect();
    const x = Math.max(4, Math.min(open.x, window.innerWidth - r.width - 4));
    const y = Math.max(4, Math.min(open.y, window.innerHeight - r.height - 4));
    el.style.left = `${x}px`;
    el.style.top = `${y}px`;
    el.style.visibility = "visible";
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) close();
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("mousedown", away, true);
    window.addEventListener("keydown", esc);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    window.addEventListener("wheel", close, { passive: true });
    return () => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("keydown", esc);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      window.removeEventListener("wheel", close);
    };
  }, [open, close]);

  return (
    <Ctx.Provider value={show}>
      {children}
      {open && (
        <div
          ref={box}
          className="menu context-menu"
          role="menu"
          style={{ left: open.x, top: open.y, visibility: "hidden" }}
          onContextMenu={(e) => e.preventDefault()}
        >
          {open.items.map((it, i) =>
            "separator" in it ? (
              <div key={i} className="menu-sep" role="separator" />
            ) : (
              <button
                key={i}
                role="menuitem"
                className="menu-item"
                disabled={it.disabled}
                onClick={() => {
                  close();
                  it.action();
                }}
              >
                <span>{it.label}</span>
                <span className="muted">{it.keys ?? ""}</span>
              </button>
            ),
          )}
        </div>
      )}
    </Ctx.Provider>
  );
}

export const useContextMenu = () => useContext(Ctx);

/** The items that save the solution; shared by every right-click outside the solution explorer. */
export function saveItems(project: Project): MenuItem[] {
  return [
    { label: "Save", keys: "Ctrl+S", action: () => void project.save() },
    { label: "Save As…", keys: "Ctrl+Shift+S", action: () => void project.saveAs() },
  ];
}

/** `onContextMenu` for an area (editor, visualizer) whose right-click offers saving the solution. */
export function useSaveMenu(project: Project) {
  const show = useContextMenu();
  return useMemo(
    () => (e: ReactMouseEvent) => {
      // Monaco shows its own menu (which has Save Solution in it); don't stack a second one.
      if ((e.target as HTMLElement).closest(".monaco-editor")) return;
      show(e, saveItems(project));
    },
    [show, project],
  );
}

/** A wrapper element whose right-click offers saving the solution. */
export function SaveArea({ project, className, children }: { project: Project; className?: string; children: ReactNode }) {
  const onContextMenu = useSaveMenu(project);
  return (
    <div className={className} onContextMenu={onContextMenu}>
      {children}
    </div>
  );
}
