import type { KeyboardEvent, ReactNode } from "react";
import type {
  FrameContext,
  InspectorModel,
  InspectorRow,
  Link,
  MissingContext,
  ObjectContext,
  ObjectSummary,
  Relationship,
  VariableContext,
  VariableSummary,
} from "./inspector";
import type { SelectedRuntimeEntity } from "./selection";

interface Props {
  model: InspectorModel;
  /** The snapshot has not caught up with the selection yet (an object being fetched). */
  loading: boolean;
  onDrill: (entity: SelectedRuntimeEntity) => void;
  onGoto: (index: number) => void;
  onClose: () => void;
}

/**
 * The inspector: one panel, docked beside the canvas, whose contents follow the user's
 * drill-down (frame, variable, object) and the timeline. It only renders an
 * `InspectorModel`; everything it shows was decided in `buildInspector`.
 */
export function InspectorPanel({ model, loading, onDrill, onGoto, onClose }: Props) {
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      onClose();
      e.stopPropagation();
    } else if (e.key === " " || e.key === "Enter") {
      // Space/Enter activate the focused button here; they must not also play/pause the timeline.
      e.stopPropagation();
    }
  };
  const body = model.body;
  return (
    <aside className="inspector" aria-label="Inspector" data-testid="inspector" onKeyDown={onKeyDown}>
      <header className="insp-head">
        <nav className="insp-crumbs" aria-label="Selection trail">
          {model.crumbs.map((c, i) => {
            const last = i === model.crumbs.length - 1;
            return (
              <span key={i} className="insp-crumb-wrap">
                {i > 0 && <span className="insp-sep" aria-hidden="true">›</span>}
                {last ? (
                  <span className={`insp-crumb current ${c.missing ? "missing" : ""}`} aria-current="true">{c.label}</span>
                ) : (
                  <button className={`insp-crumb ${c.missing ? "missing" : ""}`} onClick={() => onGoto(i)}>{c.label}</button>
                )}
              </span>
            );
          })}
        </nav>
        <button className="btn btn-icon insp-close" onClick={onClose} title="Close (Esc)" aria-label="Close inspector">✕</button>
      </header>
      <div className="insp-sync" data-testid="inspector-step">
        at event {model.step.toLocaleString()} / {model.total.toLocaleString()}
      </div>
      <div className="insp-body">
        {loading && body.kind === "missing" ? (
          <p className="insp-note">Loading…</p>
        ) : body.kind === "frame" ? (
          <Frame c={body} onDrill={onDrill} />
        ) : body.kind === "variable" ? (
          <Variable c={body} onDrill={onDrill} />
        ) : body.kind === "object" ? (
          <ObjectBody c={body} onDrill={onDrill} />
        ) : (
          <Missing c={body} />
        )}
      </div>
    </aside>
  );
}

// ---- pieces ------------------------------------------------------------------------

function Field({ label, children, testId }: { label: string; children: ReactNode; testId?: string }) {
  return (
    <div className="insp-field" data-testid={testId}>
      <span className="insp-k">{label}</span>
      <span className="insp-v">{children}</span>
    </div>
  );
}

function Section({ title, count, children }: { title: string; count?: number; children: ReactNode }) {
  return (
    <section className="insp-section">
      <h4>
        {title}
        {count !== undefined && <span className="insp-count">{count}</span>}
      </h4>
      {children}
    </section>
  );
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="insp-note">{children}</p>;
}

function LinkButton({ link, onDrill }: { link: Link; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <button
      className={`insp-link ${link.dangling ? "dangling" : ""} ${link.reference ? "reference" : ""}`}
      onClick={() => link.entity && onDrill(link.entity)}
      title={link.dangling ? "The object this points at has been freed" : "Inspect"}
    >
      → {link.label}
      {link.dangling && <span className="insp-tag"> freed</span>}
    </button>
  );
}

function Rows({ rows, onDrill, highlight = "" }: { rows: InspectorRow[]; onDrill: (e: SelectedRuntimeEntity) => void; highlight?: string }) {
  if (rows.length === 0) return <Empty>No contents.</Empty>;
  return (
    <ul className="insp-rows">
      {rows.map((r, i) => (
        <li
          key={i}
          className={`insp-row ${r.muted ? "muted" : ""} ${highlight !== "" && r.path === highlight ? "highlight" : ""}`}
          style={{ paddingLeft: 8 + r.depth * 14 }}
          title={r.ty || undefined}
        >
          <span className="insp-lbl">{r.label}</span>
          {r.link ? <LinkButton link={r.link} onDrill={onDrill} /> : r.text !== null && <span className="insp-val">{r.text}</span>}
        </li>
      ))}
    </ul>
  );
}

function VariableList({ items, onDrill }: { items: VariableSummary[]; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <ul className="insp-list">
      {items.map((v, i) => (
        <li key={i} className="insp-item">
          <button className="insp-name" onClick={() => onDrill(v.entity)} title={`Inspect ${v.name}`}>{v.name}</button>
          <span className="insp-ty">{v.ty}</span>
          {v.link ? <LinkButton link={v.link} onDrill={onDrill} /> : <span className="insp-val">{v.summary}</span>}
        </li>
      ))}
    </ul>
  );
}

function ObjectList({ items, onDrill }: { items: ObjectSummary[]; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <ul className="insp-list">
      {items.map((o) => (
        <li key={o.id} className={`insp-item state-${o.state}`}>
          <button className="insp-name" onClick={() => onDrill(o.entity)} title="Inspect">
            {o.ty} #{o.id}
          </button>
          {o.state !== "alive" && <span className="insp-tag">{o.state === "destroyed" ? "freed" : o.state}</span>}
          {o.variable && <span className="insp-ty">variable {o.variable}</span>}
          <span className="insp-val">{o.summary}</span>
        </li>
      ))}
    </ul>
  );
}

function Relationships({ items, onDrill }: { items: Relationship[]; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <ul className="insp-list">
      {items.map((r, i) => (
        <li key={i} className="insp-item">
          <span className="insp-from">{r.from}</span>
          <button className={`insp-link ${r.dangling ? "dangling" : ""}`} onClick={() => onDrill(r.entity)}>
            → {r.to}
            {r.dangling && <span className="insp-tag"> freed</span>}
          </button>
        </li>
      ))}
    </ul>
  );
}

// ---- bodies ------------------------------------------------------------------------

function Frame({ c, onDrill }: { c: FrameContext; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <>
      <h3 className="insp-title">
        {c.function}()
        {c.isTop && <span className="insp-badge">executing</span>}
      </h3>
      <Field label="Function" testId="insp-function">{c.function}()</Field>
      <Field label="Source" testId="insp-source">{c.source ?? "unknown"}</Field>
      <Field label="Stack frame" testId="insp-frame">#{c.depth}</Field>
      {c.caller && (
        <Field label="Called from">
          <button className="insp-link" onClick={() => onDrill(c.caller!.entity)}>
            {c.caller.function}()
          </button>
        </Field>
      )}
      <Section title="Parameters" count={c.parameters.length}>
        {c.parameters.length === 0 ? <Empty>None.</Empty> : <VariableList items={c.parameters} onDrill={onDrill} />}
      </Section>
      <Section title="Local variables" count={c.locals.length}>
        {c.locals.length === 0 ? <Empty>None yet.</Empty> : <VariableList items={c.locals} onDrill={onDrill} />}
      </Section>
      <Section title="Relevant runtime objects" count={c.objects.length}>
        {c.objects.length === 0 ? <Empty>No variable of this frame leads to an object.</Empty> : <ObjectList items={c.objects} onDrill={onDrill} />}
      </Section>
      <Section title="Relationships" count={c.relationships.length}>
        {c.relationships.length === 0 ? <Empty>None.</Empty> : <Relationships items={c.relationships} onDrill={onDrill} />}
      </Section>
    </>
  );
}

function Variable({ c, onDrill }: { c: VariableContext; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <>
      <h3 className="insp-title">
        {c.name}
        <span className="insp-badge">{c.variableKind === "staticLocal" ? "static" : c.variableKind}</span>
      </h3>
      <Field label="Type" testId="insp-type">{c.ty}</Field>
      <Field label="Value" testId="insp-value">{c.summary}</Field>
      {c.frame ? (
        <Field label="In">
          <button className="insp-link" onClick={() => onDrill(c.frame!.entity)}>{c.frame.function}()</button>
        </Field>
      ) : (
        <Field label="In">global scope</Field>
      )}
      <Section title="Contents">
        <Rows rows={c.rows} onDrill={onDrill} />
      </Section>
      {c.targets.map((t) => (
        <Section key={t.id} title={`Points to ${t.ty} #${t.id}`}>
          <Field label="Status">{t.status}</Field>
          <Rows rows={t.rows} onDrill={onDrill} />
          <button className="insp-more" onClick={() => onDrill({ kind: "object", object: t.id, path: "" })}>Inspect {t.ty} #{t.id} →</button>
        </Section>
      ))}
    </>
  );
}

function ObjectBody({ c, onDrill }: { c: ObjectContext; onDrill: (e: SelectedRuntimeEntity) => void }) {
  return (
    <>
      <h3 className="insp-title">
        {c.ty} #{c.id}
        {!c.alive && <span className="insp-badge freed">gone</span>}
      </h3>
      <Field label="Type" testId="insp-type">{c.ty}</Field>
      <Field label="Status" testId="insp-status">
        <span className={c.alive ? "" : "insp-gone"}>{c.status}</span>
      </Field>
      <Field label="Storage">{c.storage}</Field>
      {c.address && <Field label="Address">{c.address}</Field>}
      {c.lifetime && (
        <Field label="Created">
          event #{c.lifetime.allocatedStep}
          {c.lifetime.originLine !== null && <span className="insp-ty"> line {c.lifetime.originLine}</span>}
        </Field>
      )}
      {c.variable && <Field label="Storage of">variable {c.variable}</Field>}
      {!c.alive && <p className="insp-note insp-gone">Last known contents (this object no longer exists):</p>}
      <Section title="Fields" count={c.rows.length}>
        <Rows rows={c.rows} onDrill={onDrill} highlight={c.highlight} />
      </Section>
      <Section title="Referenced by" count={c.referencedBy.length}>
        {c.referencedBy.length === 0 ? <Empty>Nothing points here.</Empty> : <Relationships items={c.referencedBy} onDrill={onDrill} />}
      </Section>
    </>
  );
}

function Missing({ c }: { c: MissingContext }) {
  return (
    <>
      <h3 className="insp-title">{c.title}</h3>
      <p className="insp-note insp-gone" data-testid="insp-missing">{c.reason}</p>
    </>
  );
}
