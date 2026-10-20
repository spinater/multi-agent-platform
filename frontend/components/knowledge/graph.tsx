"use client";

/** Knowledge › graph (backend knowledge_graph.rs; design: Knowledge.dc.html): every indexed document, the code files it
 * cites, and the links between them, clustered into domains by the code folder they point at. Built from the index
 * on every load, no LLM. A force layout on a canvas: each domain is a soft colour cloud with its name in capitals;
 * a document is a rounded rectangle outlined in its kind's colour (ringed yellow or red for findings), a code file a
 * diamond (grey, yellow stale, red missing). Drag a node (the layout reheats), drag empty space to pan, wheel to zoom
 * around the cursor, hover to highlight neighbours, click for the side panel. The graph, circle and orbit modes
 * (`layouts.ts`) place every node in layers instead; "free" is the force layout. */

import { useEffect, useMemo, useRef, useState } from "react";

import { api } from "@/lib/api";
import type { KnowledgeGraph } from "@/lib/generated/KnowledgeGraph";
import type { KnowledgeGraphNode } from "@/lib/generated/KnowledgeGraphNode";
import type { KnowledgeKind } from "@/lib/generated/KnowledgeKind";

import { MODES, bounds, drawGuides, mono, place, type LabelSide, type Mode, type Placed } from "./layouts";
import { Panel } from "./panel";
import { KIND_LABEL, KINDS } from "./shared";

export const KIND_COLOR: Record<KnowledgeKind, string> = { card: "#FF7A1A", requirement: "#38bdf8", task: "#4ade80", rulebook: "#F2C14E", doc: "#8b95a5", memory: "#B48CF2" };
const DOMAIN_PAL = ["#F2A65A", "#5EA8F2", "#B48CF2", "#4FD1B5", "#F27A9B", "#C9D45C", "#7DD3FC", "#E6E9EF"];

type SimNode = KnowledgeGraphNode & { x: number; y: number; vx: number; vy: number; fixed: boolean; r: number };
export type Sel = { kind: "node"; id: string } | { kind: "domain"; id: string } | null;

function rgba(hex: string, a: number) {
  const v = parseInt(hex.slice(1), 16);
  return `rgba(${(v >> 16) & 255},${(v >> 8) & 255},${v & 255},${a})`;
}

export function KnowledgeGraphView({ slug, onOpenDoc }: { slug: string; onOpenDoc: (path: string) => void }) {
  const [graph, setGraph] = useState<KnowledgeGraph | null>(null);
  const [error, setError] = useState("");
  const [hidden, setHidden] = useState(false);
  const [showDocs, setShowDocs] = useState(true);
  const [showCode, setShowCode] = useState(true);
  const [labels, setLabels] = useState(true);
  const [q, setQ] = useState("");
  const [kinds, setKinds] = useState<KnowledgeKind[]>([]);
  const [sel, setSel] = useState<Sel>(null);
  const [hover, setHover] = useState<string | null>(null);
  const [seed, setSeed] = useState(0);
  const [mode, setModeState] = useState<Mode>(storedMode);
  const setMode = (m: Mode) => {
    setModeState(m);
    try {
      localStorage.setItem("map.knowledge.mode", m);
    } catch {
      // Storage blocked: the mode lasts for this page only.
    }
  };
  const layered = useRef<Placed | null>(null);
  const modeRef = useRef(mode);
  useEffect(() => {
    modeRef.current = mode;
  }, [mode]);
  const canvas = useRef<HTMLCanvasElement>(null);
  const sim = useRef<{ nodes: SimNode[]; alpha: number; view: { x: number; y: number; k: number } } | null>(null);
  const raf = useRef(0);
  const drawRef = useRef<() => void>(() => {});
  const anchors = useRef<Map<string, [number, number]>>(new Map());

  useEffect(() => {
    let live = true;
    api.knowledgeGraph(slug).then(
      (g) => {
        if (!live) return;
        setGraph(g);
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [slug]);

  const domColor = useMemo(() => {
    const m = new Map<string, string>();
    graph?.domains.forEach((d, i) => m.set(d.id, DOMAIN_PAL[i % DOMAIN_PAL.length]));
    return m;
  }, [graph]);
  const visible = useMemo(() => {
    if (!graph) return new Set<string>();
    return new Set(graph.nodes.filter((n) => (n.kind === "doc" ? showDocs : showCode)).map((n) => n.id));
  }, [graph, showDocs, showCode]);
  const neighbours = useMemo(() => {
    const m = new Map<string, Set<string>>();
    for (const e of graph?.edges ?? []) {
      if (!m.has(e.from)) m.set(e.from, new Set());
      if (!m.has(e.to)) m.set(e.to, new Set());
      m.get(e.from)!.add(e.to);
      m.get(e.to)!.add(e.from);
    }
    return m;
  }, [graph]);
  const matches = (n: KnowledgeGraphNode) => {
    const okQ = !q.trim() || `${n.label} ${n.path}`.toLowerCase().includes(q.trim().toLowerCase());
    const okK = !kinds.length || (n.doc_kind != null && kinds.includes(n.doc_kind));
    return okQ && okK;
  };

  // Layout: domains start on a ring by size; nodes near their domain's centre; then a force simulation.
  useEffect(() => {
    if (!graph) return;
    const doms = graph.domains;
    const centre = new Map<string, [number, number]>();
    doms.forEach((d, i) => {
      const a = i * 2.39996; // golden angle spiral: big domains in the middle
      const r = 230 * Math.sqrt(i);
      centre.set(d.id, [Math.cos(a) * r, Math.sin(a) * r]);
    });
    let s = 1 + seed;
    const rnd = () => ((s = (s * 16807) % 2147483647) / 2147483647) - 0.5;
    const nodes: SimNode[] = graph.nodes.map((n) => {
      const [cx, cy] = centre.get(n.domain) ?? [0, 0];
      return { ...n, x: cx + rnd() * 80, y: cy + rnd() * 80, vx: 0, vy: 0, fixed: false, r: n.kind === "doc" ? 6 + Math.min(8, Math.sqrt(n.degree) * 1.6) : 4 + Math.min(5, Math.sqrt(n.degree)) };
    });
    const prev = sim.current?.view;
    anchors.current = centre;
    sim.current = { nodes, alpha: 1, view: prev ?? { x: 0, y: 0, k: 0.32 } };
  }, [graph, seed]);

  // The layered modes: fixed spots, then fit the view to them.
  useEffect(() => {
    if (!graph || mode === "free") {
      layered.current = null;
      if (sim.current) sim.current.alpha = 1;
      return;
    }
    const p = place(mode, graph, graph.nodes.filter((n) => visible.has(n.id)), (d) => domColor.get(d) ?? "#8b95a5");
    layered.current = p;
    const S = sim.current;
    const c = canvas.current;
    const nb = bounds(p.pos);
    // The graph mode's domain column sits left of every node.
    const b = nb && mode === "graph" ? { ...nb, x0: Math.min(nb.x0, -860), x1: Math.max(nb.x1, 560) } : nb;
    if (S && c && b) {
      const pad = mode === "graph" ? 20 : 160;
      const kw = c.clientWidth / (b.x1 - b.x0 + 2 * pad);
      // The graph mode fits its width and scrolls down by panning; the round ones fit whole.
      const k = Math.max(0.15, Math.min(1.1, mode === "graph" ? kw : Math.min(kw, c.clientHeight / (b.y1 - b.y0 + 2 * pad))));
      // The graph mode starts at the top; the round ones centred.
      S.view = { k, x: -((b.x0 + b.x1) / 2) * k, y: mode === "graph" ? -c.clientHeight / 2 - (b.y0 - 80) * k : -((b.y0 + b.y1) / 2) * k };
      S.alpha = 1;
    }
  }, [graph, mode, visible, domColor, seed]);
  // Simulation + drawing loop.
  useEffect(() => {
    if (!graph || hidden) return;
    const c = canvas.current;
    if (!c) return;
    const byId = new Map<string, SimNode>();
    const step = () => {
      const S = sim.current;
      if (!S) return;
      byId.clear();
      S.nodes.forEach((n) => byId.set(n.id, n));
      const P = layered.current;
      if (P) {
        // Ease every node to its spot; no forces.
        for (const n of S.nodes) {
          const t = P.pos.get(n.id);
          if (!t) continue;
          n.x += (t[0] - n.x) * 0.18;
          n.y += (t[1] - n.y) * 0.18;
        }
      } else if (S.alpha > 0.01) {
        const nodes = S.nodes.filter((n) => visible.has(n.id));
        // Repulsion on a grid (only nearby cells).
        const cell = 70;
        const grid = new Map<string, SimNode[]>();
        for (const n of nodes) {
          const k = `${Math.floor(n.x / cell)},${Math.floor(n.y / cell)}`;
          (grid.get(k) ?? grid.set(k, []).get(k)!).push(n);
        }
        for (const n of nodes) {
          const gx = Math.floor(n.x / cell);
          const gy = Math.floor(n.y / cell);
          for (let dx = -1; dx <= 1; dx++)
            for (let dy = -1; dy <= 1; dy++)
              for (const m of grid.get(`${gx + dx},${gy + dy}`) ?? []) {
                if (m === n) continue;
                let x = n.x - m.x;
                let y = n.y - m.y;
                let d2 = x * x + y * y;
                if (d2 < 0.01) {
                  x = 0.1;
                  y = 0.1;
                  d2 = 0.02;
                }
                if (d2 > cell * cell) continue;
                const f = (S.alpha * (n.domain === m.domain ? 260 : 900)) / d2;
                n.vx += x * f;
                n.vy += y * f;
              }
        }
        // Springs along edges.
        for (const e of graph.edges) {
          const a = byId.get(e.from);
          const b = byId.get(e.to);
          if (!a || !b || !visible.has(a.id) || !visible.has(b.id)) continue;
          const x = b.x - a.x;
          const y = b.y - a.y;
          const d = Math.sqrt(x * x + y * y) || 1;
          const f = ((d - 46) / d) * 0.04 * S.alpha;
          a.vx += x * f;
          a.vy += y * f;
          b.vx -= x * f;
          b.vy -= y * f;
        }
        // Pull towards the domain's anchor, so domains stay apart.
        for (const n of nodes) {
          const [ax, ay] = anchors.current.get(n.domain) ?? [0, 0];
          n.vx += (ax - n.x) * 0.035 * S.alpha;
          n.vy += (ay - n.y) * 0.035 * S.alpha;
        }
        for (const n of nodes) {
          if (n.fixed) {
            n.vx = n.vy = 0;
            continue;
          }
          n.vx *= 0.6;
          n.vy *= 0.6;
          n.x += Math.max(-20, Math.min(20, n.vx));
          n.y += Math.max(-20, Math.min(20, n.vy));
        }
        S.alpha *= 0.985;
      }
      drawRef.current();
      raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf.current);
  }, [graph, hidden, visible]);

  // Drawing (reads state through a ref so the loop never restarts for a hover).
  useEffect(() => {
    drawRef.current = () => {
      const c = canvas.current;
      const S = sim.current;
      if (!c || !S || !graph) return;
      const dpr = window.devicePixelRatio || 1;
      const W = c.clientWidth;
      const H = c.clientHeight;
      if (c.width !== Math.round(W * dpr) || c.height !== Math.round(H * dpr)) {
        c.width = Math.round(W * dpr);
        c.height = Math.round(H * dpr);
      }
      const ctx = c.getContext("2d");
      if (!ctx) return;
      const { x: ox, y: oy, k } = S.view;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, W, H);
      ctx.translate(W / 2 + ox, H / 2 + oy);
      ctx.scale(k, k);
      const byId = new Map(S.nodes.map((n) => [n.id, n]));
      const focus = hover ?? (sel?.kind === "node" ? sel.id : null);
      const near = focus ? (neighbours.get(focus) ?? new Set()) : null;
      const domFocus = sel?.kind === "domain" ? sel.id : null;
      const filtering = !!q.trim() || kinds.length > 0;
      const dimmed = (n: SimNode) => (near ? n.id !== focus && !near.has(n.id) : domFocus ? n.domain !== domFocus : filtering ? !matches(n) : false);
      const shown = S.nodes.filter((n) => visible.has(n.id));

      const P = layered.current;
      if (P) drawGuides(ctx, P.guides, k, rgba);
      // Domain clouds (free layout only).
      for (const n of P ? [] : shown) {
        const col = domColor.get(n.domain) ?? "#8b95a5";
        const g = ctx.createRadialGradient(n.x, n.y, 0, n.x, n.y, 70);
        g.addColorStop(0, rgba(col, domFocus && n.domain !== domFocus ? 0.03 : 0.1));
        g.addColorStop(1, rgba(col, 0));
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(n.x, n.y, 70, 0, Math.PI * 2);
        ctx.fill();
      }
      // Edges.
      for (const e of graph.edges) {
        const a = byId.get(e.from);
        const b = byId.get(e.to);
        if (!a || !b || !visible.has(a.id) || !visible.has(b.id)) continue;
        const on = !!focus && (e.from === focus || e.to === focus);
        // The round layers have every edge crossing the middle: faint until a node is in focus.
        const base = P && mode !== "graph" ? 0.1 : P ? 0.3 : 0.45;
        ctx.globalAlpha = focus ? (on ? 0.95 : 0.04) : dimmed(a) && dimmed(b) ? 0.05 : base;
        ctx.lineWidth = (on ? 1.6 : 1) / Math.sqrt(k);
        ctx.strokeStyle = e.state === "missing" ? "#F26D6D" : e.state === "stale" ? "#F2C14E" : e.kind === "link" ? "#8b95a5" : "#5d6673";
        ctx.setLineDash(e.state === "missing" ? [5, 4] : e.kind === "mention" ? [4, 4] : e.kind === "source" ? [1.5, 3] : []);
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        if (P && mode === "graph") {
          const mx = (a.x + b.x) / 2;
          ctx.bezierCurveTo(mx, a.y, mx, b.y, b.x, b.y);
        } else if (P && mode === "circle") {
          // Bend along the rings instead of through the middle.
          const aa = Math.atan2(a.y, a.x);
          let d = Math.atan2(b.y, b.x) - aa;
          d = Math.atan2(Math.sin(d), Math.cos(d));
          const rm = (Math.hypot(a.x, a.y) + Math.hypot(b.x, b.y)) / 2;
          ctx.quadraticCurveTo(Math.cos(aa + d / 2) * rm, Math.sin(aa + d / 2) * rm, b.x, b.y);
        } else ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
      // Domain names at their centroids.
      const cent = new Map<string, [number, number, number]>();
      for (const n of shown) {
        const c0 = cent.get(n.domain) ?? [0, 0, 0];
        cent.set(n.domain, [c0[0] + n.x, c0[1] + n.y, c0[2] + 1]);
      }
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      for (const [id, [sx, sy, cnt]] of cent) {
        if (cnt < 3 || P) continue;
        const col = domColor.get(id) ?? "#8b95a5";
        ctx.font = `600 ${12 / k}px ${mono()}`;
        ctx.fillStyle = rgba(col, domFocus && id !== domFocus ? 0.25 : 0.8);
        ctx.fillText(id.replace(/^docs: /, "").toUpperCase(), sx / cnt, sy / cnt);
      }
      // Nodes.
      const placed: [number, number, number, number][] = [];
      for (const n of shown) {
        const dim = dimmed(n);
        ctx.globalAlpha = dim ? 0.18 : 1;
        if (n.kind === "doc") {
          const col = KIND_COLOR[n.doc_kind ?? "doc"];
          const w = n.r * 2.2;
          const h = n.r * 1.5;
          ctx.fillStyle = "#0c0e13";
          ctx.strokeStyle = col;
          ctx.lineWidth = 1.6 / Math.sqrt(k);
          ctx.beginPath();
          ctx.roundRect(n.x - w / 2, n.y - h / 2, w, h, 3);
          ctx.fill();
          ctx.stroke();
          const ring = n.state === "missing" || n.level === "error" ? "#F26D6D" : n.state === "stale" || n.level === "warning" ? "#F2C14E" : null;
          if (ring) {
            ctx.strokeStyle = ring;
            ctx.beginPath();
            ctx.roundRect(n.x - w / 2 - 3, n.y - h / 2 - 3, w + 6, h + 6, 5);
            ctx.stroke();
          }
        } else {
          const col = n.state === "missing" ? "#F26D6D" : n.state === "stale" ? "#F2C14E" : "#8b95a5";
          ctx.fillStyle = col;
          ctx.beginPath();
          ctx.moveTo(n.x, n.y - n.r);
          ctx.lineTo(n.x + n.r, n.y);
          ctx.lineTo(n.x, n.y + n.r);
          ctx.lineTo(n.x - n.r, n.y);
          ctx.closePath();
          ctx.fill();
        }
        if (focus === n.id || (sel?.kind === "node" && sel.id === n.id)) {
          ctx.strokeStyle = "#ffffff";
          ctx.lineWidth = 1.5 / k;
          ctx.beginPath();
          ctx.arc(n.x, n.y, n.r + 6, 0, Math.PI * 2);
          ctx.stroke();
        }
      }
      // Screen room between neighbours on each circle ring: labels only when they fit.
      const roomOnRing = new Map<string, number>();
      if (P && mode === "circle")
        for (const kind of ["doc", "code"]) {
          const list = shown.filter((n) => n.kind === kind);
          const r = list.length ? Math.hypot(list[0].x, list[0].y) : 0;
          roomOnRing.set(kind, list.length ? (2 * Math.PI * r) / list.length : 0);
        }
      // Labels: constant screen size, placed greedily (below, above, right, left) where they do not overlap.
      if (labels) {
        ctx.font = `400 ${10.5 / k}px ${mono()}`;
        const order = [...shown].sort((a, b) => Number(b.id === focus) - Number(a.id === focus) || Number(near?.has(b.id) ?? 0) - Number(near?.has(a.id) ?? 0) || b.degree - a.degree);
        for (const n of order) {
          const key = n.id === focus || !!near?.has(n.id);
          if (dimmed(n) || (!key && (!P || mode === "orbit") && (k < 0.5 || (k < 0.9 && n.degree < 8)))) continue;
          if (!key && mode === "circle" && P && (roomOnRing.get(n.kind) ?? 0) * k < 11) continue;
          const text = n.label.length > 28 ? `${n.label.slice(0, 27)}…` : n.label;
          const tw = ctx.measureText(text).width;
          const th = 12 / k;
          const pad = n.r + 4 / k;
          const s: LabelSide = P?.side.get(n.id) ?? "free";
          if (s === "in" || s === "out") {
            radial(ctx, text, n, s, pad, n.id === focus ? "#ffffff" : "#a3acb9");
            continue;
          }
          const spots: [number, number][] = s === "left" ? [[n.x - pad - tw, n.y - th / 2]] : s === "right" ? [[n.x + pad, n.y - th / 2]] : [
            [n.x - tw / 2, n.y + pad],
            [n.x - tw / 2, n.y - pad - th],
            [n.x + pad, n.y - th / 2],
            [n.x - pad - tw, n.y - th / 2],
          ];
          const spot = spots.find(([x, y]) => !placed.some(([px, py, pw, ph]) => x < px + pw && x + tw > px && y < py + ph && y + th > py));
          if (!spot) continue;
          placed.push([spot[0], spot[1], tw, th]);
          ctx.globalAlpha = 1;
          ctx.fillStyle = n.id === focus ? "#ffffff" : "#a3acb9";
          ctx.textAlign = "left";
          ctx.textBaseline = "top";
          ctx.fillText(text, spot[0], spot[1]);
        }
      }
      ctx.globalAlpha = 1;
    };
  });

  // Pointer: drag nodes, pan, zoom, hover, click.
  const toWorld = (e: { clientX: number; clientY: number }) => {
    const c = canvas.current!;
    const b = c.getBoundingClientRect();
    const { x, y, k } = sim.current!.view;
    return [(e.clientX - b.left - b.width / 2 - x) / k, (e.clientY - b.top - b.height / 2 - y) / k] as const;
  };
  const hit = (e: { clientX: number; clientY: number }) => {
    const S = sim.current;
    if (!S) return null;
    const [wx, wy] = toWorld(e);
    let best: SimNode | null = null;
    let bd = Infinity;
    for (const n of S.nodes) {
      if (!visible.has(n.id)) continue;
      const d = Math.hypot(n.x - wx, n.y - wy);
      if (d < n.r + 6 / S.view.k && d < bd) {
        bd = d;
        best = n;
      }
    }
    return best;
  };
  const drag = useRef<{ node: SimNode | null; hit: SimNode | null; sx: number; sy: number; ox: number; oy: number; moved: boolean } | null>(null);
  const onDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const n = hit(e);
    const v = sim.current!.view;
    const movable = mode === "free" ? n : null;
    drag.current = { node: movable, hit: n, sx: e.clientX, sy: e.clientY, ox: v.x, oy: v.y, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
    if (movable) movable.fixed = true;
  };
  const onMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const d = drag.current;
    const S = sim.current;
    if (!S) return;
    if (!d) {
      const n = hit(e);
      e.currentTarget.style.cursor = n ? "pointer" : "grab";
      if ((n?.id ?? null) !== hover) setHover(n?.id ?? null);
      return;
    }
    if (Math.hypot(e.clientX - d.sx, e.clientY - d.sy) > 3) d.moved = true;
    if (d.node) {
      const [wx, wy] = toWorld(e);
      d.node.x = wx;
      d.node.y = wy;
      S.alpha = Math.max(S.alpha, 0.3);
    } else {
      S.view.x = d.ox + e.clientX - d.sx;
      S.view.y = d.oy + e.clientY - d.sy;
    }
  };
  const onUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.node) d.node.fixed = false;
    if (!d.moved) setSel(d.hit ? { kind: "node", id: d.hit.id } : null);
  };
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const wheel = (e: WheelEvent) => {
      const S = sim.current;
      if (!S) return;
      e.preventDefault();
      // The graph mode is a long list: the wheel scrolls it, and zooms with Ctrl (or a pinch).
      if (layered.current && modeRef.current === "graph" && !e.ctrlKey) {
        S.view.y -= e.deltaY;
        S.view.x -= e.deltaX;
        return;
      }
      const b = c.getBoundingClientRect();
      const mx = e.clientX - b.left - b.width / 2;
      const my = e.clientY - b.top - b.height / 2;
      const k2 = Math.max(0.15, Math.min(4, S.view.k * Math.exp(-e.deltaY * 0.0015)));
      S.view.x = mx - ((mx - S.view.x) * k2) / S.view.k;
      S.view.y = my - ((my - S.view.y) * k2) / S.view.k;
      S.view.k = k2;
    };
    c.addEventListener("wheel", wheel, { passive: false });
    return () => c.removeEventListener("wheel", wheel);
  }, [graph, hidden]);

  const btn = (label: string, on: boolean, fn: () => void) => (
    <button
      key={label}
      onClick={fn}
      className="h-[26px] cursor-pointer rounded-md border px-2.5 font-mono text-[11px] font-medium whitespace-nowrap"
      style={{ borderColor: on ? "var(--accent)" : "var(--line-2)", background: on ? "var(--accent-tint)" : "transparent", color: on ? "var(--accent-soft)" : "var(--text-2)" }}
    >
      {label}
    </button>
  );

  return (
    <section className="flex flex-col gap-3 rounded-xl border border-line bg-surface p-4">
      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-2">
        <span className="text-[15px] leading-none font-semibold">Knowledge graph</span>
        <span className="text-[11.5px] leading-[1.4] text-muted">
          Documents, the code they cite and the links between them, clustered by the code folder they point at.
          {graph && ` ${graph.domains.length} domains · ${graph.nodes.filter((n) => n.kind === "doc").length} of ${graph.total_docs} documents · ${graph.nodes.filter((n) => n.kind === "code").length} code files${graph.truncated ? " · the most useful shown" : ""}.`}
        </span>
        <div className="ml-auto flex flex-wrap gap-1.5">
          {btn("Documents", showDocs, () => setShowDocs(!showDocs))}
          {btn("Code files", showCode, () => setShowCode(!showCode))}
          {btn("Labels", labels, () => setLabels(!labels))}
          <div className="flex overflow-hidden rounded-md border border-line-2">
            {MODES.map((m) => (
              <button
                key={m}
                onClick={() => setMode(m)}
                className="h-[24px] cursor-pointer px-2.5 font-mono text-[11px] font-medium"
                style={{ background: mode === m ? "var(--accent)" : "transparent", color: mode === m ? "var(--accent-text)" : "var(--text-2)" }}
              >
                {m}
              </button>
            ))}
          </div>
          {btn(mode === "free" ? "Re-layout" : "Fit", false, () => setSeed(seed + 1))}
          {btn(hidden ? "Show" : "Hide", hidden, () => setHidden(!hidden))}
        </div>
      </div>
      {error && <span className="font-mono text-[11px] text-bad">{error}</span>}
      {!hidden && (
        <>
          <div className="flex flex-wrap items-center gap-1.5">
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="find a document or file" className="h-[26px] w-[220px] rounded-md border border-line-2 bg-inset px-2 font-mono text-[11px] text-text outline-none focus:border-accent" />
            {KINDS.filter((k) => k !== "memory").map((k) => {
              const on = kinds.includes(k);
              return (
                <button
                  key={k}
                  onClick={() => setKinds(on ? kinds.filter((x) => x !== k) : [...kinds, k])}
                  className="flex h-[26px] cursor-pointer items-center gap-1.5 rounded-md border px-2 font-mono text-[10.5px]"
                  style={{ borderColor: on ? KIND_COLOR[k] : "var(--line-2)", color: on ? "var(--text)" : "var(--text-2)" }}
                >
                  <span className="size-2 rounded-[2px] border" style={{ borderColor: KIND_COLOR[k] }} />
                  {KIND_LABEL[k]}
                </button>
              );
            })}
          </div>
          <div className="flex flex-wrap items-stretch gap-3">
            <div className="relative h-[580px] min-w-0 flex-[999_1_520px] overflow-hidden rounded-[10px] border border-line bg-inset max-sm:h-[420px]">
              <canvas ref={canvas} className="absolute inset-0 block size-full touch-none" style={{ cursor: "grab" }} onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerLeave={() => setHover(null)} />
              <div className="pointer-events-none absolute bottom-2.5 left-3 flex max-w-[calc(100%-24px)] flex-wrap gap-x-3 gap-y-1 font-mono text-[10.5px] leading-[1.4] text-muted">
                <span className="flex items-center gap-1.5">
                  <span className="h-2 w-3 rounded-[2px] border" style={{ borderColor: "#FF7A1A" }} />
                  document (kind colour)
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="size-2 rotate-45 bg-[#8b95a5]" />
                  code file
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="size-2 rotate-45 bg-[#F2C14E]" />
                  stale
                </span>
                <span className="flex items-center gap-1.5">
                  <span className="size-2 rotate-45 bg-[#F26D6D]" />
                  missing
                </span>
                <span>· · cites (sources) · - - mentions · ── links</span>
              </div>
              <span className="pointer-events-none absolute top-2.5 right-3 font-mono text-[10.5px] text-muted/70">{mode === "free" ? "drag a node · " : ""}drag to pan · {mode === "graph" ? "wheel to scroll · ctrl+wheel to zoom" : "wheel to zoom"} · click to inspect</span>
              {!graph && !error && <span className="absolute inset-0 flex items-center justify-center font-mono text-[12px] text-muted">building the graph…</span>}
            </div>
            <Panel graph={graph} sel={sel} setSel={setSel} domColor={domColor} onOpenDoc={onOpenDoc} />
          </div>
        </>
      )}
    </section>
  );
}

/** The mode picked last (per browser); the layered graph first. */
function storedMode(): Mode {
  try {
    const m = localStorage.getItem("map.knowledge.mode");
    if ((MODES as readonly string[]).includes(m ?? "")) return m as Mode;
  } catch {
    // Storage blocked: the default.
  }
  return "graph";
}

/** A label along the radius from the middle: outward past an outer-ring node, inward from an inner-ring one. */
function radial(ctx: CanvasRenderingContext2D, text: string, n: { x: number; y: number }, side: "in" | "out", pad: number, color: string) {
  const ang = Math.atan2(n.y, n.x);
  const left = Math.cos(ang) < 0;
  const outward = side === "out";
  ctx.save();
  ctx.translate(n.x, n.y);
  ctx.rotate(left ? ang + Math.PI : ang);
  ctx.textBaseline = "middle";
  ctx.textAlign = outward === left ? "right" : "left";
  ctx.fillStyle = color;
  ctx.globalAlpha = 1;
  ctx.fillText(text, outward === left ? -pad : pad, 0);
  ctx.restore();
}
