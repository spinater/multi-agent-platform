import type { Event } from "@/lib/generated/Event";
import type { PipelineInfo } from "@/lib/generated/PipelineInfo";
import type { LaneInfo } from "@/lib/generated/LaneInfo";
import type { Session } from "@/lib/generated/Session";
import type { Task } from "@/lib/generated/Task";

/** The command centre's graph (design: Agent Command Centre.dc.html, `sync()`): the orchestrator, one hub per
 * provider (or per project in all-projects mode), sessions, tasks, the pipeline stages from `map.toml`, and one dot
 * per recent event of a session ("runs"). Colours are CSS colour strings; the canvas resolves `var(...)`. */

export type NodeType = "orch" | "group" | "session" | "task" | "stage" | "run";
export type LinkKind = "hub" | "own" | "work" | "dep" | "stage" | "infra";
export type Phase = "verify" | "audit" | "land" | "post_done" | "terminal" | "store" | "hub";

/** A provider hub, the queue of unassigned tasks, or (all projects) a project. */
export type Group = { key: string; label: string; color: string; kind: "provider" | "queue" | "project"; available?: boolean };

export type Stage = { id: string; label: string; glyph: string; phase: Phase; command?: string };

export type Ref =
  | { kind: "orch" }
  | { kind: "group"; group: Group }
  | { kind: "session"; session: Session }
  | { kind: "task"; task: Task }
  | { kind: "stage"; stage: Stage }
  | { kind: "run"; event: Event };

export type GraphNode = { id: string; type: NodeType; group: string | null; label: string; color: string; parent: string | null; ref: Ref };
export type GraphLink = { id: string; a: string; b: string; kind: LinkKind; active: boolean };

export type Graph = {
  nodes: GraphNode[];
  links: GraphLink[];
  groups: Group[];
  /** Sessions and tasks per group, for the chips and the hub labels. */
  count: Record<string, number>;
  activeStages: Set<string>;
  /** Dependency depth of each task id (0 = no dependencies in view), for the graph view's columns. */
  depth: Record<number, number>;
  stages: Stage[];
};

export const QUEUE = "queue";
const WORKING = new Set(["working", "starting"]);

/** A short glyph for a command: `bun run typecheck` → `Ty`, `git commit -am …` → `C`. Unique within `taken`. */
function glyphFor(command: string, taken: Set<string>): string {
  const words = command
    .split(/\s+/)
    .filter((w) => /^[a-z]/i.test(w) && !["bun", "npm", "pnpm", "yarn", "run", "cargo", "git", "make", "npx", "bunx", "uv", "python", "python3", "sh", "bash", "-c"].includes(w));
  const word = (words[0] ?? command.replace(/[^a-z]/gi, "") ?? "x").replace(/[^a-z]/gi, "") || "x";
  for (let n = 1; n <= Math.min(3, word.length); n++) {
    const g = word[0].toUpperCase() + word.slice(1, n).toLowerCase();
    if (!taken.has(g)) {
      taken.add(g);
      return g;
    }
  }
  let i = 2;
  while (taken.has(`${word[0].toUpperCase()}${i}`)) i++;
  taken.add(`${word[0].toUpperCase()}${i}`);
  return `${word[0].toUpperCase()}${i}`;
}

/** The pipeline column: each verify command, the audit, landing the task's branch (when some task has one), each
 * post_done hook, then the platform's own plumbing. */
export function buildStages(pipeline: PipelineInfo | null, database: string | undefined, branches = false): Stage[] {
  const taken = new Set<string>(["A", "L", "tm", "db", "ws"]);
  const out: Stage[] = [];
  const verify = pipeline?.verify ?? [];
  if (verify.length === 0) out.push({ id: "verify", label: "verify · none configured", glyph: "V", phase: "verify" });
  verify.forEach((c, i) => out.push({ id: `verify-${i}`, label: c, glyph: glyphFor(c, taken), phase: "verify", command: c }));
  if (pipeline?.audit_enabled) out.push({ id: "audit", label: `audit · ${pipeline.audit_chain.join(" → ")}`, glyph: "A", phase: "audit" });
  if (branches) out.push({ id: "land", label: "land · map/task-N → base", glyph: "L", phase: "land" });
  (pipeline?.post_done ?? []).forEach((c, i) => out.push({ id: `post-${i}`, label: c, glyph: glyphFor(c, taken), phase: "post_done", command: c }));
  out.push({ id: "tmux", label: "tmux · send-keys / capture-pane", glyph: "tm", phase: "terminal" });
  out.push({ id: "sqlite", label: `sqlite · ${database ?? ".map/map.db"}`, glyph: "db", phase: "store" });
  out.push({ id: "ws", label: "/api/ws push", glyph: "ws", phase: "hub" });
  return out;
}

export function taskDepths(tasks: Task[]): Record<number, number> {
  const memo: Record<number, number> = {};
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const depth = (id: number, guard: number): number => {
    if (memo[id] != null) return memo[id];
    const t = byId.get(id);
    if (!t || guard > 30) return 0;
    const deps = t.depends_on.filter((d) => byId.has(d));
    const d = deps.length ? 1 + Math.max(...deps.map((x) => depth(x, guard + 1))) : 0;
    memo[id] = d;
    return d;
  };
  tasks.forEach((t) => depth(t.id, 0));
  return memo;
}

export type BuildInput = {
  sessions: Session[];
  tasks: Task[];
  events: Event[];
  groups: Group[];
  stages: Stage[];
  /** Group key of a session (its provider, or its project in all-projects mode). */
  groupOfSession: (s: Session) => string;
  /** Group key of a task without a known session (the queue, or its project). */
  groupOfLooseTask: (t: Task) => string;
  taskLabel: (t: Task) => string;
  sessionLabel: (s: Session) => string;
  /** At most this many run dots, newest first. */
  maxRuns?: number;
  /** Lanes (agent sessions MAP watches, `lanes.rs`) as session-like nodes `l:<name>` under `groupOfLane`. */
  lanes?: { lane: LaneInfo; session: Session }[];
  groupOfLane?: (l: LaneInfo) => string;
};

const LANE_STATE: Record<string, Session["state"]> = { busy: "working", idle: "idle", waiting: "waiting_input", pending_input: "waiting_input", rate_limited: "rate_limited", missing: "exited" };

/** A lane as a session, so the canvases draw it like one: its loudest tmux session's state. */
export function laneSession(l: LaneInfo): Session {
  const order = ["missing", "waiting", "pending_input", "rate_limited", "busy", "idle"];
  const worst = order.find((st) => l.sessions.some((x) => x.state === st));
  const state: Session["state"] = l.stopped || !l.sessions.length ? "exited" : (LANE_STATE[worst ?? "idle"] ?? "idle");
  const now = l.checked_at ?? new Date(0).toISOString();
  return {
    name: l.name,
    provider: "claude",
    model: l.role ?? "lane",
    role: "manual",
    task_id: null,
    state,
    detail: l.sessions.map((x) => `${x.name}: ${x.why}`).join(" · ") || null,
    rate_limit_reset_at: null,
    wakeups: 0,
    last_line: null,
    attached: l.sessions.some((x) => x.attached),
    created_at: now,
    last_activity_at: now,
    project: l.project,
    spawned_by: "lane",
  };
}

/** A lane's sub-agent row (`◯ card-475-browser-hit-test ▱▱▱ 2m46s`) as its name and time. */
export function subAgentLabel(row: string): string {
  return row.replace(/^[●◯○]\s*/, "").replace(/[▱▰]+/g, " ").replace(/\s+/g, " ").trim();
}

/** A lane's sub-agents: `a:<lane>:<i>`, on the task ring under the lane, drawn as working sessions. */
function subAgents(lane: LaneInfo, session: Session): { id: string; label: string; session: Session }[] {
  const rows = lane.sessions.flatMap((x) => x.sub_agents);
  return rows.map((row, i) => {
    const label = subAgentLabel(row);
    return { id: `a:${lane.name}:${i}`, label, session: { ...session, name: `${lane.name} › ${label.split(" ")[0]}`, state: "working", detail: `sub-agent of ${lane.name}: ${label}`, spawned_by: lane.name } };
  });
}

export function buildGraph(input: BuildInput): Graph {
  const { sessions, tasks, events, groups, stages } = input;
  const color = new Map(groups.map((g) => [g.key, g.color]));
  const gcolor = (g: string) => color.get(g) ?? "var(--p-other)";
  const byName = new Map(sessions.map((s) => [s.name, s]));
  const nodes: GraphNode[] = [{ id: "orch", type: "orch", group: null, label: "MAP · ORCHESTRATOR", color: "var(--accent)", parent: null, ref: { kind: "orch" } }];
  for (const g of groups) nodes.push({ id: `p:${g.key}`, type: "group", group: g.key, label: g.label.toUpperCase(), color: g.color, parent: "orch", ref: { kind: "group", group: g } });
  for (const s of sessions) {
    const g = input.groupOfSession(s);
    nodes.push({ id: `s:${s.name}`, type: "session", group: g, label: input.sessionLabel(s), color: gcolor(g), parent: `p:${g}`, ref: { kind: "session", session: s } });
  }
  const taskGroup = (t: Task) => {
    const s = t.session ? byName.get(t.session) : undefined;
    return s ? input.groupOfSession(s) : input.groupOfLooseTask(t);
  };
  for (const t of tasks) {
    const s = t.session ? byName.get(t.session) : undefined;
    const g = taskGroup(t);
    nodes.push({ id: `t:${t.id}`, type: "task", group: g, label: `${input.taskLabel(t)} ${t.title}`, color: gcolor(g), parent: s ? `s:${s.name}` : `p:${g}`, ref: { kind: "task", task: t } });
  }
  for (const { lane, session } of input.lanes ?? []) {
    const g = input.groupOfLane?.(lane) ?? "lanes";
    nodes.push({ id: `l:${lane.name}`, type: "session", group: g, label: lane.label ?? lane.name, color: gcolor(g), parent: `p:${g}`, ref: { kind: "session", session } });
    for (const a of subAgents(lane, session)) nodes.push({ id: a.id, type: "task", group: g, label: a.label, color: gcolor(g), parent: `l:${lane.name}`, ref: { kind: "session", session: a.session } });
  }
  for (const st of stages) nodes.push({ id: `g:${st.id}`, type: "stage", group: null, label: st.label, color: "var(--text-2)", parent: "orch", ref: { kind: "stage", stage: st } });
  let runs = 0;
  for (const e of events) {
    if (runs >= (input.maxRuns ?? 320)) break;
    const s = e.session ? byName.get(e.session) : undefined;
    if (!s) continue;
    const g = input.groupOfSession(s);
    nodes.push({ id: `r:${e.id}`, type: "run", group: g, label: `${e.kind} · ${s.name}`, color: gcolor(g), parent: `s:${s.name}`, ref: { kind: "run", event: e } });
    runs++;
  }

  const links: GraphLink[] = [];
  const add = (a: string, b: string, kind: LinkKind, active: boolean) => links.push({ id: `${a}>${b}`, a, b, kind, active });
  const working = new Set(sessions.filter((s) => WORKING.has(s.state)).map((s) => s.name));
  const activeGroups = new Set(sessions.filter((s) => working.has(s.name)).map(input.groupOfSession));
  for (const { lane, session } of input.lanes ?? []) if (session.state === "working") activeGroups.add(input.groupOfLane?.(lane) ?? "lanes");
  const taskIds = new Set(tasks.map((t) => t.id));
  for (const g of groups) add("orch", `p:${g.key}`, "hub", activeGroups.has(g.key));
  for (const s of sessions) add(`p:${input.groupOfSession(s)}`, `s:${s.name}`, "own", working.has(s.name));
  for (const { lane, session } of input.lanes ?? []) add(`p:${input.groupOfLane?.(lane) ?? "lanes"}`, `l:${lane.name}`, "own", session.state === "working");
  for (const { lane, session } of input.lanes ?? []) for (const a of subAgents(lane, session)) add(`l:${lane.name}`, a.id, "work", true);
  for (const s of sessions) if (s.task_id != null && taskIds.has(s.task_id)) add(`s:${s.name}`, `t:${s.task_id}`, "work", working.has(s.name));
  for (const t of tasks) if (!t.session || !byName.has(t.session)) add(`p:${taskGroup(t)}`, `t:${t.id}`, "own", false);
  for (const t of tasks) for (const d of t.depends_on) if (taskIds.has(d)) add(`t:${d}`, `t:${t.id}`, "dep", false);
  const activeStages = new Set<string>();
  for (const t of tasks) {
    const phase = t.status === "verifying" ? "verify" : t.status === "auditing" ? "audit" : t.status === "landing" ? "land" : null;
    if (!phase) continue;
    for (const st of stages.filter((x) => x.phase === phase)) {
      add(`t:${t.id}`, `g:${st.id}`, "stage", true);
      activeStages.add(`g:${st.id}`);
    }
  }
  for (const st of stages) if (st.phase === "terminal" || st.phase === "store" || st.phase === "hub") add("orch", `g:${st.id}`, "infra", false);

  const count: Record<string, number> = {};
  for (const n of nodes) if ((n.type === "session" || n.type === "task") && n.group) count[n.group] = (count[n.group] ?? 0) + 1;
  return { nodes, links, groups, count, activeStages, depth: taskDepths(tasks), stages };
}
