"use client";

import clsx from "clsx";
import { Maximize, Scan, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { LAYERS, areaVar, type Graph, type GraphNode, type Kind } from "./model";
import { TUNING, VIEWS, layout, project3d, type Point, type View } from "./layouts";

const VIEW_LABEL: Record<View, string> = { rings: "Rings", circle: "Circle", areas: "Areas", links: "Links", timeline: "Timeline", orbit: "3D orbit" };
const KINDS: Kind[] = ["project", "domain", "task", "session", "provider"];
const KIND_GLYPH: Record<Kind, string> = { root: "◉", project: "⬡", domain: "●", task: "◆", session: "◎", provider: "▲" };
const LAYER_LABEL: Record<(typeof LAYERS)[number], string> = { core: "", projects: "PROJECTS", corpus: "CORPUS", tasks: "TASKS", sessions: "SESSIONS", providers: "PROVIDERS" };

type Camera = { zoom: number; x: number; y: number };
type Screen = { id: string; x: number; y: number; r: number };

function readColors(): Record<string, string> {
  const css = getComputedStyle(document.documentElement);
  const get = (v: string) => css.getPropertyValue(v).trim();
  const out: Record<string, string> = {};
  for (const v of ["--bg", "--panel", "--line", "--text", "--muted", "--accent", "--area-0", "--area-1", "--area-2", "--area-3", "--area-4", "--area-5", "--area-6", "--area-7", "--area-8"]) out[v] = get(v);
  return out;
}

function shape(ctx: CanvasRenderingContext2D, kind: Kind, x: number, y: number, r: number) {
  ctx.beginPath();
  switch (kind) {
    case "project": {
      for (let i = 0; i < 6; i++) {
        const a = Math.PI / 6 + (i * Math.PI) / 3;
        if (i === 0) ctx.moveTo(x + r * Math.cos(a), y + r * Math.sin(a));
        else ctx.lineTo(x + r * Math.cos(a), y + r * Math.sin(a));
      }
      ctx.closePath();
      break;
    }
    case "task":
      ctx.moveTo(x, y - r);
      ctx.lineTo(x + r, y);
      ctx.lineTo(x, y + r);
      ctx.lineTo(x - r, y);
      ctx.closePath();
      break;
    case "provider":
      ctx.moveTo(x, y - r * 1.15);
      ctx.lineTo(x + r, y + r * 0.75);
      ctx.lineTo(x - r, y + r * 0.75);
      ctx.closePath();
      break;
    default:
      ctx.arc(x, y, r, 0, Math.PI * 2);
  }
}

export function Brain({
  graph,
  selected,
  onSelect,
  actions,
  stats,
}: {
  graph: Graph;
  selected: string | null;
  onSelect: (node: GraphNode | null) => void;
  /** Buttons on the card of the selected node. */
  actions: (node: GraphNode) => ReactNode;
  stats?: ReactNode;
}) {
  const [view, setView] = useState<View>("rings");
  const [names, setNames] = useState(false);
  const [motion, setMotion] = useState(true);
  const [areaFilter, setAreaFilter] = useState<Set<string>>(new Set());
  const [kindFilter, setKindFilter] = useState<Set<Kind>>(new Set());
  const [query, setQuery] = useState("");
  const [hover, setHover] = useState<string | null>(null);

  const root = useRef<HTMLDivElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const positions = useRef(new Map<string, Point>());
  const screen = useRef<Screen[]>([]);
  const camera = useRef<Camera>({ zoom: 1, x: 0, y: 0 });
  const cameraTarget = useRef<Camera>({ zoom: 1, x: 0, y: 0 });
  const angle = useRef(0);
  const drag = useRef<{ x: number; y: number; cx: number; cy: number; moved: boolean } | null>(null);

  const targets = useMemo(() => layout(view, graph), [view, graph]);
  const degree = useMemo(() => {
    const d = new Map<string, number>();
    for (const l of graph.links) {
      d.set(l.s, (d.get(l.s) ?? 0) + 1);
      d.set(l.t, (d.get(l.t) ?? 0) + 1);
    }
    return d;
  }, [graph]);
  const neighbours = useMemo(() => {
    const focus = hover ?? selected;
    if (!focus) return null;
    const set = new Set([focus]);
    for (const l of graph.links) {
      if (l.s === focus) set.add(l.t);
      if (l.t === focus) set.add(l.s);
    }
    return set;
  }, [hover, selected, graph]);
  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    return graph.nodes.filter((n) => `${n.label} ${n.note} ${n.ref}`.toLowerCase().includes(q)).slice(0, 8);
  }, [query, graph]);

  // Everything the draw loop reads lives in one ref, so the loop never restarts on a state change.
  const live = useRef({ graph, targets, degree, neighbours, view, names, motion, areaFilter, kindFilter, selected });
  useEffect(() => {
    live.current = { graph, targets, degree, neighbours, view, names, motion, areaFilter, kindFilter, selected };
  });

  useEffect(() => {
    const el = canvas.current;
    const container = box.current;
    if (!el || !container) return;
    const ctx = el.getContext("2d");
    if (!ctx) return;
    let colors = readColors();
    // Canvas fonts cannot use CSS variables; take the resolved family from the page.
    const family = getComputedStyle(document.body).fontFamily || "ui-monospace, monospace";
    let raf = 0;
    let last = performance.now();
    const theme = window.matchMedia("(prefers-color-scheme: dark)");
    const recolor = () => (colors = readColors());
    theme.addEventListener("change", recolor);

    const resize = () => {
      const dpr = window.devicePixelRatio || 1;
      const { width, height } = container.getBoundingClientRect();
      el.width = Math.max(1, Math.floor(width * dpr));
      el.height = Math.max(1, Math.floor(height * dpr));
      el.style.width = `${width}px`;
      el.style.height = `${height}px`;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    const observer = new ResizeObserver(resize);
    observer.observe(container);
    resize();

    const frame = (now: number) => {
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      const s = live.current;
      const w = el.clientWidth;
      const h = el.clientHeight;
      const cam = camera.current;
      const goal = cameraTarget.current;
      cam.zoom += (goal.zoom - cam.zoom) * TUNING.ease;
      cam.x += (goal.x - cam.x) * TUNING.ease;
      cam.y += (goal.y - cam.y) * TUNING.ease;
      if (s.motion && s.view === "orbit") angle.current += TUNING.orbitSpeed * dt;
      const scale = (Math.min(w, h) / 2) * 0.9 * cam.zoom;
      const cx = w / 2 + cam.x;
      const cy = h / 2 + cam.y;

      // Move every node a step towards its target. The first paint places them directly; later arrivals fly out
      // from the centre.
      const firstPaint = positions.current.size === 0;
      for (const n of s.graph.nodes) {
        const t = s.targets.get(n.id) ?? { x: 0, y: 0, z: 0 };
        const p = positions.current.get(n.id) ?? (firstPaint ? { ...t } : { x: 0, y: 0, z: 0 });
        p.x += (t.x - p.x) * TUNING.ease;
        p.y += (t.y - p.y) * TUNING.ease;
        p.z += (t.z - p.z) * TUNING.ease;
        positions.current.set(n.id, p);
      }

      ctx.clearRect(0, 0, w, h);
      const glow = ctx.createRadialGradient(cx, cy, 0, cx, cy, scale * 1.2);
      glow.addColorStop(0, `${colors["--area-1"]}14`);
      glow.addColorStop(1, "transparent");
      ctx.fillStyle = glow;
      ctx.fillRect(0, 0, w, h);

      // Guides: layer rings, or timeline lanes.
      ctx.lineWidth = 1;
      ctx.font = `10px ${family}`;
      ctx.textAlign = "center";
      if (s.view === "rings") {
        TUNING.ringRadii.forEach((r, i) => {
          if (!r) return;
          ctx.strokeStyle = colors["--line"];
          ctx.setLineDash(i % 2 ? [2, 4] : []);
          ctx.beginPath();
          ctx.arc(cx, cy, r * scale, 0, Math.PI * 2);
          ctx.stroke();
          ctx.setLineDash([]);
          ctx.fillStyle = colors["--muted"];
          ctx.fillText(LAYER_LABEL[LAYERS[i]], cx, cy - r * scale - 6);
        });
      } else if (s.view === "timeline") {
        ctx.textAlign = "left";
        LAYERS.forEach((layer, li) => {
          const y = cy + (-0.85 + (1.7 * li) / (LAYERS.length - 1)) * scale;
          ctx.strokeStyle = colors["--line"];
          ctx.beginPath();
          ctx.moveTo(cx - scale, y);
          ctx.lineTo(cx + scale, y);
          ctx.stroke();
          ctx.fillStyle = colors["--muted"];
          ctx.fillText(LAYER_LABEL[layer] || "CORE", cx - scale, y - 6);
        });
        ctx.strokeStyle = colors["--accent"];
        ctx.beginPath();
        ctx.moveTo(cx + 0.92 * scale, cy - 0.95 * scale);
        ctx.lineTo(cx + 0.92 * scale, cy + 0.95 * scale);
        ctx.stroke();
        ctx.textAlign = "center";
      } else if (s.view === "orbit") {
        ctx.strokeStyle = colors["--line"];
        ctx.beginPath();
        ctx.ellipse(cx, cy + 0.1 * scale, 1.05 * scale, 0.3 * scale, 0, 0, Math.PI * 2);
        ctx.stroke();
      }

      // Screen positions (with idle breathing in flat views, projection in the orbit).
      const pts = new Map<string, { x: number; y: number; depth: number }>();
      for (const n of s.graph.nodes) {
        const p = positions.current.get(n.id)!;
        if (s.view === "orbit") {
          const q = project3d(p, angle.current);
          pts.set(n.id, { x: cx + q.x * scale, y: cy + q.y * scale, depth: q.depth });
        } else {
          const phase = s.motion ? now / 1000 + n.id.length : 0;
          const b = s.motion ? TUNING.breathe : 0;
          pts.set(n.id, { x: cx + p.x * scale + Math.sin(phase) * b, y: cy + p.y * scale + Math.cos(phase * 0.8) * b, depth: 0 });
        }
      }

      const filtered = (n: GraphNode) =>
        (s.areaFilter.size === 0 || s.areaFilter.has(n.area) || n.kind === "root") && (s.kindFilter.size === 0 || s.kindFilter.has(n.kind) || n.kind === "root");
      const alphaOf = (n: GraphNode) => (!filtered(n) || (s.neighbours && !s.neighbours.has(n.id)) ? TUNING.dimAlpha : 1);
      const byId = new Map(s.graph.nodes.map((n) => [n.id, n]));

      for (const l of s.graph.links) {
        const a = pts.get(l.s);
        const b = pts.get(l.t);
        const na = byId.get(l.s);
        const nb = byId.get(l.t);
        if (!a || !b || !na || !nb) continue;
        const lit = s.neighbours?.has(l.s) && s.neighbours?.has(l.t);
        ctx.globalAlpha = lit ? 0.7 : Math.min(alphaOf(na), alphaOf(nb)) * TUNING.linkAlpha;
        ctx.strokeStyle = lit ? colors["--accent"] : colors[areaVar(s.graph.areas, nb.area)];
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }

      const order = s.graph.nodes.slice().sort((a, b) => (pts.get(a.id)!.depth - pts.get(b.id)!.depth));
      const hits: Screen[] = [];
      for (const n of order) {
        const p = pts.get(n.id)!;
        const depthShade = s.view === "orbit" ? 0.45 + 0.55 * ((p.depth + 1) / 2) : 1;
        const r = n.kind === "root" ? 9 : TUNING.dotBase + Math.sqrt(s.degree.get(n.id) ?? 0) * TUNING.dotPerLink + (n.kind === "project" ? 3 : 0);
        const color = n.kind === "root" ? colors["--accent"] : colors[areaVar(s.graph.areas, n.area)];
        ctx.globalAlpha = alphaOf(n) * depthShade;
        if (n.kind === "root") {
          const halo = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, r * 4);
          halo.addColorStop(0, `${color}88`);
          halo.addColorStop(1, "transparent");
          ctx.fillStyle = halo;
          ctx.fillRect(p.x - r * 4, p.y - r * 4, r * 8, r * 8);
        }
        shape(ctx, n.kind, p.x, p.y, r);
        if (n.kind === "session") {
          ctx.strokeStyle = color;
          ctx.lineWidth = 1.4;
          ctx.stroke();
          ctx.beginPath();
          ctx.arc(p.x, p.y, r * 0.4, 0, Math.PI * 2);
        }
        ctx.fillStyle = color;
        ctx.fill();
        if (n.id === s.selected) {
          ctx.globalAlpha = 1;
          ctx.strokeStyle = colors["--text"];
          ctx.lineWidth = 1.5;
          ctx.beginPath();
          ctx.arc(p.x, p.y, r + 4, 0, Math.PI * 2);
          ctx.stroke();
        }
        ctx.lineWidth = 1;
        hits.push({ id: n.id, x: p.x, y: p.y, r });

        const showName = n.kind === "root" || n.kind === "project" || n.id === s.selected || (s.neighbours?.has(n.id) && s.neighbours.size < 30) || s.names;
        if (showName && alphaOf(n) === 1) {
          const text = n.kind === "root" ? n.label.toUpperCase() : n.label.length > 28 ? `${n.label.slice(0, 27)}…` : n.label;
          ctx.font = `${n.kind === "project" || n.kind === "root" ? 11 : 10}px ${family}`;
          const tw = ctx.measureText(text).width;
          const ty = p.y + r + 13;
          ctx.globalAlpha = 0.85;
          ctx.fillStyle = colors["--panel"];
          ctx.fillRect(p.x - tw / 2 - 3, ty - 10, tw + 6, 13);
          ctx.globalAlpha = 1;
          ctx.fillStyle = n.kind === "project" ? color : colors["--text"];
          ctx.fillText(text, p.x, ty);
        }
      }
      ctx.globalAlpha = 1;
      screen.current = hits;
      raf = requestAnimationFrame(frame);
    };
    raf = requestAnimationFrame(frame);
    return () => {
      cancelAnimationFrame(raf);
      observer.disconnect();
      theme.removeEventListener("change", recolor);
    };
  }, []);

  const pick = (e: React.PointerEvent | React.MouseEvent): string | null => {
    const rect = canvas.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    let best: Screen | null = null;
    let bestD = Infinity;
    for (const s of screen.current) {
      const d = Math.hypot(s.x - x, s.y - y);
      if (d < Math.max(TUNING.hitRadius, s.r + 3) && d < bestD) {
        best = s;
        bestD = d;
      }
    }
    return best?.id ?? null;
  };

  const flyTo = (id: string) => {
    const el = canvas.current;
    const p = screen.current.find((s) => s.id === id);
    if (!el || !p) return;
    const cam = cameraTarget.current;
    const w = el.clientWidth;
    const h = el.clientHeight;
    const zoom = Math.max(cam.zoom, 2);
    // Keep the node's world position under the centre at the new zoom.
    const k = zoom / camera.current.zoom;
    cameraTarget.current = { zoom, x: (w / 2 - (p.x - w / 2 - camera.current.x) * k) - w / 2, y: (h / 2 - (p.y - h / 2 - camera.current.y) * k) - h / 2 };
  };

  const toggle = <T,>(set: Set<T>, value: T) => {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    return next;
  };

  const node = graph.nodes.find((n) => n.id === selected) ?? null;
  const counts = (pred: (n: GraphNode) => boolean) => graph.nodes.filter(pred).length;

  return (
    <div ref={root} className="flex h-full min-h-0 flex-col bg-panel">
      <div className="flex flex-wrap items-center gap-x-1 gap-y-2 border-b border-line px-3 py-2">
        {VIEWS.map((v) => (
          <button
            key={v}
            onClick={() => setView(v)}
            className={clsx("label px-2 py-1 transition-colors hover:text-text", view === v && "border-b border-accent text-text")}
          >
            {VIEW_LABEL[v]}
          </button>
        ))}
        <span className="mx-1 h-4 w-px bg-line" />
        <button className={clsx("label px-2 py-1", names && "text-text")} onClick={() => setNames(!names)}>
          names
        </button>
        <button className={clsx("label px-2 py-1", motion && "text-text")} onClick={() => setMotion(!motion)}>
          motion
        </button>
        <div className="relative ml-1">
          <Search className="pointer-events-none absolute top-1.5 left-1.5 size-3.5 text-muted" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && matches[0]) {
                onSelect(matches[0]);
                flyTo(matches[0].id);
                setQuery("");
              }
            }}
            placeholder="search"
            className="h-7 w-36 rounded border border-line bg-panel-2 pr-2 pl-6 text-[11px] focus:border-accent focus:outline-none"
          />
          {matches.length > 0 && (
            <ul className="absolute z-20 mt-1 w-72 rounded border border-line bg-panel shadow-lg">
              {matches.map((m) => (
                <li key={m.id}>
                  <button
                    className="flex w-full items-center gap-2 px-2 py-1.5 text-left text-[11px] hover:bg-panel-2"
                    onClick={() => {
                      onSelect(m);
                      flyTo(m.id);
                      setQuery("");
                    }}
                  >
                    <span className="text-muted">{KIND_GLYPH[m.kind]}</span>
                    <span className="truncate">{m.label}</span>
                    <span className="ml-auto text-muted">{m.area}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        <button className="label flex items-center gap-1 px-2 py-1 hover:text-text" onClick={() => (cameraTarget.current = { zoom: 1, x: 0, y: 0 })}>
          <Scan className="size-3.5" /> fit
        </button>
        <button className="label flex items-center gap-1 px-2 py-1 hover:text-text" onClick={() => (document.fullscreenElement ? document.exitFullscreen() : root.current?.requestFullscreen?.())}>
          <Maximize className="size-3.5" /> full
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-1.5 px-3 pt-2 text-[11px]">
        <span className="text-muted">
          {graph.nodes.length} items · {graph.links.length} links
        </span>
        {stats}
      </div>
      <div className="flex flex-wrap gap-1.5 px-3 py-2">
        <button
          className={clsx("rounded border px-2 py-0.5 text-[11px]", areaFilter.size === 0 ? "border-accent text-text" : "border-line text-muted")}
          onClick={() => setAreaFilter(new Set())}
        >
          all areas
        </button>
        {graph.areas.map((a) => (
          <button
            key={a}
            onClick={() => setAreaFilter(toggle(areaFilter, a))}
            className={clsx("flex items-center gap-1.5 rounded border px-2 py-0.5 text-[11px]", areaFilter.has(a) ? "border-text text-text" : "border-line text-muted hover:text-text")}
          >
            <span className="size-2 rounded-full" style={{ background: `var(${areaVar(graph.areas, a)})` }} />
            {a} <span className="opacity-60">{counts((n) => n.area === a)}</span>
          </button>
        ))}
        <span className="mx-1 h-5 w-px bg-line" />
        {KINDS.map((k) => (
          <button
            key={k}
            onClick={() => setKindFilter(toggle(kindFilter, k))}
            className={clsx("flex items-center gap-1 rounded border px-2 py-0.5 text-[11px]", kindFilter.has(k) ? "border-text text-text" : "border-line text-muted hover:text-text")}
          >
            <span>{KIND_GLYPH[k]}</span> {k}
          </button>
        ))}
      </div>

      <div ref={box} className="relative min-h-[360px] flex-1 overflow-hidden">
        <canvas
          ref={canvas}
          className={clsx("absolute inset-0", hover ? "cursor-pointer" : "cursor-grab active:cursor-grabbing")}
          onPointerDown={(e) => {
            drag.current = { x: e.clientX, y: e.clientY, cx: cameraTarget.current.x, cy: cameraTarget.current.y, moved: false };
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={(e) => {
            const d = drag.current;
            if (d) {
              const dx = e.clientX - d.x;
              const dy = e.clientY - d.y;
              if (Math.abs(dx) + Math.abs(dy) > 3) d.moved = true;
              if (d.moved) {
                cameraTarget.current = { ...cameraTarget.current, x: d.cx + dx, y: d.cy + dy };
                camera.current = { ...camera.current, x: d.cx + dx, y: d.cy + dy };
                return;
              }
            }
            setHover(pick(e));
          }}
          onPointerUp={(e) => {
            const d = drag.current;
            drag.current = null;
            if (d?.moved) return;
            const id = pick(e);
            onSelect(id ? (graph.nodes.find((n) => n.id === id) ?? null) : null);
          }}
          onPointerLeave={() => setHover(null)}
          onWheel={(e) => {
            const cam = cameraTarget.current;
            const zoom = Math.min(8, Math.max(0.4, cam.zoom * (e.deltaY < 0 ? 1.15 : 1 / 1.15)));
            cameraTarget.current = { ...cam, zoom };
          }}
        />
        {node && (
          <div className="absolute bottom-3 left-3 z-10 w-80 max-w-[calc(100%-1.5rem)] rounded-md border border-line bg-panel/95 p-3 shadow-xl backdrop-blur">
            <div className="flex items-start gap-2">
              <span className="text-muted">{KIND_GLYPH[node.kind]}</span>
              <div className="min-w-0 flex-1">
                <p className="truncate text-[13px] font-semibold">{node.label}</p>
                <p className="label mt-0.5">
                  {node.kind} · {node.area}
                  {node.status ? ` · ${node.status.replace("_", " ")}` : ""}
                </p>
              </div>
              <button className="text-muted hover:text-text" onClick={() => onSelect(null)} aria-label="Close card">
                ×
              </button>
            </div>
            {node.note && <p className="mt-2 line-clamp-3 text-[11px] text-muted">{node.note}</p>}
            <div className="mt-2.5 flex flex-wrap gap-1.5">
              {actions(node)}
              <button className="h-6 rounded border border-line px-2 text-[11px] hover:border-muted" onClick={() => flyTo(node.id)}>
                fly to
              </button>
            </div>
          </div>
        )}
        <p className="pointer-events-none absolute right-0 bottom-2 left-0 text-center text-[10px] tracking-[0.14em] text-muted uppercase">
          dot = item · click = card · area chip isolates · wheel zooms · drag pans
        </p>
      </div>
    </div>
  );
}
