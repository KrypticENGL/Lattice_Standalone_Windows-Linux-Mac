import { appConfig } from "../config/appConfig";
import { ChevronDownIcon, FolderIcon, PlayIcon, SettingsIcon } from "./icons";

/** Header. Every control here is a placeholder except the app name. */
export function Toolbar() {
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

      <button className="btn btn-run" disabled title="Execution is not implemented yet">
        <PlayIcon />
        Run
      </button>
      <button className="btn btn-icon" disabled title="Settings are not implemented yet" aria-label="Settings">
        <SettingsIcon />
      </button>
    </header>
  );
}
