import type { DecideResponse } from "./generated/DecideResponse";
import type { Decision } from "./generated/Decision";
import type { Event } from "./generated/Event";
import type { FitResult } from "./generated/FitResult";
import type { Overview } from "./generated/Overview";
import type { ProjectInfo } from "./generated/ProjectInfo";
import type { Question } from "./generated/Question";
import type { ReflexStatus } from "./generated/ReflexStatus";
import type { RepoListing } from "./generated/RepoListing";
import type { Session } from "./generated/Session";
import type { Task } from "./generated/Task";
// Reflex screen.
import type { ArchiveResult } from "./generated/ArchiveResult";
import type { DecisionFeedback } from "./generated/DecisionFeedback";
import type { DecisionSummary } from "./generated/DecisionSummary";
import type { ReflexCase } from "./generated/ReflexCase";

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

  // ---- Reflex screen: calibration cases, decision feedback and summary, corpus archive
  cases: (slug: string) => request<ReflexCase[]>("GET", `/projects/${slug}/cases`),
  /** Adds cases (scored by Reflex at once). `expected` is an option's text, or yes/no for a yes/no question. */
  addCases: (slug: string, cases: { question_id: string; text: string; expected: string | boolean }[]) => request<ReflexCase[]>("POST", `/projects/${slug}/cases`, { cases }),
  deleteCase: (slug: string, id: number) => request("DELETE", `/projects/${slug}/cases/${id}`),
  /** Re-scores the stored cases and fits the threshold (ρ-quantile); `apply` saves it. Needs 16 cases. */
  fitCases: (slug: string, body: { rho?: number; apply: boolean }) => request<FitResult>("POST", `/projects/${slug}/cases/fit`, body),
  decisionSummary: (slug: string, hours = 24) => request<DecisionSummary>("GET", `/projects/${slug}/decisions/summary?hours=${hours}`),
  /** A wrong answer needs `expected`, the right one; by default the message also becomes a calibration case. */
  decisionFeedback: (slug: string, id: number, body: { correct: boolean; expected?: string | boolean; add_case?: boolean }) =>
    request<DecisionFeedback>("POST", `/projects/${slug}/decisions/${id}/feedback`, body),
  /** Moves domains to `_archive/` in the corpus repo (default: the smallest until 8 are left), or back with `restore`. */
  archiveDomains: (slug: string, body: { domains?: string[]; restore?: boolean }) => request<ArchiveResult>("POST", `/projects/${slug}/corpus/archive`, body),
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

export const fmtConf = (c: number | null | undefined) => (c == null ? "—" : c === 0 ? "0" : c < 0.001 ? c.toExponential(2) : c.toFixed(4));
