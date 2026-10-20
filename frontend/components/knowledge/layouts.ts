/** Knowledge › graph layouts other than the free force layout. Each puts every visible node at a fixed spot, in
 * layers, and says what to draw behind them (rings, orbits, column headers, domain names):
 *
 * - graph: three columns, DOMAINS · DOCUMENTS · CODE FILES, one band per domain; code is ordered by the documents
 *   that cite it, so links run mostly straight across.
 * - circle: domains as arcs around the middle, documents on the inner ring, code files on the outer ring, each
 *   domain on its own slice of both.
 * - orbit: the biggest domain at the centre, the others around it on a tilted ring; every domain is a small system
 *   with its documents on the inner orbit and its code files on the outer one. */

import type { KnowledgeGraph } from "@/lib/generated/KnowledgeGraph";
import type { KnowledgeGraphNode } from "@/lib/generated/KnowledgeGraphNode";

export const MODES = ["graph", "circle", "orbit", "free"] as const;
export type Mode = (typeof MODES)[number];

/** What to draw behind the nodes, in world units; text sizes are screen pixels. */
export type Guide =
  | { t: "ellipse"; x: number; y: number; rx: number; ry: number; color: string; alpha: number }
  | { t: "arc"; x: number; y: number; r: number; a0: number; a1: number; color: string; alpha: number; width: number }
  | { t: "band"; x: number; y: number; w: number; h: number; color: string }
  | { t: "text"; x: number; y: number; text: string; color: string; size: number; align: CanvasTextAlign; alpha: number; room?: number };

/** Where a node's label goes: beside it (graph), along the radius (circle), or wherever there is room. */
export type LabelSide = "left" | "right" | "in" | "out" | "free";

export type Placed = { pos: Map<string, [number, number]>; guides: Guide[]; side: Map<string, LabelSide>; curved: boolean };

const TILT = 0.55;

let monoStack = "";
/** The page's mono stack with its Thai fallback, for canvas text (the canvas cannot read CSS variables). */
export function mono(): string {
  if (!monoStack && typeof document !== "undefined") {
    const css = getComputedStyle(document.documentElement);
    const v = (name: string) => css.getPropertyValue(name).trim();
    monoStack = [v("--font-jetbrains-mono"), v("--font-thai"), "ui-monospace", "monospace"].filter(Boolean).join(", ");
  }
  return monoStack || "ui-monospace, monospace";
}

/** Members of each domain in the graph's domain order: documents by degree, code by where its citing documents sit. */
function groups(graph: KnowledgeGraph, shown: KnowledgeGraphNode[]) {
  const byDomain = new Map<string, { docs: KnowledgeGraphNode[]; code: KnowledgeGraphNode[] }>();
  for (const d of graph.domains) byDomain.set(d.id, { docs: [], code: [] });
  for (const n of shown) {
    if (!byDomain.has(n.domain)) byDomain.set(n.domain, { docs: [], code: [] });
    byDomain.get(n.domain)![n.kind === "doc" ? "docs" : "code"].push(n);
  }
  const links = new Map<string, string[]>();
  for (const e of graph.edges) {
    links.set(e.to, [...(links.get(e.to) ?? []), e.from]);
    links.set(e.from, [...(links.get(e.from) ?? []), e.to]);
  }
  for (const g of byDomain.values()) {
    g.docs.sort((a, b) => b.degree - a.degree || a.label.localeCompare(b.label));
    const rank = new Map(g.docs.map((d, i) => [d.id, i]));
    const at = (c: KnowledgeGraphNode) => {
      const r = (links.get(c.id) ?? []).map((id) => rank.get(id)).filter((x): x is number => x !== undefined);
      return r.length ? r.reduce((a, b) => a + b, 0) / r.length : 1e6;
    };
    g.code.sort((a, b) => at(a) - at(b) || a.path.localeCompare(b.path));
  }
  return [...byDomain.entries()].filter(([, g]) => g.docs.length + g.code.length > 0);
}

const domainName = (id: string) => id.replace(/^docs: /, "").toUpperCase();

export function place(mode: Exclude<Mode, "free">, graph: KnowledgeGraph, shown: KnowledgeGraphNode[], color: (domain: string) => string): Placed {
  const doms = groups(graph, shown);
  const pos = new Map<string, [number, number]>();
  const side = new Map<string, LabelSide>();
  const guides: Guide[] = [];

  if (mode === "graph") {
    const X = { domain: -720, doc: -260, code: 260 };
    const head = (x: number, text: string) => guides.push({ t: "text", x, y: -60, text, color: "#8b95a5", size: 10, align: "center", alpha: 0.9 });
    head(X.domain, "DOMAINS");
    head(X.doc, "DOCUMENTS");
    head(X.code, "CODE FILES");
    let y = 0;
    doms.forEach(([id, g], i) => {
      const h = Math.max(g.docs.length * 26, g.code.length * 20, 40) + 30;
      const col = color(id);
      if (i % 2 === 0) guides.push({ t: "band", x: X.domain - 120, y: y - 8, w: X.code - X.domain + 420, h: h - 4, color: col });
      guides.push({ t: "text", x: X.domain - 100, y: y + 14, text: domainName(id), color: col, size: 11, align: "left", alpha: 0.9 });
      guides.push({ t: "text", x: X.domain - 100, y: y + 30, text: `${g.docs.length} docs · ${g.code.length} code`, color: "#8b95a5", size: 9.5, align: "left", alpha: 0.8 });
      const spread = (list: KnowledgeGraphNode[], x: number, step: number, s: LabelSide) => {
        const top = y + 14;
        list.forEach((n, k) => {
          pos.set(n.id, [x, top + k * step]);
          side.set(n.id, s);
        });
      };
      spread(g.docs, X.doc, 26, "left");
      spread(g.code, X.code, 20, "right");
      y += h;
    });
    return { pos, guides, side, curved: true };
  }

  if (mode === "circle") {
    const docs = doms.reduce((a, [, g]) => a + g.docs.length, 0);
    const code = doms.reduce((a, [, g]) => a + g.code.length, 0);
    const R1 = Math.max(260, (docs * 22) / (2 * Math.PI));
    const R2 = Math.max(R1 + 170, (code * 16) / (2 * Math.PI));
    guides.push({ t: "ellipse", x: 0, y: 0, rx: R1, ry: R1, color: "#8b95a5", alpha: 0.18 });
    guides.push({ t: "ellipse", x: 0, y: 0, rx: R2, ry: R2, color: "#8b95a5", alpha: 0.18 });
    guides.push({ t: "text", x: 0, y: -R1 + 18, text: "DOCUMENTS", color: "#8b95a5", size: 9.5, align: "center", alpha: 0.8 });
    guides.push({ t: "text", x: 0, y: -R2 + 18, text: "CODE FILES", color: "#8b95a5", size: 9.5, align: "center", alpha: 0.8 });
    // Slices as wide as the domain's bigger ring needs, with a gap between domains.
    const weight = (g: { docs: unknown[]; code: unknown[] }) => Math.max(g.docs.length / Math.max(1, docs), g.code.length / Math.max(1, code), 0.02);
    const total = doms.reduce((a, [, g]) => a + weight(g), 0) || 1;
    const gap = doms.length > 1 ? 0.06 : 0;
    const usable = Math.PI * 2 - gap * doms.length;
    let a = -Math.PI / 2 + gap / 2;
    for (const [id, g] of doms) {
      const arc = (weight(g) / total) * usable;
      const col = color(id);
      const ring = (list: KnowledgeGraphNode[], r: number, s: LabelSide) =>
        list.forEach((n, k) => {
          const t = a + (arc * (k + 0.5)) / list.length;
          pos.set(n.id, [Math.cos(t) * r, Math.sin(t) * r]);
          side.set(n.id, s);
        });
      ring(g.docs, R1, "in");
      ring(g.code, R2, "out");
      guides.push({ t: "arc", x: 0, y: 0, r: R1 - 150, a0: a, a1: a + arc, color: col, alpha: 0.7, width: 4 });
      const mid = a + arc / 2;
      guides.push({ t: "text", x: Math.cos(mid) * (R1 - 175), y: Math.sin(mid) * (R1 - 175), text: domainName(id), color: col, size: 10, align: "center", alpha: 0.95, room: arc * (R1 - 175) });
      a += arc + gap;
    }
    return { pos, guides, side, curved: true };
  }

  // Orbit: one system per domain; the biggest sits in the middle.
  const size = ([, g]: (typeof doms)[number]) => {
    const r1 = 40 + Math.max(g.docs.length, 1) * 7;
    const r2 = r1 + 40 + g.code.length * 3.2;
    return { r1, r2: g.code.length ? r2 : r1 };
  };
  const sorted = [...doms].sort((x, y) => y[1].docs.length + y[1].code.length - (x[1].docs.length + x[1].code.length));
  const sizes = sorted.map(size);
  const rest = sizes.slice(1);
  const ringR = sorted.length > 1 ? Math.max(sizes[0].r2 + Math.max(...rest.map((s) => s.r2)) + 80, rest.reduce((a, s) => a + 2 * s.r2 + 60, 0) / (2 * Math.PI)) : 0;
  if (ringR) guides.push({ t: "ellipse", x: 0, y: 0, rx: ringR, ry: ringR * TILT, color: "#8b95a5", alpha: 0.12 });
  sorted.forEach(([id, g], i) => {
    const { r1, r2 } = sizes[i];
    const t = i === 0 ? 0 : -Math.PI / 2 + ((i - 1) / Math.max(1, sorted.length - 1)) * Math.PI * 2;
    const cx = i === 0 ? 0 : Math.cos(t) * ringR;
    const cy = i === 0 ? 0 : Math.sin(t) * ringR * TILT;
    const col = color(id);
    guides.push({ t: "ellipse", x: cx, y: cy, rx: r1, ry: r1 * TILT, color: col, alpha: 0.35 });
    if (g.code.length) guides.push({ t: "ellipse", x: cx, y: cy, rx: r2, ry: r2 * TILT, color: col, alpha: 0.2 });
    guides.push({ t: "text", x: cx, y: cy, text: domainName(id), color: col, size: 11, align: "center", alpha: 0.95, room: 2 * r1 });
    const orbit = (list: KnowledgeGraphNode[], r: number, phase: number) =>
      list.forEach((n, k) => {
        const u = phase + (k / list.length) * Math.PI * 2;
        pos.set(n.id, [cx + Math.cos(u) * r, cy + Math.sin(u) * r * TILT]);
        side.set(n.id, "free");
      });
    orbit(g.docs, r1, -Math.PI / 2);
    orbit(g.code, r2, -Math.PI / 2 + 0.2);
  });
  return { pos, guides, side, curved: false };
}

/** The box around every placed node, for fitting the view. */
export function bounds(pos: Map<string, [number, number]>): { x0: number; y0: number; x1: number; y1: number } | null {
  if (!pos.size) return null;
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  for (const [x, y] of pos.values()) {
    x0 = Math.min(x0, x);
    y0 = Math.min(y0, y);
    x1 = Math.max(x1, x);
    y1 = Math.max(y1, y);
  }
  return { x0, y0, x1, y1 };
}

/** Draws the guides; `k` is the view's zoom, so text keeps its screen size. */
export function drawGuides(ctx: CanvasRenderingContext2D, guides: Guide[], k: number, rgba: (hex: string, a: number) => string) {
  for (const g of guides) {
    if (g.t === "band") {
      ctx.fillStyle = rgba(g.color, 0.035);
      ctx.fillRect(g.x, g.y, g.w, g.h);
    } else if (g.t === "ellipse") {
      ctx.beginPath();
      ctx.ellipse(g.x, g.y, g.rx, g.ry, 0, 0, Math.PI * 2);
      ctx.strokeStyle = rgba(g.color, g.alpha);
      ctx.lineWidth = 1 / k;
      ctx.stroke();
    } else if (g.t === "arc") {
      ctx.beginPath();
      ctx.arc(g.x, g.y, g.r, g.a0, g.a1);
      ctx.strokeStyle = rgba(g.color, g.alpha);
      ctx.lineWidth = g.width / k;
      ctx.stroke();
    } else {
      ctx.font = `600 ${g.size / k}px ${mono()}`;
      // A name wider than the room it has (a thin slice, a small system) waits for a closer zoom.
      if (g.room !== undefined && g.room < ctx.measureText(g.text).width) continue;
      ctx.textAlign = g.align;
      ctx.textBaseline = "middle";
      ctx.fillStyle = rgba(g.color, g.alpha);
      ctx.fillText(g.text, g.x, g.y);
    }
  }
}
