import type { ReactNode } from "react";

const Svg = ({ children, size = 16 }: { children: ReactNode; size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor"
    strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {children}
  </svg>
);

export const PlayIcon = () => <Svg><path d="M4.5 3v10l8-5z" fill="currentColor" /></Svg>;
export const SettingsIcon = () => (
  <Svg><circle cx="8" cy="8" r="2.2" /><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4" /></Svg>
);
export const FolderIcon = () => <Svg><path d="M1.5 4.5a1 1 0 0 1 1-1H6l1.5 1.5h6a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z" /></Svg>;
export const ChevronDownIcon = () => <Svg><path d="M4 6l4 4 4-4" /></Svg>;

// Sidebar icons (20px)
export const EditorIcon = () => <Svg size={20}><path d="M5.5 4.5 2 8l3.5 3.5M10.5 4.5 14 8l-3.5 3.5" /></Svg>;
export const VisualizerIcon = () => (
  <Svg size={20}><circle cx="8" cy="3.5" r="1.7" /><circle cx="3.5" cy="12" r="1.7" /><circle cx="12.5" cy="12" r="1.7" /><path d="M7.2 5 4.3 10.4M8.8 5l2.9 5.4M5.2 12h5.6" /></Svg>
);
export const ProjectIcon = () => <Svg size={20}><path d="M1.5 4.5a1 1 0 0 1 1-1H6l1.5 1.5h6a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1z" /></Svg>;
export const PluginsIcon = () => (
  <Svg size={20}><path d="M6 2v3M10 2v3M4 5h8v3a4 4 0 0 1-8 0zM8 12v2.5" /></Svg>
);
export const CanvasIcon = () => (
  <Svg size={20}><rect x="1.8" y="2.5" width="4.5" height="3.5" rx=".8" /><rect x="9.7" y="10" width="4.5" height="3.5" rx=".8" /><path d="M6.3 4.2h1.7a1.5 1.5 0 0 1 1.5 1.5v4.3" /></Svg>
);
export const PostsIcon = () => (
  <Svg size={20}><rect x="2" y="2.5" width="12" height="11" rx="1.2" /><path d="M4.8 6h6.4M4.8 8.5h6.4M4.8 11h3.5" /></Svg>
);
export const SavedIcon = () => <Svg size={20}><path d="M4 2.5h8v11L8 10.5 4 13.5z" /></Svg>;
