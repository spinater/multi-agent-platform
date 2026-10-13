import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type SimulationNodeDatum } from "d3-force";

import { LAYERS, type Graph, type GraphNode } from "./model";

/** Every tunable number of the graph lives here. */
export const TUNING = {
  /** Fraction of the remaining distance a node travels per frame when the view changes. */
  ease: 0.12,
  /** Radians per second the 3D orbit turns while motion is on. */
  orbitSpeed: 0.18,
  /** Camera distance for the orbit's perspective; smaller is stronger. */
  orbitDepth: 2.6,
  /** Ring radii in the rings view, core outwards (unit space, 1 = edge). */
  ringRadii: [0, 0.22, 0.4, 0.58, 0.76, 0.94],
  /** Force layout ticks, computed once per graph change. */
  forceTicks: 220,
  /** Dot radius in px: base plus a step per square root of link count. */
  dotBase: 2.4,
  dotPerLink: 1.1,
  /** Opacity of links normally, and of everything outside the hovered neighbourhood. */
  linkAlpha: 0.14,
  dimAlpha: 0.1,
  /** px within which the pointer picks a node. */
  hitRadius: 10,
  /** Gentle idle drift in px for the flat views while motion is on. */
  breathe: 1.2,
} as const;

export const VIEWS = ["rings", "circle", "areas", "links", "timeline", "orbit"] as const;
export type View = (typeof VIEWS)[number];

export type Point = { x: number; y: number; z: number };
export type Layout = Map<string, Point>;

/** Nodes sorted so each area forms a contiguous run, which turns rings into area sectors. */
function byArea(nodes: GraphNode[], areas: string[]): GraphNode[] {
  return nodes.slice().sort((a, b) => areas.indexOf(a.area) - areas.indexOf(b.area) || a.id.localeCompare(b.id));
}

function ring(nodes: GraphNode[], radius: number, out: Layout, offset = -Math.PI / 2) {
  nodes.forEach((n, i) => {
    const a = offset + (2 * Math.PI * (i + 0.5)) / Math.max(1, nodes.length);
    out.set(n.id, { x: radius * Math.cos(a), y: radius * Math.sin(a), z: 0 });
  });
}

function rings(g: Graph): Layout {
  const out: Layout = new Map();
  LAYERS.forEach((layer, i) => {
    const members = byArea(g.nodes.filter((n) => n.layer === layer), g.areas);
    if (layer === "core") members.forEach((n) => out.set(n.id, { x: 0, y: 0, z: 0 }));
    else ring(members, TUNING.ringRadii[i], out);
  });
  return out;
}

function circle(g: Graph): Layout {
  const out: Layout = new Map();
  const others = byArea(g.nodes.filter((n) => n.kind !== "root"), g.areas);
  ring(others, 0.9, out);
  g.nodes.filter((n) => n.kind === "root").forEach((n) => out.set(n.id, { x: 0, y: 0, z: 0 }));
  return out;
}

/** One cluster per area on a ring, each a sunflower spiral; the core in the middle. */
function areas(g: Graph): Layout {
  const out: Layout = new Map();
  const golden = Math.PI * (3 - Math.sqrt(5));
  const n = Math.max(1, g.areas.length);
  g.areas.forEach((area, ai) => {
    const a = -Math.PI / 2 + (2 * Math.PI * ai) / n;
    const cx = n === 1 ? 0 : 0.58 * Math.cos(a);
    const cy = n === 1 ? 0 : 0.58 * Math.sin(a);
    // Hubs (projects, providers) sit at the cluster's centre.
    const members = g.nodes.filter((x) => x.area === area).sort((p, q) => Number(q.kind === "project") - Number(p.kind === "project") || p.id.localeCompare(q.id));
    const spread = Math.min(0.3, 0.05 + 0.025 * Math.sqrt(members.length));
    members.forEach((m, i) => {
      const r = spread * Math.sqrt(i / Math.max(1, members.length - 1));
      out.set(m.id, { x: cx + r * Math.cos(i * golden), y: cy + r * Math.sin(i * golden), z: 0 });
    });
  });
  g.nodes.filter((x) => x.kind === "root").forEach((x) => out.set(x.id, { x: 0, y: 0, z: 0 }));
  return out;
}

type SimNode = SimulationNodeDatum & { id: string };

function links(g: Graph): Layout {
  const sim: SimNode[] = g.nodes.map((n, i) => ({ id: n.id, x: Math.cos(i) * 50, y: Math.sin(i) * 50 }));
  const simulation = forceSimulation(sim)
    .force("link", forceLink(g.links.map((l) => ({ source: l.s, target: l.t }))).id((d) => (d as SimNode).id).distance(28).strength(0.6))
    .force("charge", forceManyBody().strength(-40))
    .force("collide", forceCollide(6))
    .force("x", forceX(0).strength(0.04))
    .force("y", forceY(0).strength(0.04))
    .stop();
  for (let i = 0; i < TUNING.forceTicks; i++) simulation.tick();
  const extent = Math.max(1, ...sim.map((d) => Math.hypot(d.x ?? 0, d.y ?? 0)));
  const out: Layout = new Map();
  sim.forEach((d) => out.set(d.id, { x: ((d.x ?? 0) / extent) * 0.92, y: ((d.y ?? 0) / extent) * 0.92, z: 0 }));
  return out;
}

/** Left to right by time; one horizontal lane per layer. */
function timeline(g: Graph): Layout {
  const out: Layout = new Map();
  const times = g.nodes.map((n) => n.at).filter((t) => t > 0);
  const lo = Math.min(...times, Date.now());
  const hi = Math.max(...times, lo + 1);
  LAYERS.forEach((layer, li) => {
    const members = g.nodes.filter((n) => n.layer === layer).sort((a, b) => a.at - b.at);
    const y = -0.85 + (1.7 * li) / (LAYERS.length - 1);
    members.forEach((n, i) => {
      const x = -0.92 + (1.84 * (n.at - lo)) / (hi - lo);
      // Stagger same-time nodes vertically so they stay readable.
      out.set(n.id, { x, y: y + ((i % 3) - 1) * 0.045, z: 0 });
    });
  });
  return out;
}

/** Points on a sphere (Fibonacci), area-ordered so areas form bands; rotated and projected at draw time. */
function orbit(g: Graph): Layout {
  const out: Layout = new Map();
  const others = byArea(g.nodes.filter((n) => n.kind !== "root"), g.areas);
  const n = Math.max(1, others.length);
  others.forEach((node, i) => {
    const y = 1 - (2 * (i + 0.5)) / n;
    const r = Math.sqrt(1 - y * y);
    const a = i * Math.PI * (3 - Math.sqrt(5));
    out.set(node.id, { x: 0.85 * r * Math.cos(a), y: 0.85 * y, z: 0.85 * r * Math.sin(a) });
  });
  g.nodes.filter((x) => x.kind === "root").forEach((x) => out.set(x.id, { x: 0, y: 0, z: 0 }));
  return out;
}

export function layout(view: View, g: Graph): Layout {
  switch (view) {
    case "rings":
      return rings(g);
    case "circle":
      return circle(g);
    case "areas":
      return areas(g);
    case "links":
      return links(g);
    case "timeline":
      return timeline(g);
    case "orbit":
      return orbit(g);
  }
}

/** Rotates around the vertical axis, tilts slightly, and applies perspective. Returns screen-unit x, y and depth. */
export function project3d(p: Point, angle: number): { x: number; y: number; depth: number } {
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const x = p.x * cos - p.z * sin;
  const z = p.x * sin + p.z * cos;
  const tilt = 0.32;
  const y = p.y * Math.cos(tilt) - z * Math.sin(tilt);
  const zt = p.y * Math.sin(tilt) + z * Math.cos(tilt);
  const s = TUNING.orbitDepth / (TUNING.orbitDepth - zt);
  return { x: x * s, y: y * s, depth: zt };
}
