import { useEffect, useRef } from "react";

/** A narrow drag target with native slider semantics and keyboard controls. */
export function ColumnResize({ label, width, min, max, onResize, onReset }: {
  label: string; width: number; min: number; max: number;
  onResize: (width: number) => void; onReset: () => void;
}) {
  const cleanup = useRef<(() => void) | null>(null);
  useEffect(() => () => cleanup.current?.(), []);
  const clamp = (value: number) => Math.round(Math.min(max, Math.max(min, value)));
  return <input
    type="range" min={min} max={max} step={1} value={clamp(width)}
    aria-label={`${label} column width`} aria-valuetext={`${Math.round(width)} pixels`}
    title="Drag to resize · Arrow keys to adjust · Double-click to reset"
    className="absolute right-0 top-0 h-full w-[7px] z-10 cursor-col-resize appearance-none bg-transparent hover:bg-accent/40 focus-visible:bg-accent/40 outline-none touch-none select-none [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-px [&::-webkit-slider-thumb]:h-4 [&::-webkit-slider-thumb]:bg-line"
    onChange={event => onResize(Number(event.target.value))}
    onMouseDown={event => event.preventDefault()}
    onDoubleClick={event => { event.preventDefault(); onReset(); }}
    onKeyDown={event => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      onResize(clamp(width + (event.key === "ArrowLeft" ? -1 : 1) * (event.shiftKey ? 50 : 10)));
    }}
    onPointerDown={event => {
      if (event.button !== 0) return;
      event.preventDefault(); event.currentTarget.focus();
      cleanup.current?.();
      const x = event.clientX, pointer = event.pointerId;
      const move = (next: PointerEvent) => {
        if (next.pointerId === pointer) onResize(clamp(width + next.clientX - x));
      };
      const end = (next?: PointerEvent | FocusEvent) => {
        if (next && "pointerId" in next && next.pointerId !== pointer) return;
        document.removeEventListener("pointermove", move);
        document.removeEventListener("pointerup", end);
        document.removeEventListener("pointercancel", end);
        window.removeEventListener("blur", end);
        cleanup.current = null;
      };
      cleanup.current = end;
      document.addEventListener("pointermove", move);
      document.addEventListener("pointerup", end);
      document.addEventListener("pointercancel", end);
      window.addEventListener("blur", end);
    }}
  />;
}
