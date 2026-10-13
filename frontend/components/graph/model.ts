import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Session } from "@/lib/generated/Session";
import type { Task } from "@/lib/generated/Task";

/** What a node is. Shape follows kind; colour follows area. */
export type Kind = "root" | "project" | "domain" | "task" | "session" | "provider";

/** Concentric ring a node sits on in the rings view, innermost first. */
export const LAYERS = ["core", "projects", "corpus", "tasks", "sessions", "providers"] as const;
export type Layer = (typeof LAYERS)[number];

export const LAYER_OF: Record<Kind, Layer> = {
  root: "core",
  project: "projects",
  domain: "corpus",
  task: "tasks",
  session: "sessions",
  provider: "providers",
};

/** The area of tasks and sessions that run in `workspace.dir` rather than a project. */
export const WORKSPACE_AREA = "workspace";
export const PROVIDER_AREA = "providers";

export type GraphNode = {
  id: string;
  kind: Kind;
  label: string;
  area: string;
  layer: Layer;
  note: string;
  /** Epoch ms, for the timeline view. */
  at: number;
  /** Status word shown on the card (task status, session state, Reflex state…). */
  status?: string;
  /** The record behind the node, for card actions. */
  ref: string;
};

export type GraphLink = { s: string; t: string };

export type Graph = { nodes: GraphNode[]; links: GraphLink[]; areas: string[] };

const ms = (iso: string | null | undefined) => (iso ? new Date(iso).getTime() : 0);

/** Builds one graph of the whole platform: the core, projects and their corpus domains, tasks, sessions, providers. */
export function buildGraph(projects: ProjectInfo[], tasks: Task[], sessions: Session[], providers: ProviderInfo[]): Graph {
  const nodes: GraphNode[] = [];
  const links: GraphLink[] = [];
  const ids = new Set<string>();
  const add = (n: GraphNode) => {
    if (ids.has(n.id)) return;
    ids.add(n.id);
    nodes.push(n);
  };
  const link = (s: string, t: string) => {
    if (ids.has(s) && ids.has(t)) links.push({ s, t });
  };

  const first = Math.min(...[...projects.map((p) => ms(p.project.created_at)), ...tasks.map((t) => ms(t.created_at)), Date.now()]);
  add({ id: "root", kind: "root", label: "map", area: "core", layer: "core", note: "The platform: orchestrator, watcher and decision gate", at: first, ref: "" });

  for (const { project: p, reflex, stats } of projects) {
    const id = `project:${p.slug}`;
    add({
      id,
      kind: "project",
      label: p.slug,
      area: p.slug,
      layer: "projects",
      note: `${p.description || p.name} · ${stats.total} decisions, ${stats.reflex} by Reflex`,
      at: ms(p.created_at),
      status: reflex.state,
      ref: p.slug,
    });
    link("root", id);
    for (const d of reflex.domains) {
      const did = `domain:${p.slug}:${d}`;
      add({ id: did, kind: "domain", label: d, area: p.slug, layer: "corpus", note: `Corpus domain of ${p.slug}`, at: ms(reflex.since), ref: `${p.slug}/${d}` });
      link(id, did);
    }
  }

  for (const t of tasks) {
    const area = t.project ?? WORKSPACE_AREA;
    const id = `task:${t.id}`;
    add({ id, kind: "task", label: `#${t.id} ${t.title}`, area, layer: "tasks", note: t.result?.split("\n")[0] ?? t.prompt.slice(0, 140), at: ms(t.created_at), status: t.status, ref: String(t.id) });
    link(t.project ? `project:${t.project}` : "root", id);
  }
  for (const t of tasks) for (const d of t.depends_on) link(`task:${d}`, `task:${t.id}`);

  for (const p of providers) {
    const id = `provider:${p.name}`;
    add({ id, kind: "provider", label: p.name, area: PROVIDER_AREA, layer: "providers", note: `${p.models.join(", ") || p.default_model}`, at: first, status: !p.available ? "missing" : p.cooldown_until ? "cooldown" : "ready", ref: p.name });
    link("root", id);
  }

  for (const s of sessions) {
    const id = `session:${s.name}`;
    add({ id, kind: "session", label: s.name, area: s.project ?? WORKSPACE_AREA, layer: "sessions", note: s.last_line ?? s.detail ?? `${s.provider} · ${s.model}`, at: ms(s.created_at), status: s.state, ref: s.name });
    if (s.task_id !== null) link(`task:${s.task_id}`, id);
    else link(s.project ? `project:${s.project}` : "root", id);
    link(id, `provider:${s.provider}`);
  }

  const areas = [...new Set(nodes.filter((n) => n.kind !== "root").map((n) => n.area))].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
  return { nodes, links, areas };
}

/** Projects first, then the shared workspace, then providers. */
function rank(area: string) {
  return area === PROVIDER_AREA ? 2 : area === WORKSPACE_AREA ? 1 : 0;
}

/** CSS variable for an area's colour; providers and the workspace use the neutral hue. */
export function areaVar(areas: string[], area: string): string {
  if (area === PROVIDER_AREA || area === WORKSPACE_AREA || area === "core") return "--area-0";
  const i = areas.filter((a) => a !== PROVIDER_AREA && a !== WORKSPACE_AREA).indexOf(area);
  return `--area-${(i % 8) + 1}`;
}
