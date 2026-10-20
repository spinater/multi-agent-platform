import type { Graph, GraphNode } from "./model";

/** The two canvas views (design: `layout()` in Agent Command Centre.dc.html). Positions are in a unit space the
 * camera scales to the canvas; orbit uses all three axes, graph lies flat. */
export const VIEWS = ["orbit", "circle", "graph"] as const;
export type View = (typeof VIEWS)[number];

export type Point = [number, number, number];

/** Every number of the layouts, in one place. */
export const TUNING = {
  graph: { orch: -1.5, groups: -1.15, sessions: -0.78, tasks: -0.36, depthStep: 0.29, tasksEnd: 0.95, stages: 1.2, spread: 0.75, sessionSpread: 0.88, taskSpread: 1.7, taskGap: 0.24 },
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

/** The orbit's layers, inside out: each is a flat ring around the orchestrator, drawn as a guide with its name. */
export const ORBIT_RINGS = { group: 0.3, session: 0.55, task: 0.8, run: 1.0, stage: 1.2 } as const;
export type OrbitLayer = keyof typeof ORBIT_RINGS;

/** Orbit positions: every layer on its own ring, every group (provider or project) on its own arc of all of them,
 * so a group's sessions, tasks and runs line up behind it. A group's arc is as wide as its busiest layer needs;
 * inside it, a layer's nodes are spread evenly, ordered by their parent so links stay short and do not cross. */
function orbitLayout(graph: Graph, keys: string[]): Map<string, Point> {
  const out = new Map<string, Point>();
  out.set("orch", [0, 0, 0]);
  const layerOf = (n: GraphNode): OrbitLayer | null => (n.type === "session" || n.type === "task" || n.type === "run" || n.type === "group" ? (n.type as OrbitLayer) : null);
  const byGroup = new Map<string, Record<OrbitLayer, GraphNode[]>>(keys.map((k) => [k, { group: [], session: [], task: [], run: [], stage: [] }]));
  for (const n of graph.nodes) {
    const layer = layerOf(n);
    const g = n.type === "group" ? n.id.slice(2) : n.group;
    if (!layer || g == null) continue;
    if (!byGroup.has(g)) byGroup.set(g, { group: [], session: [], task: [], run: [], stage: [] });
    byGroup.get(g)![layer].push(n);
  }
  // Wider arcs for busier groups (runs count less: they are dots); an empty group still keeps one wide enough for its hub.
  const weight = (l: Record<OrbitLayer, GraphNode[]>) => 2 + Math.max(l.session.length, l.task.length, l.run.length / 4);
  const groups = [...byGroup.entries()];
  const total = groups.reduce((a, [, l]) => a + weight(l), 0) || 1;
  const angle = new Map<string, number>();
  let at = -Math.PI / 2;
  for (const [, layers] of groups) {
    const arc = (weight(layers) / total) * Math.PI * 2;
    const mid = at + arc / 2;
    const place = (layer: OrbitLayer, nodes: GraphNode[]) => {
      const r = ORBIT_RINGS[layer];
      const parentAt = (n: GraphNode) => (n.parent ? (angle.get(n.parent) ?? mid) : mid);
      nodes.sort((a, b) => parentAt(a) - parentAt(b) || (a.id < b.id ? -1 : 1));
      // Keep a gap between arcs, and no wider a spread than the ring has room for at this size.
      const span = Math.min(arc * 0.8, (nodes.length - 1) * (layer === "run" ? 0.05 : 0.32));
      nodes.forEach((n, i) => {
        const t = nodes.length > 1 ? mid - span / 2 + (span * i) / (nodes.length - 1) : mid;
        angle.set(n.id, t);
        out.set(n.id, [Math.cos(t) * r, 0, Math.sin(t) * r]);
      });
    };
    place("group", layers.group);
    place("session", layers.session);
    place("task", layers.task);
    place("run", layers.run);
    at += arc;
  }
  return out;
}

/** Target positions of every node for a view. */
export function layout(graph: Graph, view: View): Map<string, Point> {
  const out = new Map<string, Point>();
  const keys = graph.groups.map((g) => g.key);
  const stages = graph.nodes.filter((n) => n.type === "stage");
  const stageIndex = new Map(stages.map((n, i) => [n.id, i]));
  const SN = Math.max(1, stages.length);

  if (view === "orbit" || view === "circle") {
    for (const [id, pt] of orbitLayout(graph, keys)) out.set(id, pt);
    for (const n of stages) {
      const t = ((stageIndex.get(n.id) ?? 0) / SN) * Math.PI * 2;
      out.set(n.id, [Math.cos(t) * ORBIT_RINGS.stage, 0, Math.sin(t) * ORBIT_RINGS.stage]);
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
  // A lane's sub-agents sit just right of their lane.
  const subs = graph.nodes.filter((n) => n.type === "task" && n.ref.kind === "session" && n.parent);
  subs.forEach((n, i) => {
    const p = out.get(n.parent!);
    const k = subs.slice(0, i).filter((x) => x.parent === n.parent).length;
    if (p) out.set(n.id, [p[0] + 0.16, p[1] + k * 0.07, 0]);
  });
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
