import type { DecideResponse } from "./generated/DecideResponse";
import type { Decision } from "./generated/Decision";
import type { DecisionCounts } from "./generated/DecisionCounts";
import type { DecisionModel } from "./generated/DecisionModel";
import type { Event } from "./generated/Event";
import type { FitResult } from "./generated/FitResult";
import type { Overview } from "./generated/Overview";
import type { PipelineInfo } from "./generated/PipelineInfo";
import type { ProjectInfo } from "./generated/ProjectInfo";
import type { Question } from "./generated/Question";
import type { ReflexStatus } from "./generated/ReflexStatus";
import type { RepoListing } from "./generated/RepoListing";
import type { Session } from "./generated/Session";
import type { StepResult } from "./generated/StepResult";
import type { SkillFile } from "./generated/SkillFile";
import type { SkillsOverview } from "./generated/SkillsOverview";
import type { Task } from "./generated/Task";

const TOKEN_KEY = "map.apiToken";

export function getToken(): string {
  try {
    return localStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

export function setToken(token: string) {
  try {
    if (token) localStorage.setItem(TOKEN_KEY, token);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Private mode: the token lasts for this page only.
  }
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(`/api${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    cache: "no-store",
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new ApiError(response.status, data.error ?? response.statusText);
  return data as T;
}

/** In `next dev` the page is served on :3000 and cannot proxy WebSockets, so it talks to the backend directly. */
export function wsUrl(): string {
  const direct = process.env.NODE_ENV === "development" ? (process.env.NEXT_PUBLIC_MAP_BACKEND_WS ?? "ws://127.0.0.1:4400") : null;
  const base = direct ?? `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}`;
  const token = getToken();
  return `${base}/api/ws${token ? `?token=${encodeURIComponent(token)}` : ""}`;
}

export const api = {
  overview: () => request<Overview>("GET", "/overview"),
  events: (limit = 150) => request<Event[]>("GET", `/events?limit=${limit}`),
  pane: (name: string, lines = 400) => request<{ name: string; text: string }>("GET", `/sessions/${encodeURIComponent(name)}/pane?lines=${lines}`),
  createSession: (body: { provider: string; model?: string; name?: string; prompt?: string; supervisor?: boolean; project?: string }) =>
    request<Session>("POST", "/sessions", body),
  deleteSession: (name: string, force = false) =>
    request<{ message: string }>("DELETE", `/sessions/${encodeURIComponent(name)}${force ? "?force=true" : ""}`),
  prompt: (name: string, text: string) => request("POST", `/sessions/${encodeURIComponent(name)}/prompt`, { text }),
  key: (name: string, key: string) => request("POST", `/sessions/${encodeURIComponent(name)}/key`, { key }),
  createTask: (body: { title: string; prompt: string; depends_on: number[]; provider?: string; model?: string; project?: string }) =>
    request<Task>("POST", "/tasks", body),
  taskAction: (id: number, action: "retry" | "cancel") => request<Task>("PATCH", `/tasks/${id}`, { action }),
  deleteTask: (id: number) => request("DELETE", `/tasks/${id}`),
  setOrchestrator: (running: boolean) => request("POST", "/orchestrator", { running }),
  /** The orchestrator's verify / audit / post_done settings (command centre). */
  pipeline: () => request<PipelineInfo>("GET", "/pipeline"),

  createProject: (body: { name: string; slug?: string; description?: string; fallback?: string; model?: string }) =>
    request<ProjectInfo>("POST", "/projects", body),
  updateProject: (slug: string, body: ProjectUpdate) => request<ProjectInfo>("PATCH", `/projects/${slug}`, body),
  deleteProject: (slug: string) => request("DELETE", `/projects/${slug}`),
  repo: (slug: string, repo: RepoName) => request<RepoListing>("GET", `/projects/${slug}/repos/${repo}`),
  readFile: (slug: string, repo: RepoName, path: string) =>
    request<{ path: string; content: string }>("GET", `/projects/${slug}/repos/${repo}/file?path=${encodeURIComponent(path)}`),
  /** `content: null` deletes the file. */
  writeFile: (slug: string, repo: RepoName, body: { path: string; content: string | null; message?: string }) =>
    request<{ commit: string }>("PUT", `/projects/${slug}/repos/${repo}/file`, body),
  restartReflex: (slug: string) => request<ReflexStatus>("POST", `/projects/${slug}/reflex/restart`),
  reflexLog: (slug: string) => request<{ log: string }>("GET", `/projects/${slug}/reflex/log`),
  decide: (slug: string, body: { state: string; questions: Question[]; route?: Route }) => request<DecideResponse>("POST", `/projects/${slug}/decide`, body),
  feedback: (slug: string, p: number, outcome: boolean) => request<{ refit?: boolean }>("POST", `/projects/${slug}/feedback`, { p, outcome }),
  fit: (slug: string, body: { cases: { state: string; question: Question; expected: string | boolean }[]; rho: number; apply: boolean }) =>
    request<FitResult>("POST", `/projects/${slug}/fit`, body),
  decisions: (slug: string, limit = 200) => request<Decision[]>("GET", `/projects/${slug}/decisions?limit=${limit}`),

  // ---- skills (Skills screen). `project: null` is the default workspace.
  skills: (project: string | null) => request<SkillsOverview>("GET", `/skills?project=${encodeURIComponent(project ?? "")}`),
  /** Writes the workspace's `.claude/settings.local.json`; returns the fresh overview. */
  setSkillEnabled: (project: string | null, skill: string, enabled: boolean) =>
    request<SkillsOverview>("PATCH", "/skills/enabled", { project: project ?? "", skill, enabled }),
  /** Only ids the scanner returned are readable. */
  skillFile: (project: string | null, id: string) =>
    request<SkillFile>("GET", `/skills/file?project=${encodeURIComponent(project ?? "")}&path=${encodeURIComponent(id)}`),
};

export type RepoName = "code" | "corpus";
export type Route = "auto" | "reflex" | "llm";
export type ProjectUpdate = {
  name?: string;
  description?: string;
  threshold?: number | null;
  fallback?: string;
  model?: string | null;
  questions?: Question[];
};

/** Clone URL for a project repo; git asks for the API token as the password when one is set. */
export function cloneUrl(slug: string, repo: RepoName): string {
  return `${location.origin}/git/${slug}/${repo}.git`;
}

// ---- orchestrator decisions (backend decisions.rs; not Reflex decisions)

/** `project`: `null` is the default workspace (tasks without a project). */
const projectQuery = (project: string | null) => `project=${encodeURIComponent(project ?? "")}`;

export const decisionsApi = {
  model: () => request<DecisionModel>("GET", "/decisions/model"),
  edges: (since: string, project: string | null) => request<DecisionCounts>("GET", `/decisions/edges?since=${encodeURIComponent(since)}&${projectQuery(project)}`),
  latest: (project: string | null) => request<Event[]>("GET", `/decisions/latest?${projectQuery(project)}`),
  /** The task's `decision.*` events, newest first. */
  trace: (id: number, limit = 300) => request<Event[]>("GET", `/tasks/${id}/trace?limit=${limit}`),
  /** One orchestrator tick while it is paused. */
  step: () => request<StepResult>("POST", "/orchestrator/step"),
};

// ----

export const fmtConf =(c: number | null | undefined) => (c == null ? "—" : c === 0 ? "0" : c < 0.001 ? c.toExponential(2) : c.toFixed(4));
