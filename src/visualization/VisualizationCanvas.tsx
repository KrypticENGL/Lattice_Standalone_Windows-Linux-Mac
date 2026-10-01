import { useEffect, useMemo, useRef, useState } from "react";
import type { Recording } from "../runtime/useRecording";
import { layoutGraph, metricsFor, type BoxLayout, type EdgeLayout, type Layout, type Metrics } from "./layout";

/** Width of one character of the monospace UI font, measured once the font is available. */
function useCharWidth(): number {
  const [w, setW] = useState(7);
  useEffect(() => {
    const measure = () => {
      const font = getComputedStyle(document.documentElement).getPropertyValue("--font-mono") || "monospace";
      const ctx = document.createElement("canvas").getContext("2d");
      if (!ctx) return;
      ctx.font = `12px ${font}`;
      const probe = "MMMMMMMMMMMMMMMMMMMM";
      const width = ctx.measureText(probe).width / probe.length;
      if (width > 3 && width < 20) setW(width);
    };
    measure();
    void document.fonts?.ready.then(measure);
  }, []);
  return w;
}

interface Props {
  recording: Recording | null;
}

/**
 * The program's runtime state at the current step: the call stack on the left,
 * heap objects to the right, pointers as arrows. It draws whatever the recording
 * contains; nothing here knows what data structure the program is building.
 */
export function VisualizationCanvas({ recording: r }: Props) {
  const charW = useCharWidth();
  const metrics = useMemo(() => metricsFor(charW), [charW]);
  const graph = r?.graph ?? null;
  const layout = useMemo(() => (graph ? layoutGraph(graph, metrics) : null), [graph, metrics]);
  const scroller = useRef<HTMLDivElement | null>(null);

  // Keep the action in view: bring what changed at this step (else the executing
  // frame) on screen. `nearest` leaves the view alone when it is already visible.
  useEffect(() => {
    const root = scroller.current;
    if (!root || !layout) return;
    const target = root.querySelector(".box.changed") ?? root.querySelector(".box.current");
    target?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [layout]);

  if (!r || (!r.available && !r.summary)) {
    return (
      <div className="viz-surface" data-testid="visualization-surface">
        <div className="viz-placeholder">
          Turn on <b>Observe</b> and press Run to see the program's runtime state here.
        </div>
      </div>
    );
  }
  if (r.summary?.skipped) {
    return (
      <div className="viz-surface">
        <div className="viz-placeholder">The run was not observed: {r.summary.skipped}</div>
      </div>
    );
  }
  if (!r.available) {
    return (
      <div className="viz-surface">
        <div className="viz-placeholder">The program ran, but nothing was recorded.</div>
      </div>
    );
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowRight") r.setStep(r.step + 1);
    else if (e.key === "ArrowLeft") r.setStep(r.step - 1);
    else if (e.key === "Home") r.setStep(0);
    else if (e.key === "End") r.setStep(r.total);
    else if (e.key === " ") r.playing ? r.pause() : r.play();
    else return;
    e.preventDefault();
  };

  return (
    <div className="viz" tabIndex={0} onKeyDown={onKeyDown} data-testid="visualization-surface">
      <Controls r={r} />
      {r.summary?.truncated && (
        <div className="viz-note viz-warn">
          Recording stopped after {r.summary.eventLimit.toLocaleString()} events; the program ran on, so later
          states are not shown.
        </div>
      )}
      {graph?.truncatedHere && <div className="viz-note viz-warn">The recording ends at this step.</div>}
      {r.error && <div className="viz-note viz-error">{r.error}</div>}
      <div className="viz-scroll" ref={scroller}>
        {layout && graph ? (
          <Scene layout={layout} metrics={metrics} />
        ) : (
          <div className="viz-placeholder">Loading…</div>
        )}
        {graph && graph.objectsOmitted > 0 && (
          <div className="viz-note">{graph.objectsOmitted} more objects exist but are not drawn.</div>
        )}
        {layout && layout.unplaced > 0 && (
          <div className="viz-note">{layout.unplaced} pointer(s) lead to objects that are not drawn.</div>
        )}
      </div>
    </div>
  );
}

function Controls({ r }: { r: Recording }) {
  const g = r.graph;
  return (
    <div className="viz-controls">
      <button className="btn btn-icon" onClick={() => r.setStep(0)} disabled={r.step === 0} title="Start (Home)" aria-label="Start">
        ⏮
      </button>
      <button className="btn btn-icon" onClick={() => r.setStep(r.step - 1)} disabled={r.step === 0} title="Previous event (←)" aria-label="Previous event">
        ◀
      </button>
      <button
        className="btn btn-icon"
        onClick={() => (r.playing ? r.pause() : r.play())}
        title={r.playing ? "Pause (Space)" : "Play (Space)"}
        aria-label={r.playing ? "Pause" : "Play"}
      >
        {r.playing ? "⏸" : "▷"}
      </button>
      <button className="btn btn-icon" onClick={() => r.setStep(r.step + 1)} disabled={r.step >= r.total} title="Next event (→)" aria-label="Next event">
        ▶
      </button>
      <button className="btn btn-icon" onClick={() => r.setStep(r.total)} disabled={r.step >= r.total} title="End (End)" aria-label="End">
        ⏭
      </button>
      <input
        className="viz-slider"
        type="range"
        min={0}
        max={r.total}
        value={r.step}
        onChange={(e) => r.setStep(Number(e.target.value))}
        aria-label="Position in the run"
      />
      <span className="viz-position">
        {r.step.toLocaleString()} / {r.total.toLocaleString()}
      </span>
      <span className="viz-event" title={g?.event ?? ""}>
        {g?.event ?? (r.step === 0 ? "before the program ran" : "")}
      </span>
    </div>
  );
}

function Scene({ layout, metrics: m }: { layout: Layout; metrics: Metrics }) {
  return (
    <svg
      className="viz-svg"
      width={layout.viewBox.w}
      height={layout.viewBox.h}
      viewBox={`${layout.viewBox.x} ${layout.viewBox.y} ${layout.viewBox.w} ${layout.viewBox.h}`}
      role="img"
      aria-label="Program state"
    >
      <defs>
        {(["", "-dangling", "-changed"] as const).map((k) => (
          <marker key={k} id={`arrow${k}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" className={`arrow-head arrow-head${k}`} />
          </marker>
        ))}
      </defs>
      {layout.boxes.map((b) => (
        <Box key={b.key} b={b} m={m} />
      ))}
      {layout.edges.map((e) => (
        <Edge key={e.id} e={e} />
      ))}
    </svg>
  );
}

function Edge({ e }: { e: EdgeLayout }) {
  const kind = e.dangling ? "-dangling" : e.changed ? "-changed" : "";
  return (
    <path
      d={e.path}
      className={`edge ${e.dangling ? "dangling" : ""} ${e.changed ? "changed" : ""} ${e.reference ? "reference" : ""}`}
      markerEnd={`url(#arrow${kind})`}
    />
  );
}

function Box({ b, m }: { b: BoxLayout; m: Metrics }) {
  const cls = [
    "box",
    `box-${b.kind}`,
    b.state ? `state-${b.state}` : "",
    b.current ? "current" : "",
    b.changed ? "changed" : "",
  ].join(" ");
  return (
    <g className={cls} style={{ transform: `translate(${b.x}px, ${b.y}px)` }}>
      <rect className="box-body" width={b.w} height={b.h} rx={6} />
      <text className="box-title" x={m.padX} y={m.headerH / 2 + 4}>
        {b.title}
      </text>
      {b.subtitle && (
        <text className="box-sub" x={b.w - m.padX} y={m.headerH / 2 + 4} textAnchor="end">
          {b.subtitle}
        </text>
      )}
      <line className="box-rule" x1={0} x2={b.w} y1={m.headerH} y2={m.headerH} />
      {b.rows.map((r, i) => (
        <g key={i} transform={`translate(0, ${m.headerH + r.y})`} className={`row ${r.muted ? "muted" : ""}`}>
          {r.changed && <rect className="row-changed" width={b.w} height={m.rowH} />}
          <text x={m.padX + r.depth * m.indent} y={m.rowH / 2 + 4}>
            <tspan className="lbl">{r.label}</tspan>
            {r.text !== null && (
              <tspan className="val">
                {r.label ? ": " : ""}
                {r.text}
              </tspan>
            )}
          </text>
          {r.pointer && <circle className="ptr-dot" cx={b.w - 8} cy={m.rowH / 2} r={4} />}
          {r.ty && <title>{r.ty}</title>}
        </g>
      ))}
    </g>
  );
}
