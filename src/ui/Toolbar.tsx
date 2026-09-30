import { appConfig } from "../config/appConfig";
import { ChevronDownIcon, FolderIcon, PlayIcon, SettingsIcon } from "./icons";

interface Props {
  running: boolean;
  onRun: () => void;
  onStop: () => void;
}

/** Header. Run/Stop are live; the project picker and settings are still placeholders. */
export function Toolbar({ running, onRun, onStop }: Props) {
  return (
    <header className="toolbar">
      <div className="brand">
        <img src="/lattice.svg" alt="" width={22} height={22} />
        <span className="brand-name">{appConfig.name}</span>
      </div>

      <button className="project-picker" disabled title="Projects are not implemented yet">
        <FolderIcon />
        <span>New Project</span>
        <ChevronDownIcon />
      </button>

      <div className="toolbar-spacer" />

      {running ? (
        <button className="btn btn-run" onClick={onStop} title="Stop the running program">
          Stop
        </button>
      ) : (
        <button className="btn btn-run" onClick={onRun} title="Compile and run (Ctrl+Enter)">
          <PlayIcon />
          Run
        </button>
      )}
      <button className="btn btn-icon" disabled title="Settings are not implemented yet" aria-label="Settings">
        <SettingsIcon />
      </button>
    </header>
  );
}
