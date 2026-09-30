import { useCallback } from "react";

interface Props {
  orientation: "vertical" | "horizontal"; // vertical = the divider line is vertical
  onDrag: (delta: number) => void;
}

/** Pointer-driven divider. Reports movement in px; the parent owns sizes. */
export function Splitter({ orientation, onDrag }: Props) {
  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const el = e.currentTarget;
      el.setPointerCapture(e.pointerId);
      let last = orientation === "vertical" ? e.clientX : e.clientY;
      const move = (ev: PointerEvent) => {
        const cur = orientation === "vertical" ? ev.clientX : ev.clientY;
        onDrag(cur - last);
        last = cur;
      };
      const up = () => {
        el.removeEventListener("pointermove", move);
        el.removeEventListener("pointerup", up);
      };
      el.addEventListener("pointermove", move);
      el.addEventListener("pointerup", up);
    },
    [orientation, onDrag],
  );

  return <div className={`splitter splitter-${orientation}`} role="separator" onPointerDown={onPointerDown} />;
}
