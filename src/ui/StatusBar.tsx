import type { ClangdSnapshot, Phase } from "../lsp/controller";
import type { AppInfo } from "../utils/native";

interface Props {
  line: number;
  column: number;
  info: AppInfo | null;
  status: string;
  language: ClangdSnapshot;
  onLanguageClick: () => void;
}

const PHASE_LABEL: Record<Phase, string> = {
  unsupported: "clangd: desktop only",
  checking: "clangd: checking…",
  unavailable: "clangd: not found",
  starting: "clangd: starting…",
  ready: "clangd",
  crashed: "clangd: stopped",
  error: "clangd: error",
};

export function StatusBar({ line, column, info, status, language, onLanguageClick }: Props) {
  const bad = language.phase === "unavailable" || language.phase === "crashed" || language.phase === "error";
  return (
    <div className="statusbar">
      <span>{status}</span>
      <span className="toolbar-spacer" />
      {language.phase === "ready" && (language.errors > 0 || language.warnings > 0) && (
        <span title="Errors and warnings from clangd">✖ {language.errors} · ⚠ {language.warnings}</span>
      )}
      <button
        className={`status-lang${bad ? " status-lang-bad" : ""}`}
        onClick={onLanguageClick}
        title={language.message ?? language.version ?? "clangd settings"}
      >
        {PHASE_LABEL[language.phase]}
      </button>
      <span>Ln {line}, Col {column}</span>
      <span>Spaces: 4</span>
      <span>UTF-8</span>
      <span>C++</span>
      {info && <span>{info.name} v{info.version}</span>}
    </div>
  );
}
