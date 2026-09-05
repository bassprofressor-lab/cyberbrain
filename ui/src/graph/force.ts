/**
 * A small force layout. O(n²) repulsion is fine for the few hundred nodes a store has,
 * and it needs no library. Rings are laid out as rings: each node is pulled towards a
 * radius proportional to its ring, so the picture matches the mental model (r0 in the
 * middle, r4 outside, intent-only links beyond).
 */
import type { Ring } from "@/api/client";

export interface SimNode {
  id: string;
  name: string;
  ring: Ring;
  /** 5 = dangling target (intent). */
  layer: number;
  kind: string;
  degree: number;
  dangling: boolean;
  x: number;
  y: number;
  vx: number;
  vy: number;
  fixed: boolean;
}

export interface SimEdge {
  s: number;
  t: number;
  intent: boolean;
}

export class Simulation {
  nodes: SimNode[] = [];
  edges: SimEdge[] = [];
  alpha = 1;
  alphaMin = 0.003;
  alphaDecay = 0.024;
  radiusStep = 105;
  radiusBase = 50;

  constructor(nodes: SimNode[], edges: SimEdge[]) {
    this.nodes = nodes;
    this.edges = edges;
    // Seed on the ring circle with a deterministic angle so the first frame is not a blob.
    const perLayer = new Map<number, number>();
    for (const n of nodes) perLayer.set(n.layer, (perLayer.get(n.layer) ?? 0) + 1);
    const seen = new Map<number, number>();
    for (const n of nodes) {
      if (n.fixed) continue;
      const i = seen.get(n.layer) ?? 0;
      seen.set(n.layer, i + 1);
      const total = perLayer.get(n.layer) ?? 1;
      const a = (i / total) * Math.PI * 2 + n.layer * 0.7;
      const r = this.radiusBase + n.layer * this.radiusStep;
      n.x = Math.cos(a) * r;
      n.y = Math.sin(a) * r;
      n.vx = 0;
      n.vy = 0;
    }
  }

  reheat(a = 0.5) {
    this.alpha = Math.max(this.alpha, a);
  }

  get running() {
    return this.alpha > this.alphaMin;
  }

  tick() {
    if (!this.running) return;
    const ns = this.nodes;
    const a = this.alpha;
    // Repulsion (Coulomb-ish, capped).
    for (let i = 0; i < ns.length; i++) {
      const p = ns[i] as SimNode;
      for (let j = i + 1; j < ns.length; j++) {
        const q = ns[j] as SimNode;
        let dx = q.x - p.x;
        let dy = q.y - p.y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1) {
          dx = (Math.random() - 0.5) * 2;
          dy = (Math.random() - 0.5) * 2;
          d2 = dx * dx + dy * dy + 1;
        }
        if (d2 > 90_000) continue; // 300px cutoff
        const f = (a * 900) / d2;
        const fx = dx * f;
        const fy = dy * f;
        if (!p.fixed) {
          p.vx -= fx;
          p.vy -= fy;
        }
        if (!q.fixed) {
          q.vx += fx;
          q.vy += fy;
        }
      }
    }
    // Springs.
    for (const e of this.edges) {
      const p = ns[e.s] as SimNode;
      const q = ns[e.t] as SimNode;
      const dx = q.x - p.x;
      const dy = q.y - p.y;
      const d = Math.sqrt(dx * dx + dy * dy) || 1;
      const rest = e.intent ? 70 : 55 + Math.min(40, Math.abs(p.layer - q.layer) * 20);
      const k = (a * 0.06 * (d - rest)) / d;
      const fx = dx * k;
      const fy = dy * k;
      if (!p.fixed) {
        p.vx += fx;
        p.vy += fy;
      }
      if (!q.fixed) {
        q.vx -= fx;
        q.vy -= fy;
      }
    }
    // Radial by ring, plus a weak centring.
    for (const n of ns) {
      if (n.fixed) continue;
      const r = Math.sqrt(n.x * n.x + n.y * n.y) || 1;
      const target = this.radiusBase + n.layer * this.radiusStep;
      const k = (a * 0.08 * (target - r)) / r;
      n.vx += n.x * k;
      n.vy += n.y * k;
    }
    // Integrate with damping.
    for (const n of ns) {
      if (n.fixed) {
        n.vx = 0;
        n.vy = 0;
        continue;
      }
      n.vx *= 0.6;
      n.vy *= 0.6;
      n.x += n.vx;
      n.y += n.vy;
    }
    this.alpha += (0 - this.alpha) * this.alphaDecay;
  }

  /** Run to rest synchronously (used for the first frame so the page never shows the seed). */
  settle(maxTicks = 300) {
    let i = 0;
    while (this.running && i++ < maxTicks) this.tick();
  }
}
