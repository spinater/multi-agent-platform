import type { Graph, GraphNode } from "./model";

/** The two canvas views (design: `layout()` in Agent Command Centre.dc.html). Positions are in a unit space the
 * camera scales to the canvas; orbit uses all three axes, graph lies flat. */
export const VIEWS = ["orbit", "graph"] as const;
export type View = (typeof VIEWS)[number];

export type Point = [number, number, number];

/** Every number of the layouts, in one place. */
export const TUNING = {
  orbit: { hub: 0.46, session: 0.62, sessionJitter: 0.14, task: 0.72, taskJitter: 0.18, run: 0.58, runJitter: 0.24, runMax: 0.96, stageRing: 1.2 },
  graph: { orch: -1.5, groups: -1.15, sessions: -0.78, tasks: -0.36, depthStep: 0.29, tasksEnd: 0.95, stages: 1.2, spread: 0.75, sessionSpread: 0.88, taskSpread: 1.7, taskGap: 0.24 },
  /** Fixed directions for the usual providers in orbit, so they stay put when others come and go. */
  directions: { claude: [-0.72, -0.3, 0.55], codex: [0.7, -0.38, 0.5], gemini: [0.1, 0.66, 0.62], agy: [0.1, 0.66, 0.62], aider: [0.62, 0.36, -0.68], queue: [-0.58, 0.42, -0.66] } as Record<string, Point>,
  spare: [
    [0.05, -0.85, -0.5],
    [-0.9, 0.15, -0.1],
    [0.9, 0.05, -0.1],
    [-0.2, 0.2, -0.95],
  ] as Point[],
} as const;

/** Stable pseudo-random number in [0, 1) from a string, so nodes keep their place across renders. */
export function hash(str: string, k = 0): number {
  let x = 2166136261 ^ Math.imul(k, 374761393);
  for (let i = 0; i < str.length; i++) {
    x ^= str.charCodeAt(i);
    x = Math.imul(x, 16777619);
  }
  x ^= x >>> 13;
  x = Math.imul(x, 0x5bd1e995);
  x ^= x >>> 15;
  return (x >>> 0) / 4294967296;
}

const gauss = (id: string, k: number) => (hash(id, k) + hash(id, k + 1) + hash(id, k + 2) - 1.5) * 2;

function norm(v: Point): Point {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
}

/** Direction of a group on the sphere: a fixed one for known providers, else a Fibonacci point by its index. */
function direction(group: string | null, keys: string[], fixed: boolean): Point {
  if (fixed && group) {
    const known = TUNING.directions[group];
    if (known) return norm(known);
    // Other providers take the spare directions in turn, so they never sit on a usual one.
    const others = keys.filter((k) => !TUNING.directions[k]);
    const i = others.indexOf(group);
    if (i >= 0 && i < TUNING.spare.length) return norm(TUNING.spare[i]);
  }
  const n = Math.max(1, keys.length);
  const i = Math.max(0, group ? keys.indexOf(group) : 0);
  const y = 1 - (2 * (i + 0.5)) / n;
  const r = Math.sqrt(Math.max(0, 1 - y * y));
  const th = i * 2.39996;
  return [Math.cos(th) * r, y * 0.85, Math.sin(th) * r];
}

/** Target positions of every node for a view. `fixedDirections` is off in all-projects mode (groups are projects). */
export function layout(graph: Graph, view: View, fixedDirections: boolean): Map<string, Point> {
  const out = new Map<string, Point>();
  const keys = graph.groups.map((g) => g.key);
  const stages = graph.nodes.filter((n) => n.type === "stage");
  const stageIndex = new Map(stages.map((n, i) => [n.id, i]));
  const SN = Math.max(1, stages.length);

  if (view === "orbit") {
    const o = TUNING.orbit;
    for (const n of graph.nodes) {
      const d = direction(n.group, keys, fixedDirections);
      const G = (k: number) => gauss(n.id, k);
      if (n.type === "orch") out.set(n.id, [0, 0, 0]);
      else if (n.type === "group") out.set(n.id, [d[0] * o.hub, d[1] * o.hub, d[2] * o.hub]);
      else if (n.type === "session") out.set(n.id, [d[0] * o.session + G(1) * o.sessionJitter, d[1] * o.session + G(4) * o.sessionJitter, d[2] * o.session + G(7) * o.sessionJitter]);
      else if (n.type === "task") out.set(n.id, [d[0] * o.task + G(1) * o.taskJitter, d[1] * o.task + G(4) * o.taskJitter, d[2] * o.task + G(7) * o.taskJitter]);
      else if (n.type === "run") {
        let x = d[0] * o.run + G(1) * o.runJitter;
        let y = d[1] * o.run + G(4) * o.runJitter;
        let z = d[2] * o.run + G(7) * o.runJitter;
        const l = Math.hypot(x, y, z);
        if (l > o.runMax) {
          x *= o.runMax / l;
          y *= o.runMax / l;
          z *= o.runMax / l;
        }
        out.set(n.id, [x, y, z]);
      } else if (n.type === "stage") {
        const t = ((stageIndex.get(n.id) ?? 0) / SN) * Math.PI * 2;
        out.set(n.id, [Math.cos(t) * o.stageRing, 0, Math.sin(t) * o.stageRing]);
      }
    }
    return out;
  }

  const g = TUNING.graph;
  const byGroup = (a: GraphNode, b: GraphNode) => keys.indexOf(a.group ?? "") - keys.indexOf(b.group ?? "") || (a.id < b.id ? -1 : 1);
  out.set("orch", [g.orch, 0, 0]);
  keys.forEach((k, i) => out.set(`p:${k}`, [g.groups, keys.length > 1 ? -g.spread + (2 * g.spread * i) / (keys.length - 1) : 0, 0]));
  const sessions = graph.nodes.filter((n) => n.type === "session").sort(byGroup);
  sessions.forEach((n, i) => out.set(n.id, [g.sessions, sessions.length > 1 ? -g.sessionSpread + (2 * g.sessionSpread * i) / (sessions.length - 1) : 0, 0]));
  const cols: Record<number, GraphNode[]> = {};
  for (const n of graph.nodes) {
    if (n.ref.kind !== "task") continue;
    const d = graph.depth[n.ref.task.id] ?? 0;
    (cols[d] ??= []).push(n);
  }
  const step = depthStep(graph);
  for (const [d, arr] of Object.entries(cols)) {
    arr.sort((a, b) => (a.ref.kind === "task" && b.ref.kind === "task" ? a.ref.task.id - b.ref.task.id : 0));
    const sp = Math.min(g.taskGap, g.taskSpread / Math.max(1, arr.length - 1));
    arr.forEach((n, k) => out.set(n.id, [g.tasks + Number(d) * step, (k - (arr.length - 1) / 2) * sp, 0]));
  }
  stages.forEach((n, i) => out.set(n.id, [g.stages, SN > 1 ? -0.8 + (1.6 * i) / (SN - 1) : 0, 0]));
  for (const n of graph.nodes) {
    if (n.type !== "run" || !n.parent) continue;
    const p = out.get(n.parent);
    if (p) out.set(n.id, [p[0] - 0.07 - hash(n.id, 1) * 0.2, p[1] + (hash(n.id, 2) - 0.5) * 0.07, 0]);
  }
  return out;
}

/** Horizontal distance between task columns in the graph view: deep chains squeeze their columns so the last one
 * stays left of the pipeline. */
export function depthStep(graph: Graph): number {
  const g = TUNING.graph;
  const maxDepth = Math.max(0, ...Object.values(graph.depth));
  return maxDepth > 0 ? Math.min(g.depthStep, (g.tasksEnd - g.tasks) / maxDepth) : g.depthStep;
}

/** Column headers of the graph view, at their x positions. */
export function graphColumns(all: boolean): [number, string][] {
  const g = TUNING.graph;
  return [
    [g.orch, "ORCH"],
    [g.groups, all ? "PROJECTS" : "PROVIDERS"],
    [g.sessions, "SESSIONS"],
    [0.22, "TASKS · DEPTH →"],
    [g.stages, "PIPELINE"],
  ];
}
