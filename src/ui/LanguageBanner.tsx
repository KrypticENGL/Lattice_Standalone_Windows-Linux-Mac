import { clangd, type ClangdSnapshot } from "../lsp/controller";

interface Props {
  language: ClangdSnapshot;
  onConfigure: () => void;
}

/** Shown above the editor whenever code intelligence is not running, so failures are never silent. */
export function LanguageBanner({ language, onConfigure }: Props) {
  const { phase, message } = language;
  if (phase !== "unavailable" && phase !== "error" && phase !== "crashed") return null;
  const title =
    phase === "unavailable" ? "Code intelligence is off."
    : phase === "crashed" ? "Code intelligence stopped."
    : "Code intelligence could not start.";
  return (
    <div className="lang-banner" role="alert">
      <div className="lang-banner-text">
        <strong>{title}</strong>
        {message && <span className="lang-banner-detail">{message}</span>}
      </div>
      <button className="btn" onClick={onConfigure}>Configure clangd…</button>
      <button className="btn" onClick={() => clangd.restart()}>Retry</button>
    </div>
  );
}
