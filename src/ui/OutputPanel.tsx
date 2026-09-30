import { useState } from "react";

const tabs = ["Output", "Errors", "Runtime"] as const;
type Tab = (typeof tabs)[number];

/** Placeholder tabbed panel. Arrow keys move between tabs. */
export function OutputPanel() {
  const [active, setActive] = useState<Tab>("Output");

  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = tabs.indexOf(active);
    if (e.key === "ArrowRight") setActive(tabs[(i + 1) % tabs.length]);
    if (e.key === "ArrowLeft") setActive(tabs[(i + tabs.length - 1) % tabs.length]);
  };

  return (
    <section className="panel output-panel">
      <div className="tabs" role="tablist" onKeyDown={onKeyDown}>
        {tabs.map((t) => (
          <button key={t} role="tab" aria-selected={t === active}
            tabIndex={t === active ? 0 : -1}
            className={`tab ${t === active ? "active" : ""}`}
            onClick={() => setActive(t)}>
            {t}
          </button>
        ))}
      </div>
      <div className="panel-body output-body" role="tabpanel">
        <span className="muted">{active} — nothing to show yet.</span>
      </div>
    </section>
  );
}
