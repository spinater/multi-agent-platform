"use client";

/** Configure › Agents › Relation map (design: Configure.dc.html `drawRel`, circle layout): agents on an inner ring in
 * provider colours, what they get on an outer ring in arcs (skills, rules, MCP servers, tools), curved edges from
 * each agent to everything it gets. Hover traces links; clicking an item pins the highlight; clicking an agent
 * selects it in the editor. Hollow dashed nodes are disabled skills or MCP servers that are not connected. */

import { useEffect, useRef, useState } from "react";

export type RelType = "skill" | "rule" | "mcp" | "tool";
export type RelItem = { id: string; type: RelType; label: string; off: boolean };
export type RelAgent = { id: string; name: string; color: string };

const RC: Record<RelType, string> = { skill: "#B48CF2", rule: "#F2C14E", mcp: "#38bdf8", tool: "#7ee2a8" };
const RT: Record<RelType, string> = { skill: "skills", rule: "rules", mcp: "mcp servers", tool: "tools" };
const TYPES: RelType[] = ["skill", "rule", "mcp", "tool"];

function rgba(hex: string, a: number) {
  const v = parseInt(hex.slice(1), 16);
  return `rgba(${(v >> 16) & 255},${(v >> 8) & 255},${v & 255},${a})`;
}

/** Provider colour tokens resolved to hex for the canvas. */
function resolve(color: string, el: HTMLElement): string {
  const m = /^var\((--[^)]+)\)$/.exec(color);
  if (!m) return color;
  const v = getComputedStyle(el).getPropertyValue(m[1]).trim();
  return v.startsWith("#") && v.length === 7 ? v : "#8b95a5";
}

export function RelationMap({
  agents,
  items,
  binds,
  selected,
  onSelect,
}: {
  agents: RelAgent[];
  items: RelItem[];
  binds: (agent: string, item: RelItem) => boolean;
  selected: string | null;
  onSelect: (agent: string) => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const pts = useRef<{ x: number; y: number; r: number; id: string; kind: "agent" | "res" }[]>([]);
  const [hover, setHover] = useState<string | null>(null);
  const [pinned, setPinned] = useState<string | null>(null);
  const [size, setSize] = useState(0);
  const focus = hover ?? pinned;

  useEffect(() => {
    const c = ref.current;
    if (!c || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setSize(c.clientWidth * 10000 + c.clientHeight));
    ro.observe(c);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const c = ref.current;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const W = c.clientWidth;
    const H = c.clientHeight;
    if (!W || !H) return;
    if (c.width !== Math.round(W * dpr) || c.height !== Math.round(H * dpr)) {
      c.width = Math.round(W * dpr);
      c.height = Math.round(H * dpr);
    }
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const text = resolve("var(--text)", c);
    const nav = resolve("var(--nav)", c);
    const faint = "#5d6673";
    const cx = W / 2;
    const cy = (H - 44) / 2 + 4;
    const m = Math.min(W, H - 90);
    const R1 = m * 0.15;
    const R2 = m * 0.28;
    const gap = 0.2;
    const n = Math.max(1, items.length);
    const span = Math.PI * 2 - gap * TYPES.length;
    const pos: Record<string, { x: number; y: number; ang: number }> = {};
    const P: typeof pts.current = [];
    const short = (s: string) => (s.length > 14 ? `${s.slice(0, 13)}…` : s);
    const node = (x: number, y: number, r: number, col: string, off: boolean, ring: string | null) => {
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      if (off) {
        ctx.setLineDash([2, 2]);
        ctx.strokeStyle = col;
        ctx.lineWidth = 1.3;
        ctx.stroke();
        ctx.setLineDash([]);
      } else {
        ctx.fillStyle = col;
        ctx.fill();
      }
      if (ring) {
        ctx.beginPath();
        ctx.arc(x, y, r + 3.5, 0, Math.PI * 2);
        ctx.strokeStyle = ring;
        ctx.lineWidth = 1.6;
        ctx.stroke();
      }
    };
    const label = (txt: string, x: number, y: number, ang: number, col: string) => {
      ctx.save();
      ctx.font = "400 10.5px 'JetBrains Mono',monospace";
      ctx.fillStyle = col;
      ctx.translate(x, y);
      const left = Math.cos(ang) < 0;
      ctx.rotate(left ? ang + Math.PI : ang);
      ctx.textAlign = left ? "right" : "left";
      ctx.textBaseline = "middle";
      ctx.fillText(txt, 0, 0);
      ctx.restore();
    };
    let a = -Math.PI / 2 + gap / 2;
    for (const ty of TYPES) {
      const g = items.filter((r) => r.type === ty);
      const a0 = a;
      g.forEach((r, i) => {
        const ang = a + ((i + 0.5) * span) / n;
        pos[r.id] = { x: cx + Math.cos(ang) * R2, y: cy + Math.sin(ang) * R2, ang };
      });
      a += (g.length * span) / n;
      if (g.length) {
        ctx.beginPath();
        ctx.arc(cx, cy, R2 - 12, a0, a);
        ctx.strokeStyle = rgba(RC[ty], 0.35);
        ctx.lineWidth = 2;
        ctx.stroke();
        const mid = (a0 + a) / 2;
        ctx.font = "600 9.5px 'JetBrains Mono',monospace";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillStyle = rgba(RC[ty], 0.85);
        ctx.fillText(RT[ty].toUpperCase(), cx + Math.cos(mid) * (R2 - 30), cy + Math.sin(mid) * (R2 - 30));
      }
      a += gap;
    }
    agents.forEach((ag, i) => {
      const ang = -Math.PI / 2 + Math.PI / Math.max(1, agents.length) + (i * Math.PI * 2) / Math.max(1, agents.length);
      pos[`agent:${ag.id}`] = { x: cx + Math.cos(ang) * R1, y: cy + Math.sin(ang) * R1, ang };
    });
    const fa = focus?.startsWith("agent:") ? agents.find((x) => `agent:${x.id}` === focus) : undefined;
    const fr = !fa && focus ? items.find((x) => x.id === focus) : undefined;
    const related = (id: string) => {
      if (!focus || focus === id) return true;
      if (id.startsWith("agent:")) {
        const ag = agents.find((x) => `agent:${x.id}` === id);
        return !!(fr && ag && binds(ag.id, fr));
      }
      const r = items.find((x) => x.id === id);
      return !!(fa && r && binds(fa.id, r));
    };
    for (const ag of agents)
      for (const r of items) {
        if (!binds(ag.id, r)) continue;
        const A = pos[`agent:${ag.id}`];
        const B = pos[r.id];
        const on = !!focus && (focus === `agent:${ag.id}` || focus === r.id);
        ctx.beginPath();
        ctx.moveTo(A.x, A.y);
        ctx.quadraticCurveTo(cx + ((A.x + B.x) / 2 - cx) * 0.3, cy + ((A.y + B.y) / 2 - cy) * 0.3, B.x, B.y);
        ctx.strokeStyle = rgba(RC[r.type], on ? 0.9 : focus ? 0.05 : r.off ? 0.12 : 0.28);
        ctx.lineWidth = on ? 1.8 : 1;
        if (r.off) ctx.setLineDash([3, 3]);
        ctx.stroke();
        ctx.setLineDash([]);
      }
    for (const r of items) {
      const p = pos[r.id];
      ctx.globalAlpha = related(r.id) ? 1 : 0.25;
      node(p.x, p.y, 5, r.off ? faint : RC[r.type], r.off, focus === r.id ? text : null);
      label(short(r.label), p.x + Math.cos(p.ang) * 10, p.y + Math.sin(p.ang) * 10, p.ang, focus === r.id ? text : r.off ? faint : nav);
      ctx.globalAlpha = 1;
      P.push({ x: p.x, y: p.y, r: 5, id: r.id, kind: "res" });
    }
    for (const ag of agents) {
      const p = pos[`agent:${ag.id}`];
      const id = `agent:${ag.id}`;
      ctx.globalAlpha = related(id) ? 1 : 0.25;
      node(p.x, p.y, 10, resolve(ag.color, c), false, selected === ag.id ? "#FF7A1A" : focus === id ? text : null);
      ctx.font = "600 11.5px Geist,sans-serif";
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      ctx.fillStyle = text;
      ctx.fillText(ag.name, p.x, p.y + 15);
      ctx.globalAlpha = 1;
      P.push({ x: p.x, y: p.y, r: 11, id, kind: "agent" });
    }
    pts.current = P;
  }, [agents, items, binds, selected, focus, size]);

  const hit = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const b = e.currentTarget.getBoundingClientRect();
    const x = e.clientX - b.left;
    const y = e.clientY - b.top;
    let best: (typeof pts.current)[number] | null = null;
    let bd = 1e9;
    for (const p of pts.current) {
      const d = Math.hypot(p.x - x, p.y - y);
      if (d < p.r + 6 && d < bd) {
        bd = d;
        best = p;
      }
    }
    return best;
  };

  let info: string;
  let infoColor = "var(--text-2)";
  const fa = focus?.startsWith("agent:") ? agents.find((x) => `agent:${x.id}` === focus) : undefined;
  if (fa) info = `${fa.name} · ${TYPES.map((ty) => `${items.filter((r) => r.type === ty && binds(fa.id, r)).length} ${RT[ty]}`).join(" · ")} · click to edit`;
  else if (focus) {
    const r = items.find((x) => x.id === focus);
    const by = r ? agents.filter((a) => binds(a.id, r)).map((a) => a.name) : [];
    infoColor = r ? RC[r.type] : infoColor;
    info = r
      ? `${r.label} · ${RT[r.type].replace(/s$/, "")}${r.off ? (r.type === "skill" ? " · disabled in Skills" : " · not connected") : ""} · ${by.length ? `used by ${by.join(", ")}` : "no agent uses it"}`
      : "";
  } else info = `${agents.length} agents · ${agents.reduce((n, a) => n + items.filter((r) => binds(a.id, r)).length, 0)} links · hover to trace · click an agent to edit it · click an item to pin it`;

  return (
    <div className="flex flex-col gap-2.5 rounded-xl border border-line bg-surface p-3.5">
      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-2">
        <span className="text-[14px] leading-none font-semibold">Relation map</span>
        <span className="text-[11.5px] leading-[1.4] text-muted">Which skills, rules, MCP servers and tools each agent gets.</span>
      </div>
      <div className="relative h-[520px] overflow-hidden rounded-[10px] border border-line max-sm:h-[380px]" style={{ background: "radial-gradient(600px 400px at 50% 50%, color-mix(in srgb, var(--panel-2) 80%, transparent) 0%, var(--inset) 70%)" }}>
        <canvas
          ref={ref}
          className="absolute inset-0 block size-full"
          onMouseMove={(e) => {
            const h = hit(e);
            e.currentTarget.style.cursor = h ? "pointer" : "default";
            if ((h?.id ?? null) !== hover) setHover(h?.id ?? null);
          }}
          onMouseLeave={() => setHover(null)}
          onClick={(e) => {
            const h = hit(e);
            if (!h) setPinned(null);
            else if (h.kind === "agent") {
              setPinned(null);
              onSelect(h.id.slice(6));
            } else setPinned(pinned === h.id ? null : h.id);
          }}
        />
        <div className="pointer-events-none absolute bottom-2.5 left-3 flex max-w-[calc(100%-24px)] flex-wrap gap-x-3 gap-y-1 font-mono text-[10.5px] leading-[1.4] text-muted">
          {TYPES.map((t) => (
            <span key={t} className="flex items-center gap-1.5">
              <span className="size-2 rounded-full border-[1.5px]" style={{ borderColor: RC[t], background: RC[t] }} />
              {t === "mcp" ? "mcp server" : t}
            </span>
          ))}
          <span className="flex items-center gap-1.5">
            <span className="size-2 rounded-full border-[1.5px]" style={{ borderColor: "#F2A65A", background: "#5EA8F2" }} />
            agent · provider colour
          </span>
          <span className="flex items-center gap-1.5">
            <span className="size-2 rounded-full border-[1.5px] border-dashed" style={{ borderColor: "#5d6673" }} />
            hollow = disabled / not connected
          </span>
        </div>
      </div>
      <span className="font-mono text-[11.5px] leading-[1.45]" style={{ color: infoColor }}>
        {info}
      </span>
    </div>
  );
}
