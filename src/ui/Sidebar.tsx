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
  onSelect: (id: ViewId) => void;
}

export function Sidebar({ active, onSelect }: Props) {
  return (
    <nav className="sidebar" aria-label="Views">
      {views.map((v, i) => {
        const Icon = icons[v.id];
        return (
          <button key={v.id} className={`nav-item ${v.id === active ? "active" : ""}`}
            aria-current={v.id === active ? "page" : undefined}
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
