import { appConfig } from "../config/appConfig";
import { ChevronDownIcon, FolderIcon, PlayIcon, SettingsIcon } from "./icons";

interface Props {
  running: boolean;
  onRun: () => void;
  onStop: () => void;
  /** Record the run's runtime state (shown in the Runtime tab). */
  observe: boolean;
  onObserveChange: (on: boolean) => void;
  /** Why observing is not possible right now, or null if it is. */
  observeUnavailable: string | null;
}

/** Header. Run/Stop and Observe are live; the project picker and settings are still placeholders. */
export function Toolbar({ running, onRun, onStop, observe, onObserveChange, observeUnavailable }: Props) {
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

      <button
        className="btn btn-toggle"
        aria-pressed={observe && !observeUnavailable}
        disabled={running || observeUnavailable !== null}
        onClick={() => onObserveChange(!observe)}
        title={
          observeUnavailable ??
          "Record the program's runtime state while it runs; browse it in the Runtime tab"
        }
      >
        Observe
      </button>

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
