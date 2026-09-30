/** Top-level views reachable from the sidebar. Order = sidebar order = Ctrl+N. */
export const views = [
  { id: "editor", label: "Editor", description: null },
  { id: "visualizer", label: "Visualizer", description: null },
  { id: "project", label: "Project", description: "File tree for the current project." },
  { id: "plugins", label: "Plugins", description: "Extensions built on Lattice's plugin ecosystem." },
  { id: "canvas", label: "Canvas", description: "Node-graph programming: build programs by connecting nodes instead of writing code." },
  { id: "posts", label: "Posts", description: "Browse posts." },
  { id: "saved", label: "Saved Posts", description: "Posts you have saved." },
] as const;

export type ViewId = (typeof views)[number]["id"];
