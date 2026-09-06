import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type Graph, type Ring } from "@/api/client";
import { RingBadge, RingGlyph } from "@/components/RingBadge";
import { ErrorBanner, Kbd, Loading, Pill } from "@/components/ui";
import { useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { href, navigate } from "@/lib/router";
import { useAsync } from "@/lib/useAsync";
import { Simulation, type SimEdge, type SimNode } from "@/graph/force";

interface View {
  x: number;
  y: number;
  k: number;
}

const LABEL_MIN_DEGREE = 6;

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function buildSim(g: Graph, rings: Set<Ring>, showDangling: boolean, hideIsolated: boolean): Simulation {
  const degree = new Map<string, number>();
  for (const e of g.edges) {
    degree.set(e.from, (degree.get(e.from) ?? 0) + 1);
    degree.set(e.to, (degree.get(e.to) ?? 0) + 1);
  }
  if (showDangling) for (const d of g.dangling) degree.set(d.from, (degree.get(d.from) ?? 0) + 1);
  const keep = g.nodes.filter((n) => rings.has(n.ring) && (!hideIsolated || (degree.get(n.id) ?? 0) > 0));
  const index = new Map<string, number>();
  const nodes: SimNode[] = keep.map((n, i) => {
    index.set(n.id, i);
    return { id: n.id, name: n.name, ring: n.ring, layer: n.ring, kind: n.kind, degree: degree.get(n.id) ?? 0, dangling: false, x: 0, y: 0, vx: 0, vy: 0, fixed: false };
  });
  const edges: SimEdge[] = [];
  for (const e of g.edges) {
    const s = index.get(e.from);
    const t = index.get(e.to);
    if (s !== undefined && t !== undefined) edges.push({ s, t, intent: false });
  }
  if (showDangling) {
    const intentIndex = new Map<string, number>();
    for (const d of g.dangling) {
      const s = index.get(d.from);
      if (s === undefined) continue;
      let t = intentIndex.get(d.to_name);
      if (t === undefined) {
        t = nodes.length;
        intentIndex.set(d.to_name, t);
        nodes.push({ id: `intent:${d.to_name}`, name: d.to_name, ring: 4, layer: 5, kind: "intent", degree: 0, dangling: true, x: 0, y: 0, vx: 0, vy: 0, fixed: false });
      }
      (nodes[t] as SimNode).degree++;
      edges.push({ s, t, intent: true });
    }
  }
  return new Simulation(nodes, edges);
}

export function GraphScreen() {
  const g = useAsync(() => api.graph(), []);
  const [rings, setRings] = useState<Set<Ring>>(new Set([0, 1, 2, 3, 4]));
  const [showDangling, setShowDangling] = useState(true);
  const [hideIsolated, setHideIsolated] = useState(false);
  const t = useT();
  const [labels, setLabels] = useState<"auto" | "all" | "none">("auto");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [hover, setHover] = useState<{ node: SimNode; px: number; py: number } | null>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const simRef = useRef<Simulation | null>(null);
  const viewRef = useRef<View>({ x: 0, y: 0, k: 1 });
  const rafRef = useRef(0);
  const dragRef = useRef<{ node: SimNode | null; sx: number; sy: number; ox: number; oy: number; moved: boolean } | null>(null);

  const sim = useMemo(() => {
    if (!g.data) return null;
    const s = buildSim(g.data, rings, showDangling, hideIsolated);
    s.settle(260);
    return s;
  }, [g.data, rings, showDangling, hideIsolated]);
  simRef.current = sim;

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q || !sim) return null;
    return new Set(sim.nodes.filter((n) => n.name.toLowerCase().includes(q)).map((n) => n.id));
  }, [query, sim]);

  const neighbours = useMemo(() => {
    if (!sim || !selected) return null;
    const set = new Set<string>([selected]);
    const idx = sim.nodes.findIndex((n) => n.id === selected);
    for (const e of sim.edges) {
      if (e.s === idx) set.add((sim.nodes[e.t] as SimNode).id);
      if (e.t === idx) set.add((sim.nodes[e.s] as SimNode).id);
    }
    return set;
  }, [sim, selected]);

  const fit = useCallback(() => {
    const c = canvasRef.current;
    const s = simRef.current;
    if (!c || !s || !s.nodes.length) return;
    let minX = Infinity,
      minY = Infinity,
      maxX = -Infinity,
      maxY = -Infinity;
    for (const n of s.nodes) {
      minX = Math.min(minX, n.x);
      minY = Math.min(minY, n.y);
      maxX = Math.max(maxX, n.x);
      maxY = Math.max(maxY, n.y);
    }
    const w = c.clientWidth,
      h = c.clientHeight;
    const k = Math.min(2, 0.9 * Math.min(w / Math.max(1, maxX - minX + 80), h / Math.max(1, maxY - minY + 80)));
    viewRef.current = { k, x: w / 2 - ((minX + maxX) / 2) * k, y: h / 2 - ((minY + maxY) / 2) * k };
  }, []);

  const draw = useCallback(() => {
    const c = canvasRef.current;
    const s = simRef.current;
    if (!c) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;
    const dpr = window.devicePixelRatio || 1;
    const w = c.clientWidth,
      h = c.clientHeight;
    if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
      c.width = Math.round(w * dpr);
      c.height = Math.round(h * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    if (!s) return;
    const v = viewRef.current;
    const ringColor = [0, 1, 2, 3, 4].map((r) => cssVar(`--ring-${r}`));
    const line = cssVar("--line-strong");
    const fg = cssVar("--fg");
    const faint = cssVar("--fg-faint");
    const bg = cssVar("--bg");
    const focus = cssVar("--focus");

    ctx.save();
    ctx.translate(v.x, v.y);
    ctx.scale(v.k, v.k);

    // Ring guides.
    ctx.lineWidth = 1 / v.k;
    for (let r = 0; r <= 5; r++) {
      ctx.beginPath();
      ctx.arc(0, 0, s.radiusBase + r * s.radiusStep, 0, Math.PI * 2);
      ctx.strokeStyle = line;
      ctx.globalAlpha = r === 5 ? 0.15 : 0.28;
      ctx.setLineDash(r === 5 ? [4 / v.k, 6 / v.k] : []);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    const dim = (id: string) => (matches && !matches.has(id)) || (neighbours && !neighbours.has(id));

    // Edges.
    for (const e of s.edges) {
      const p = s.nodes[e.s] as SimNode;
      const q = s.nodes[e.t] as SimNode;
      const faded = dim(p.id) || dim(q.id);
      ctx.beginPath();
      ctx.moveTo(p.x, p.y);
      ctx.lineTo(q.x, q.y);
      ctx.strokeStyle = e.intent ? faint : line;
      ctx.globalAlpha = faded ? 0.08 : e.intent ? 0.55 : 0.6;
      ctx.lineWidth = (neighbours?.has(p.id) && neighbours?.has(q.id) ? 1.6 : 1) / v.k;
      ctx.setLineDash(e.intent ? [3 / v.k, 3 / v.k] : []);
      ctx.stroke();
    }
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    // Nodes.
    const fontPx = 11 / v.k;
    ctx.font = `500 ${fontPx}px Geist, system-ui, sans-serif`;
    ctx.textBaseline = "middle";
    for (const n of s.nodes) {
      const faded = dim(n.id);
      const r = (n.dangling ? 3.5 : 4 + Math.min(6, Math.sqrt(n.degree) * 1.2)) / Math.sqrt(v.k);
      const col = n.dangling ? faint : (ringColor[n.ring] as string);
      ctx.globalAlpha = faded ? 0.18 : 1;
      ctx.beginPath();
      switch (n.dangling ? 5 : n.ring) {
        case 0:
          ctx.moveTo(n.x, n.y - r * 1.3);
          ctx.lineTo(n.x + r * 1.3, n.y);
          ctx.lineTo(n.x, n.y + r * 1.3);
          ctx.lineTo(n.x - r * 1.3, n.y);
          ctx.closePath();
          ctx.fillStyle = col;
          ctx.fill();
          break;
        case 1:
          ctx.rect(n.x - r, n.y - r, r * 2, r * 2);
          ctx.fillStyle = col;
          ctx.fill();
          break;
        case 2:
          ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
          ctx.fillStyle = col;
          ctx.fill();
          break;
        case 3:
          ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
          ctx.fillStyle = bg;
          ctx.fill();
          ctx.lineWidth = 1.8 / v.k;
          ctx.strokeStyle = col;
          ctx.stroke();
          break;
        default:
          ctx.arc(n.x, n.y, r, 0, Math.PI * 2);
          ctx.fillStyle = bg;
          ctx.fill();
          ctx.lineWidth = 1.4 / v.k;
          ctx.setLineDash([2.5 / v.k, 2 / v.k]);
          ctx.strokeStyle = col;
          ctx.stroke();
          ctx.setLineDash([]);
      }
      if (n.id === selected || n.id === hover?.node.id) {
        ctx.beginPath();
        ctx.arc(n.x, n.y, r + 4 / v.k, 0, Math.PI * 2);
        ctx.lineWidth = 2 / v.k;
        ctx.strokeStyle = focus;
        ctx.stroke();
      }
      const showLabel = labels === "all" || n.id === selected || n.id === hover?.node.id || (matches && matches.has(n.id)) || (neighbours && neighbours.has(n.id)) || (labels === "auto" && (n.degree >= LABEL_MIN_DEGREE || v.k > 1.8 || n.ring <= 1));
      if (showLabel && !faded) {
        ctx.fillStyle = n.dangling ? faint : fg;
        ctx.globalAlpha = n.dangling ? 0.8 : 0.92;
        ctx.fillText(n.dangling ? `${n.name} ?` : n.name, n.x + r + 3 / v.k, n.y);
      }
    }
    ctx.restore();
  }, [matches, neighbours, selected, hover, labels]);

  // `draw` is rebuilt whenever hover, selection or labels change. The loop reads it through
  // a ref so that a repaint never restarts the effect below, which would call fit() and throw
  // away the view the reader panned and zoomed to.
  const drawRef = useRef(draw);

  // Animation loop while the simulation is hot; otherwise draw on demand. Runs once per
  // graph, so fit() happens on new data and never again on its own.
  useEffect(() => {
    let alive = true;
    const loop = () => {
      if (!alive) return;
      const s = simRef.current;
      if (s?.running) {
        s.tick();
        drawRef.current();
        rafRef.current = requestAnimationFrame(loop);
      } else {
        drawRef.current();
      }
    };
    fit();
    loop();
    const ro = new ResizeObserver(() => drawRef.current());
    if (wrapRef.current) ro.observe(wrapRef.current);
    const mo = new MutationObserver(() => drawRef.current());
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => {
      alive = false;
      cancelAnimationFrame(rafRef.current);
      ro.disconnect();
      mo.disconnect();
    };
  }, [sim, fit]);

  // Repaint on a visual change, leaving the view untouched. While the simulation is hot the
  // loop is already painting every frame.
  useEffect(() => {
    drawRef.current = draw;
    if (!simRef.current?.running) draw();
  }, [draw]);

  const kick = () => {
    cancelAnimationFrame(rafRef.current);
    const loop = () => {
      const s = simRef.current;
      if (s?.running) {
        s.tick();
        draw();
        rafRef.current = requestAnimationFrame(loop);
      } else draw();
    };
    loop();
  };

  const toWorld = (px: number, py: number) => {
    const v = viewRef.current;
    return { x: (px - v.x) / v.k, y: (py - v.y) / v.k };
  };
  const nodeAt = (px: number, py: number): SimNode | null => {
    const s = simRef.current;
    if (!s) return null;
    const { x, y } = toWorld(px, py);
    const rad = 10 / viewRef.current.k;
    let best: SimNode | null = null;
    let bd = rad * rad;
    for (const n of s.nodes) {
      const d = (n.x - x) ** 2 + (n.y - y) ** 2;
      if (d < bd) {
        bd = d;
        best = n;
      }
    }
    return best;
  };
  const local = (e: React.MouseEvent | React.WheelEvent) => {
    const r = canvasRef.current?.getBoundingClientRect();
    return { px: e.clientX - (r?.left ?? 0), py: e.clientY - (r?.top ?? 0) };
  };

  const onWheel = (e: React.WheelEvent) => {
    e.preventDefault();
    const { px, py } = local(e);
    const v = viewRef.current;
    const f = Math.exp(-e.deltaY * 0.0015);
    const k = Math.min(6, Math.max(0.15, v.k * f));
    viewRef.current = { k, x: px - ((px - v.x) / v.k) * k, y: py - ((py - v.y) / v.k) * k };
    draw();
  };
  const onDown = (e: React.MouseEvent) => {
    const { px, py } = local(e);
    const n = nodeAt(px, py);
    dragRef.current = { node: n, sx: px, sy: py, ox: viewRef.current.x, oy: viewRef.current.y, moved: false };
    if (n) n.fixed = true;
  };
  const onMove = (e: React.MouseEvent) => {
    const { px, py } = local(e);
    const d = dragRef.current;
    if (d) {
      const dx = px - d.sx,
        dy = py - d.sy;
      if (Math.abs(dx) + Math.abs(dy) > 2) d.moved = true;
      if (d.node) {
        const w = toWorld(px, py);
        d.node.x = w.x;
        d.node.y = w.y;
        simRef.current?.reheat(0.25);
        kick();
      } else {
        viewRef.current = { ...viewRef.current, x: d.ox + dx, y: d.oy + dy };
        draw();
      }
      return;
    }
    const n = nodeAt(px, py);
    setHover(n ? { node: n, px, py } : null);
  };
  const onUp = (e: React.MouseEvent) => {
    const d = dragRef.current;
    dragRef.current = null;
    if (!d) return;
    if (d.node) d.node.fixed = false;
    if (!d.moved) {
      const { px, py } = local(e);
      const n = nodeAt(px, py);
      setSelected(n ? n.id : null);
    }
  };
  const onDouble = (e: React.MouseEvent) => {
    const { px, py } = local(e);
    const n = nodeAt(px, py);
    if (n && !n.dangling) navigate(href("note", n.name));
  };

  const zoom = (f: number) => {
    const c = canvasRef.current;
    if (!c) return;
    const px = c.clientWidth / 2,
      py = c.clientHeight / 2;
    const v = viewRef.current;
    const k = Math.min(6, Math.max(0.15, v.k * f));
    viewRef.current = { k, x: px - ((px - v.x) / v.k) * k, y: py - ((py - v.y) / v.k) * k };
    draw();
  };

  const selNode = sim?.nodes.find((n) => n.id === selected) ?? null;
  const toggleRing = (r: Ring) =>
    setRings((s) => {
      const x = new Set(s);
      x.has(r) ? x.delete(r) : x.add(r);
      return x;
    });

  useShortcuts(
    "graph",
    [
      { keys: "/", label: t.graph.keys.find, run: () => searchRef.current?.select() },
      { keys: "+", label: t.graph.keys.zoomIn, run: () => zoom(1.25) },
      { keys: "=", label: t.graph.keys.zoomIn, run: () => zoom(1.25), hidden: true },
      { keys: "-", label: t.graph.keys.zoomOut, run: () => zoom(0.8) },
      { keys: "0", label: t.graph.keys.fit, run: () => (fit(), draw()) },
      { keys: "Enter", label: t.graph.keys.open, run: () => {
          const target = selNode ?? (matches && sim ? sim.nodes.find((n) => matches.has(n.id)) : null);
          if (target && !target.dangling) navigate(href("note", target.name));
        }, inInputs: true },
      { keys: "Escape", label: t.graph.keys.clear, run: () => (setSelected(null), setQuery("")), inInputs: true },
      { keys: "l", label: t.graph.keys.labels, run: () => setLabels((l) => (l === "auto" ? "all" : l === "all" ? "none" : "auto")) },
      { keys: "d", label: t.graph.keys.dangling, run: () => setShowDangling((v) => !v) },
    ],
    [selNode, matches, sim, t],
  );

  const counts = useMemo(() => {
    if (!g.data) return null;
    return { nodes: g.data.nodes.length, edges: g.data.edges.length, dangling: g.data.dangling.length, shown: sim?.nodes.filter((n) => !n.dangling).length ?? 0 };
  }, [g.data, sim]);

  return (
    <div className="flex flex-col h-full">
      <div className="px-4 py-2 border-b flex flex-wrap items-center gap-x-4 gap-y-2 text-xs">
        <input ref={searchRef} className="input w-52" placeholder={t.graph.findPlaceholder} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t.graph.findAria} />
        <div className="flex items-center gap-1">
          {([0, 1, 2, 3, 4] as Ring[]).map((r) => (
            <button key={r} className={`btn btn-sm ${rings.has(r) ? "" : "opacity-40"}`} onClick={() => toggleRing(r)} aria-pressed={rings.has(r)} title={t.rings.label[r]}>
              <RingGlyph ring={r} size={9} />
              <span style={{ color: `var(--ring-${r})` }}>r{r}</span>
            </button>
          ))}
        </div>
        <label className="inline-flex items-center gap-1.5 text-fg-muted cursor-pointer">
          <input type="checkbox" checked={showDangling} onChange={(e) => setShowDangling(e.target.checked)} /> {t.graph.danglingAsIntent}
        </label>
        <label className="inline-flex items-center gap-1.5 text-fg-muted cursor-pointer">
          <input type="checkbox" checked={hideIsolated} onChange={(e) => setHideIsolated(e.target.checked)} /> {t.graph.hideIsolated}
        </label>
        <select className="input h-6 text-xs" value={labels} onChange={(e) => setLabels(e.target.value as typeof labels)} aria-label={t.graph.labelsAria}>
          <option value="auto">{t.graph.labelsAuto}</option>
          <option value="all">{t.graph.labelsAll}</option>
          <option value="none">{t.graph.labelsNone}</option>
        </select>
        <span className="ml-auto text-fg-faint tnum">
          {counts ? t.graph.counts(counts) : ""} · <Kbd keys="0" /> {t.graph.fit} <Kbd keys="+" /> <Kbd keys="-" /> {t.graph.zoom}
        </span>
      </div>
      <div ref={wrapRef} className="relative flex-1 min-h-0">
        {g.error ? (
          <div className="p-4">
            <ErrorBanner error={g.error} onRetry={g.reload} />
          </div>
        ) : null}
        {g.loading && !g.data ? <Loading label={t.graph.loading} /> : null}
        <canvas
          ref={canvasRef}
          className="w-full h-full block cursor-grab active:cursor-grabbing"
          onWheel={onWheel}
          onMouseDown={onDown}
          onMouseMove={onMove}
          onMouseUp={onUp}
          onMouseLeave={() => {
            setHover(null);
            if (dragRef.current?.node) dragRef.current.node.fixed = false;
            dragRef.current = null;
          }}
          onDoubleClick={onDouble}
          role="img"
          aria-label={t.graph.canvasAria}
        />
        {hover && !dragRef.current ? (
          <div className="absolute pointer-events-none panel px-2 py-1 text-xs shadow-panel" style={{ left: hover.px + 12, top: hover.py + 12 }}>
            <div className="flex items-center gap-1.5">
              {hover.node.dangling ? <Pill>{t.common.intent}</Pill> : <RingBadge ring={hover.node.ring} />}
              <span className="font-mono">{hover.node.name}</span>
            </div>
            <div className="text-fg-faint mt-0.5">{hover.node.dangling ? t.graph.hoverDangling(hover.node.degree) : t.graph.hoverNode(hover.node.kind, hover.node.degree)}</div>
          </div>
        ) : null}
        {selNode ? (
          <div className="absolute right-3 top-3 panel shadow-panel w-64 p-3 text-sm">
            <div className="flex items-center gap-2">
              {selNode.dangling ? <Pill>{t.common.intent}</Pill> : <RingBadge ring={selNode.ring} showName />}
              <button className="ml-auto text-fg-faint hover:text-fg" onClick={() => setSelected(null)} aria-label={t.graph.clearSelection}>
                ×
              </button>
            </div>
            <div className="mt-1.5 font-mono break-words">{selNode.name}</div>
            <div className="mt-1 text-xs text-fg-muted">{selNode.dangling ? t.graph.selDangling(selNode.degree) : t.graph.selNode(selNode.kind, selNode.degree)}</div>
            {!selNode.dangling ? (
              <div className="mt-2 flex gap-1.5">
                <a className="btn btn-sm btn-primary" href={href("note", selNode.name)}>
                  {t.common.open} <Kbd keys="Enter" className="opacity-70" />
                </a>
                <a className="btn btn-sm" href={href("search", null, { q: selNode.name.replace(/-/g, " ") })}>
                  {t.graph.recall}
                </a>
              </div>
            ) : null}
          </div>
        ) : null}
        <div className="absolute left-3 bottom-3 panel px-2.5 py-1.5 text-2xs text-fg-muted flex items-center gap-3">
          {([0, 1, 2, 3, 4] as Ring[]).map((r) => (
            <span key={r} className="inline-flex items-center gap-1">
              <RingGlyph ring={r} size={9} /> r{r}
            </span>
          ))}
          <span className="inline-flex items-center gap-1">
            <span className="inline-block w-[9px] h-[9px] rounded-full border border-dashed border-fg-faint" /> {t.common.intent}
          </span>
          <span className="text-fg-faint">{t.graph.legendHint}</span>
        </div>
      </div>
    </div>
  );
}
