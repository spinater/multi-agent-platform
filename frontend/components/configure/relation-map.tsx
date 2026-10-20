"use client";

/** Configure › Agents › Relation map (design: Configure.dc.html `drawRel`, circle layout), in layers: agents on the
 * inner ring in provider colours, then one ring per kind of thing they get (tools, rules, skills, MCP servers), the
 * smallest kind innermost so the busiest one has the outer ring to itself; curved edges from each agent to
 * everything it gets. Hover traces links; clicking an item pins the highlight; clicking an agent
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

/** The rings: the agents' radius grows with their number; each kind of item that has any gets its own ring
 * outside it, fewest first. */
function rings(agentCount: number, items: RelItem[]) {
  const r1 = Math.max(110, (agentCount * 30) / (2 * Math.PI));
  const kinds = TYPES.map((type) => ({ type, list: items.filter((i) => i.type === type) }))
    .filter((k) => k.list.length > 0)
    .sort((a, b) => a.list.length - b.list.length);
  const layers = kinds.map((k, i) => ({ ...k, r: r1 + 62 + i * 50 }));
  return { r1, layers };
}

/** The canvas height that fits every ring and the outer labels. */
function heightFor(agentCount: number, items: RelItem[]) {
  const { r1, layers } = rings(agentCount, items);
  const outer = layers.length ? layers[layers.length - 1].r : r1;
  return Math.max(520, Math.round(2 * (outer + 100) + 44));
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
    const pos: Record<string, { x: number; y: number; ang: number }> = {};
    const P: typeof pts.current = [];
    const { r1: R1, layers } = rings(agents.length, items);
    const outer = layers[layers.length - 1]?.type;
    const short = (s: string, max = 14) => (s.length > max ? `${s.slice(0, Math.max(1, max - 1))}…` : s);    const node = (x: number, y: number, r: number, col: string, off: boolean, ring: string | null) => {
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
    // A faint guide per layer, named at the top in its colour.
    const guide = (r: number, name: string, col: string) => {
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.strokeStyle = rgba(col, 0.16);
      ctx.lineWidth = 1;
      ctx.stroke();
      ctx.font = "600 9px 'JetBrains Mono',monospace";
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      ctx.fillStyle = rgba(col, 0.8);
      ctx.fillText(name, cx + 6, cy - r - 7);
    };
    guide(R1, `AGENTS · ${agents.length}`, "#9aa3ad");
    for (const { type, r, list } of layers) {
      guide(r, `${RT[type].toUpperCase()} · ${list.length}`, RC[type]);
      // Every layer starts half a step past the top, so its first node never sits on the name.
      const step = (Math.PI * 2) / list.length;
      list.forEach((it, i) => {
        const ang = -Math.PI / 2 + step * (i + 0.5);
        pos[it.id] = { x: cx + Math.cos(ang) * r, y: cy + Math.sin(ang) * r, ang };
      });
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
        // Bend along the rings (half way round, half way out) instead of through the middle.
        let d = B.ang - A.ang;
        d = Math.atan2(Math.sin(d), Math.cos(d));
        const mid = A.ang + d / 2;
        const rm = (Math.hypot(A.x - cx, A.y - cy) + Math.hypot(B.x - cx, B.y - cy)) / 2;
        ctx.quadraticCurveTo(cx + Math.cos(mid) * rm, cy + Math.sin(mid) * rm, B.x, B.y);
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
      const col = focus === r.id ? text : r.off ? faint : nav;
      if (r.type === outer) label(short(r.label), p.x + Math.cos(p.ang) * 10, p.y + Math.sin(p.ang) * 10, p.ang, col);
      else {
        const L = layers.find((l) => l.type === r.type)!;
        const room = Math.floor(((2 * Math.PI * L.r) / L.list.length - 8) / 6.2);
        if (room >= 3 || focus === r.id) {
          ctx.font = "400 10px 'JetBrains Mono',monospace";
          ctx.textAlign = "center";
          ctx.textBaseline = "top";
          ctx.fillStyle = col;
          ctx.fillText(focus === r.id ? r.label : short(r.label, room), p.x, p.y + 8);
        }
      }
      ctx.globalAlpha = 1;
      P.push({ x: p.x, y: p.y, r: 5, id: r.id, kind: "res" });
    }
    for (const ag of agents) {
      const p = pos[`agent:${ag.id}`];
      const id = `agent:${ag.id}`;
      ctx.globalAlpha = related(id) ? 1 : 0.25;
      node(p.x, p.y, 10, resolve(ag.color, c), false, selected === ag.id ? "#FF7A1A" : focus === id ? text : null);
      // Names point in to the centre, where nothing else is drawn.
      ctx.save();
      ctx.font = "600 11px Geist,sans-serif";
      ctx.fillStyle = text;
      ctx.translate(p.x, p.y);
      const left = Math.cos(p.ang) < 0;
      ctx.rotate(left ? p.ang + Math.PI : p.ang);
      ctx.textAlign = left ? "left" : "right";
      ctx.textBaseline = "middle";
      ctx.fillText(short(ag.name, Math.floor((R1 - 26) / 6.4)), left ? 15 : -15, 0);
      ctx.restore();
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
      <div className="relative overflow-hidden rounded-[10px] border border-line" style={{ height: heightFor(agents.length, items), background: "radial-gradient(600px 400px at 50% 50%, color-mix(in srgb, var(--panel-2) 80%, transparent) 0%, var(--inset) 70%)" }}>
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
