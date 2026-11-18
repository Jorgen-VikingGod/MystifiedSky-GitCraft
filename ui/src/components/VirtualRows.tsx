import { useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";

/** Measured heights keep wrapping accurate while DOM work follows viewport size. */
export function VirtualRows({ count, rowHeight, render, className = "", label }: { count: number; rowHeight: number; render: (index: number, firstVisible: boolean) => ReactNode; className?: string; label: string }) {
  const root = useRef<HTMLDivElement>(null);
  const [sizes, setSizes] = useState<Map<number, number>>(new Map());
  const [view, setView] = useState({ top: 0, height: 600 });
  const offsets = useMemo(() => {
    const values = [0];
    for (let i = 0; i < count; i++) values.push(values[i] + (sizes.get(i) ?? rowHeight));
    return values;
  }, [count, sizes, rowHeight]);
  const find = (top: number) => {
    let lo = 0, hi = count;
    while (lo < hi) { const mid = (lo + hi) >>> 1; if (offsets[mid + 1] <= top) lo = mid + 1; else hi = mid; }
    return lo;
  };
  const start = Math.max(0, Math.min(count - 1, find(view.top)) - 8);
  const end = Math.min(count, find(view.top + view.height) + 9);
  useLayoutEffect(() => {
    const element = root.current!;
    const observer = new ResizeObserver((entries) => {
      setSizes((old) => {
        let next = old;
        for (const entry of entries) {
          const value = (entry.target as HTMLElement).dataset.virtualRow;
          if (value === undefined) continue;
          const index = Number(value);
          if (!Number.isFinite(index)) continue;
          const height = entry.borderBoxSize[0]?.blockSize ?? entry.target.getBoundingClientRect().height;
          if (height > 0 && old.get(index) !== height) { if (next === old) next = new Map(old); next.set(index, height); }
        }
        return next;
      });
      setView((old) => old.height === element.clientHeight ? old : { ...old, height: element.clientHeight });
    });
    observer.observe(element);
    element.querySelectorAll<HTMLElement>("[data-virtual-row]").forEach((row) => observer.observe(row));
    return () => observer.disconnect();
  }, [start, end, count, render]);
  const focus = (index: number) => {
    const element = root.current!;
    element.scrollTop = offsets[index]; setView({ top: offsets[index], height: element.clientHeight });
    requestAnimationFrame(() => (element.querySelector<HTMLElement>(`[data-virtual-row="${index}"] button`) ?? element.querySelector<HTMLElement>(`[data-virtual-row="${index}"]`))?.focus());
  };
  return <div ref={root} role="list" aria-label={label} className={`overflow-auto ${className}`} onScroll={(event) => setView({ top: event.currentTarget.scrollTop, height: event.currentTarget.clientHeight })} onKeyDown={(event) => {
    const row = (event.target as HTMLElement).closest<HTMLElement>("[data-virtual-row]");
    if (!row) return;
    const index = Number(row.dataset.virtualRow);
    let next = event.key === "ArrowDown" ? index + 1 : event.key === "ArrowUp" ? index - 1 : event.key === "Home" ? 0 : event.key === "End" ? count - 1 : null;
    if (event.key === "Tab") {
      const buttons = [...row.querySelectorAll("button")];
      if (!event.shiftKey && index === end - 1 && event.target === buttons.at(-1)) next = index + 1;
      if (event.shiftKey && index === start && event.target === buttons[0]) next = index - 1;
    }
    if (next !== null && next >= 0 && next < count) { event.preventDefault(); focus(next); }
  }}>
    <div style={{ height: offsets[count], position: "relative", minWidth: "100%" }}>
      <div style={{ position: "absolute", top: offsets[start], width: "100%" }}>
        {Array.from({ length: Math.max(0, end - start) }, (_, offset) => <div key={start + offset} data-virtual-row={start + offset} role="listitem" aria-posinset={start + offset + 1} aria-setsize={count} tabIndex={-1}>{render(start + offset, start + offset === find(view.top))}</div>)}
      </div>
    </div>
  </div>;
}
