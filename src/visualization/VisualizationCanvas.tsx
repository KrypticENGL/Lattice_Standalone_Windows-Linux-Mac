/**
 * Future 2D rendering surface. The renderer technology (Canvas 2D, SVG,
 * WebGL, ...) has not been chosen; this component only reserves the area and
 * owns the element a renderer will later attach to.
 */
export function VisualizationCanvas() {
  return (
    <div className="viz-surface" data-testid="visualization-surface">
      <div className="viz-placeholder">Runtime visualization will appear here</div>
    </div>
  );
}
