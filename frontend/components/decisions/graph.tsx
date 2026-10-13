"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";

import type { DecisionModel } from "@/lib/generated/DecisionModel";
import type { DecisionNode } from "@/lib/generated/DecisionNode";

import { LAYOUT, edgeKey, isFailEdge, nodeTone, tint } from "./model";
import type { Pulse } from "./use-decisions";

export type Token = {
  id: number;
  node: DecisionNode;
  color: string;
  /** Hollow: no provider assigned yet. */
  assigned: boolean;
  /** Dashed ring: needs a person. */
  attention: boolean;
};

export type Selection = { type: "task"; id: number } | { type: "node"; id: DecisionNode };

type Pt = [number, number];
type Geo = { x: number; y: number; w: number; h: number; cx: number; cy: number };
type Path = { p0: Pt; p3: Pt; h: boolean; c1?: Pt; c2?: Pt };

/** Where along an edge its label sits (default: the middle). */
const LABEL_T: Record<string, number> = { "watch>retry": 0.3, "retry>watch": 0.3, "audit>retry": 0.35 };

const HEIGHT = 470;
const MIN_WIDTH = 1040;
const NODE_H = 66;

function geometry(width: number) {
  const nodeW = Math.min(124, (width - 48) / 9 - 12);
  const step = (width - 48 - nodeW) / 8;
  const pos = {} as Record<DecisionNode, Geo>;
  for (const [k, { c, r }] of Object.entries(LAYOUT) as [DecisionNode, { c: number; r: number }][]) {
    const x = 24 + c * step;
    const y = 46 + r * 1.62 * 140;
    pos[k] = { x, y, w: nodeW, h: NODE_H, cx: x + nodeW / 2, cy: y + NODE_H / 2 };
  }
  return pos;
}

/** Where an edge leaves and enters its nodes (from the design's canvas). */
function anchors(a: DecisionNode, b: DecisionNode, pos: Record<DecisionNode, Geo>): Path {
  const A = pos[a];
  const B = pos[b];
  if (LAYOUT[a].r === LAYOUT[b].r) return { p0: [A.x + A.w, A.cy], p3: [B.x, B.cy], h: true };
  if (a === "retry" && b === "ready") return { p0: [A.x, A.cy], p3: [B.cx + 10, B.y + B.h], h: false, c1: [A.x - 120, A.cy], c2: [B.cx + 10, B.y + B.h + 60] };
  if (a === "retry" && b === "watch") return { p0: [A.cx - 18, A.y], p3: [B.cx + 14, B.y + B.h], h: false };
  const off: Partial<Record<DecisionNode, number>> = { launch: 6, watch: -14, ready: -10 };
  const inOff: Partial<Record<DecisionNode, number>> = { launch: -30, watch: -6, verify: 14, audit: 30 };
  return { p0: [A.cx + (off[a] ?? 0), A.y + A.h], p3: [B.cx + (inOff[a] ?? 0), B.y], h: false };
}

function controls(g: Path): [Pt, Pt] {
  const { p0, p3 } = g;
  const c1: Pt = g.c1 ?? (g.h ? [(p0[0] + p3[0]) / 2, p0[1]] : [p0[0], (p0[1] + p3[1]) / 2]);
  const c2: Pt = g.c2 ?? (g.h ? [(p0[0] + p3[0]) / 2, p3[1]] : [p3[0], (p0[1] + p3[1]) / 2]);
  return [c1, c2];
}

function bez(g: Path, t: number): Pt {
  const [c1, c2] = controls(g);
  const u = 1 - t;
  const f = (i: 0 | 1) => u * u * u * g.p0[i] + 3 * u * u * t * c1[i] + 3 * u * t * t * c2[i] + t * t * t * g.p3[i];
  return [f(0), f(1)];
}

function d(g: Path): string {
  const [c1, c2] = controls(g);
  return `M${g.p0[0]},${g.p0[1]} C${c1[0]},${c1[1]} ${c2[0]},${c2[1]} ${g.p3[0]},${g.p3[1]}`;
}

/** A dot that runs along an edge once; SMIL animations added later must be started by hand. */
function PulseDot({ path }: { path: string }) {
  return (
    <circle r={4} style={{ fill: "var(--accent)", filter: "drop-shadow(0 0 6px var(--accent))" }}>
      <animateMotion
        ref={(el) => {
          (el as SVGAnimationElement | null)?.beginElement();
        }}
        begin="indefinite"
        dur="1s"
        fill="freeze"
        path={path}
      />
    </circle>
  );
}

const MONO = "var(--font-jetbrains-mono), ui-monospace, monospace";

/** The decision graph: nodes with live counts, edges with today's counts, task tokens that glide between nodes. */
export function DecisionGraph({
  model,
  counts,
  tokens,
  pulses,
  selection,
  path,
  onSelect,
  renderHover,
}: {
  model: DecisionModel;
  counts: Map<string, number>;
  tokens: Token[];
  pulses: Pulse[];
  selection: Selection | null;
  /** Edges the selected task has taken, by `edgeKey`. */
  path: Set<string>;
  onSelect: (s: Selection) => void;
  /** Floating card under a hovered node. */
  renderHover: (node: DecisionNode) => ReactNode;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(MIN_WIDTH);
  const [hover, setHover] = useState<Selection | null>(null);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setWidth(Math.max(MIN_WIDTH, e.contentRect.width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const pos = geometry(width);
  const selNode = selection?.type === "node" ? selection.id : null;
  const selTask = selection?.type === "task" ? tokens.find((t) => t.id === selection.id) : undefined;
  const here = new Map<DecisionNode, Token[]>();
  for (const t of [...tokens].sort((a, b) => a.id - b.id)) here.set(t.node, [...(here.get(t.node) ?? []), t]);

  return (
    <div ref={box} className="relative h-[470px] min-w-[1040px]">
      <svg width={width} height={HEIGHT} className="absolute inset-0 block select-none" role="img" aria-label="Orchestrator decision graph">
        <text x={24} y={14} style={{ font: `500 9px ${MONO}`, letterSpacing: "2px", fill: "var(--muted)" }}>
          HAPPY PATH →
        </text>
        <text x={24} y={pos.retry.y - 14} style={{ font: `500 9px ${MONO}`, letterSpacing: "2px", fill: "var(--muted)" }}>
          RECOVERY
        </text>

        {model.edges.map((e) => {
          const key = edgeKey(e.from, e.to);
          const g = anchors(e.from, e.to, pos);
          const n = counts.get(key) ?? 0;
          const onPath = path.has(key);
          const lit = selNode !== null && (e.from === selNode || e.to === selNode);
          const fail = isFailEdge(e.to);
          const color = onPath ? "var(--accent)" : fail ? "var(--bad)" : "var(--text)";
          const alpha = onPath ? 95 : lit ? 60 : n ? (fail ? 38 : 30) : 14;
          const end = bez(g, 0.97);
          const ang = Math.atan2(g.p3[1] - end[1], g.p3[0] - end[0]);
          const head = `M${g.p3[0]},${g.p3[1]} L${g.p3[0] - 7 * Math.cos(ang - 0.45)},${g.p3[1] - 7 * Math.sin(ang - 0.45)} L${g.p3[0] - 7 * Math.cos(ang + 0.45)},${g.p3[1] - 7 * Math.sin(ang + 0.45)} Z`;
          // Top-row labels sit above the row; the edges in and out of Retry spread theirs along the curves.
          const mid = bez(g, LABEL_T[key] ?? 0.5);
          let [lx, ly] = [mid[0], g.h ? (LAYOUT[e.from].r === 0 ? pos.ready.y - 13 : mid[1] - 10) : mid[1]];
          if (e.from === "retry" && e.to === "ready") [lx, ly] = [pos.retry.x - 150, pos.retry.cy + 20];
          const count = `×${n}`;
          const bw = (e.label.length + count.length) * 5.5 + 20;
          return (
            <g key={key}>
              <path d={d(g)} fill="none" strokeDasharray={!n && !onPath ? "3 4" : undefined} style={{ stroke: tint(color, alpha), strokeWidth: onPath ? 2 : 1.2 }} />
              <path d={head} style={{ fill: tint(color, Math.min(100, alpha + 20)) }} />
              <rect x={lx - bw / 2} y={ly - 8} width={bw} height={16} rx={4} style={{ fill: tint("var(--inset)", 92) }} />
              <text x={lx - bw / 2 + 6} y={ly + 3} style={{ font: `400 9px ${MONO}`, fill: onPath ? "var(--accent-soft)" : tint("var(--text-2)", n ? 95 : 60) }}>
                {e.label}
              </text>
              <text x={lx + bw / 2 - 6} y={ly + 3} textAnchor="end" style={{ font: `600 9px ${MONO}`, fill: onPath ? "var(--accent)" : fail && n ? "var(--bad)" : n ? "var(--text)" : "var(--muted)" }}>
                {count}
              </text>
            </g>
          );
        })}

        {pulses.map((p) => {
          const known = model.edges.some((e) => e.from === p.from && e.to === p.to);
          return known ? <PulseDot key={p.id} path={d(anchors(p.from, p.to, pos))} /> : null;
        })}

        {model.nodes.map((n) => {
          const p = pos[n.id];
          const list = here.get(n.id) ?? [];
          const isSel = selNode === n.id || selTask?.node === n.id;
          const hov = hover?.type === "node" && hover.id === n.id;
          const tone = nodeTone(n.id);
          const fill = n.id === "done" ? tint("var(--accent)", 12) : n.id === "failed" ? tint("var(--bad)", 10) : n.id === "blocked" ? tint("var(--warn)", 8) : "var(--menu)";
          const stroke = isSel ? "var(--accent)" : hov ? tint("var(--text)", 50) : list.length ? tint(tone, 55) : "var(--line-3)";
          const titleColor = n.id === "done" ? "var(--accent-soft)" : n.id === "failed" ? "var(--bad)" : n.id === "blocked" ? "var(--warn)" : "var(--text)";
          const fn = n.source.split(" ·")[0];
          return (
            <g
              key={n.id}
              className="cursor-pointer"
              onClick={() => onSelect({ type: "node", id: n.id })}
              onPointerEnter={() => setHover({ type: "node", id: n.id })}
              onPointerLeave={() => setHover(null)}
            >
              <rect
                x={p.x}
                y={p.y}
                width={p.w}
                height={p.h}
                rx={n.terminal ? p.h / 2 : 10}
                style={{ fill, stroke, strokeWidth: isSel ? 2 : 1.2, filter: isSel ? "drop-shadow(0 0 8px var(--accent))" : undefined }}
              />
              <text x={p.x + 12} y={p.y + 22} style={{ font: "600 12.5px var(--font-geist-sans), sans-serif", fill: titleColor }}>
                {n.label}
              </text>
              <text x={p.x + 12} y={p.y + 38} style={{ font: `400 8.5px ${MONO}`, fill: "var(--muted)" }}>
                {fn.length > 18 ? `${fn.slice(0, 17)}…` : fn}
              </text>
              <text
                x={p.x + p.w - 10}
                y={p.y + p.h - 12}
                textAnchor="end"
                style={{ font: `600 17px ${MONO}`, fill: list.length ? (n.id === "done" ? "var(--accent)" : n.id === "failed" ? "var(--bad)" : "var(--text)") : "var(--line-3)" }}
              >
                {list.length}
              </text>
            </g>
          );
        })}

        {model.nodes.flatMap((n) => {
          const p = pos[n.id];
          const list = here.get(n.id) ?? [];
          const perRow = Math.max(3, Math.floor((p.w - 16) / 15));
          return list.map((t, i) => {
            const x = p.x + 12 + (i % perRow) * 15;
            const y = p.y + p.h + 14 + Math.floor(i / perRow) * 15;
            const sel = selTask?.id === t.id;
            const hov = hover?.type === "task" && hover.id === t.id;
            return (
              <g
                key={t.id}
                className="cursor-pointer"
                style={{ transform: `translate(${x}px, ${y}px)`, transition: "transform 900ms cubic-bezier(.4,0,.2,1)" }}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect({ type: "task", id: t.id });
                }}
                onPointerEnter={() => setHover({ type: "task", id: t.id })}
                onPointerLeave={() => setHover(null)}
              >
                <title>{`#${t.id}`}</title>
                <circle r={8} style={{ fill: "transparent" }} />
                <path d="M0,-5 L5,0 L0,5 L-5,0 Z" style={{ fill: t.assigned ? t.color : "transparent", stroke: t.color, strokeWidth: 1.2 }} />
                {t.attention && <circle r={8} fill="none" strokeDasharray="2 2" style={{ stroke: "var(--warn)", strokeWidth: 1.2 }} />}
                {(sel || hov) && (
                  <>
                    <circle r={9.5} fill="none" style={{ stroke: "var(--accent)", strokeWidth: 1.5 }} />
                    <rect x={11} y={-8} width={String(t.id).length * 6.5 + 16} height={16} rx={3} style={{ fill: tint("var(--inset)", 90) }} />
                    <text x={16} y={4} style={{ font: `600 10px ${MONO}`, fill: "var(--accent-soft)" }}>
                      #{t.id}
                    </text>
                  </>
                )}
              </g>
            );
          });
        })}

        <text x={width - 24} y={HEIGHT - 14} textAnchor="end" style={{ font: `400 9px ${MONO}`, fill: "var(--muted)" }}>
          ◆ task, colour = provider · ◇ not assigned · dashed ring = needs attention · ×n = times taken today
        </text>
      </svg>
      {hover?.type === "node" && (
        <div
          className="pointer-events-none absolute z-10 w-[260px] rounded-[10px] border border-line-2 bg-menu p-3 shadow-[0_20px_50px_rgba(0,0,0,.5)]"
          style={
            LAYOUT[hover.id].r
              ? { left: Math.min(pos[hover.id].x, width - 272), top: pos[hover.id].y - 8, transform: "translateY(-100%)" }
              : { left: Math.min(pos[hover.id].x, width - 272), top: pos[hover.id].y + NODE_H + 8 }
          }
        >
          {renderHover(hover.id)}
        </div>
      )}
    </div>
  );
}
