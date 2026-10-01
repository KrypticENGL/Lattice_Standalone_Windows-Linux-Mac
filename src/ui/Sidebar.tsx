import type { ComponentType } from "react";
import { views, type ViewId } from "../app/views";
import { CanvasIcon, EditorIcon, PluginsIcon, PostsIcon, ProjectIcon, SavedIcon, VisualizerIcon } from "./icons";

const icons: Record<ViewId, ComponentType> = {
  editor: EditorIcon,
  visualizer: VisualizerIcon,
  project: ProjectIcon,
  plugins: PluginsIcon,
  canvas: CanvasIcon,
  posts: PostsIcon,
  saved: SavedIcon,
};

interface Props {
  active: ViewId;
  /** The Project entry is a drawer, not a page: it is lit while the drawer is open. */
  projectOpen: boolean;
  onSelect: (id: ViewId) => void;
}

export function Sidebar({ active, projectOpen, onSelect }: Props) {
  return (
    <nav className="sidebar" aria-label="Views">
      {views.map((v, i) => {
        const Icon = icons[v.id];
        const lit = v.id === "project" ? projectOpen : v.id === active;
        return (
          <button key={v.id} className={`nav-item ${lit ? "active" : ""}`}
            aria-current={lit ? "page" : undefined}
            aria-expanded={v.id === "project" ? projectOpen : undefined}
            title={`${v.label} (Ctrl+${i + 1})`}
            onClick={() => onSelect(v.id)}>
            <Icon />
            <span>{v.label}</span>
          </button>
        );
      })}
    </nav>
  );
}
