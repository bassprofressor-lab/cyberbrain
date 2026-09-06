import { type RefObject, useEffect, useRef, useState } from "react";

/**
 * Width of an element in CSS pixels, tracked with a ResizeObserver.
 *
 * Charts on this page draw at 1:1 with the screen: the viewBox width is the measured width and
 * the height is a constant. An SVG left to `w-full h-auto` scales its whole drawing with the
 * window instead — the 720x190 chart rendered 303 px tall at a 1440 px window and 430 px at
 * 1920, with the 10-unit axis labels coming out at 16 and 23 px, larger than the panel heading
 * above them. A fixed height keeps the chart the same size on every monitor.
 */
export function useWidth<T extends HTMLElement>(fallback: number): [RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(fallback);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const w = Math.round(el.getBoundingClientRect().width);
      if (w > 0) setWidth(w);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}
