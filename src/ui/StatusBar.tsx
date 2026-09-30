import type { AppInfo } from "../utils/native";

interface Props {
  line: number;
  column: number;
  info: AppInfo | null;
}

export function StatusBar({ line, column, info }: Props) {
  return (
    <div className="statusbar">
      <span>Ready</span>
      <span className="toolbar-spacer" />
      <span>Ln {line}, Col {column}</span>
      <span>Spaces: 4</span>
      <span>UTF-8</span>
      <span>C++</span>
      {info && <span>{info.name} v{info.version}</span>}
    </div>
  );
}
