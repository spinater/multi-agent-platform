import type { AgentDef } from "./generated/AgentDef";
import type { AgentDetail } from "./generated/AgentDetail";
import type { AgentPlan } from "./generated/AgentPlan";
import type { AgentScope } from "./generated/AgentScope";
import type { AgentsOverview } from "./generated/AgentsOverview";
import type { DecideResponse } from "./generated/DecideResponse";
import type { Decision } from "./generated/Decision";
import type { DecisionCounts } from "./generated/DecisionCounts";
import type { DecisionModel } from "./generated/DecisionModel";
import type { Event } from "./generated/Event";
import type { FolderTrust } from "./generated/FolderTrust";
import type { FsListing } from "./generated/FsListing";
import type { FsRoot } from "./generated/FsRoot";
import type { FolderTrusted } from "./generated/FolderTrusted";
import type { FitResult } from "./generated/FitResult";
import type { GitAccess } from "./generated/GitAccess";
import type { GitIdentity } from "./generated/GitIdentity";
import type { GitKey } from "./generated/GitKey";
import type { GitOpResult } from "./generated/GitOpResult";
import type { GitSettings } from "./generated/GitSettings";
import type { GitStatus } from "./generated/GitStatus";
import type { KnownHost } from "./generated/KnownHost";
import type { SettingsInfo } from "./generated/SettingsInfo";
import type { GitTestResult } from "./generated/GitTestResult";
import type { InitPlan } from "./generated/InitPlan";
import type { InitStarted } from "./generated/InitStarted";
import type { LoginState } from "./generated/LoginState";
import type { Overview } from "./generated/Overview";
import type { PipelineInfo } from "./generated/PipelineInfo";
import type { ProjectCreated } from "./generated/ProjectCreated";
import type { ProjectDefaults } from "./generated/ProjectDefaults";
import type { ProjectSettings } from "./generated/ProjectSettings";
import type { ProjectUpdated } from "./generated/ProjectUpdated";
import type { SourceInspection } from "./generated/SourceInspection";
import type { SourceKind } from "./generated/SourceKind";
import type { Question } from "./generated/Question";
import type { ReflexStatus } from "./generated/ReflexStatus";
import type { RepoListing } from "./generated/RepoListing";
import type { Session } from "./generated/Session";
import type { StepResult } from "./generated/StepResult";
import type { SkillFile } from "./generated/SkillFile";
import type { SkillsOverview } from "./generated/SkillsOverview";
import type { Task } from "./generated/Task";
import type { TemplateInfo } from "./generated/TemplateInfo";
import type { TrustAnswer } from "./generated/TrustAnswer";
import type { WorktreeEstimate } from "./generated/WorktreeEstimate";
// Reflex screen.
import type { ArchiveResult } from "./generated/ArchiveResult";
import type { DecisionFeedback } from "./generated/DecisionFeedback";
import type { DecisionSummary } from "./generated/DecisionSummary";
import type { ReflexCase } from "./generated/ReflexCase";
import type { SavingsReport } from "./generated/SavingsReport";

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

export async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
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
  /** Answers the CLI's "trust this folder?" question (409 unless it is on screen and nobody is in the session). */
  trustSession: (name: string) => request<TrustAnswer>("POST", `/sessions/${encodeURIComponent(name)}/trust`, {}),
  createTask: (body: { title: string; prompt: string; depends_on: number[]; provider?: string; model?: string; project?: string }) =>
    request<Task>("POST", "/tasks", body),
  /** `note` (send_back only) goes into the agent's next prompt. 409 with the reason when the task's status refuses it. */
  taskAction: (id: number, action: TaskAction, note?: string) => request<Task>("PATCH", `/tasks/${id}`, note ? { action, note } : { action }),
  deleteTask: (id: number) => request("DELETE", `/tasks/${id}`),
  setOrchestrator: (running: boolean) => request("POST", "/orchestrator", { running }),
  /** The orchestrator's verify / audit / post_done settings (command centre). */
  pipeline: () => request<PipelineInfo>("GET", "/pipeline"),

  createProject: (body: { name: string; slug?: string; description?: string; fallback?: string; model?: string; settings?: ProjectSettings }) =>
    request<ProjectCreated>("POST", "/projects", body),
  /** `checks` says what else the change did, such as seeding the corpus for a Reflex gate just turned on. */
  updateProject: (slug: string, body: ProjectUpdate) => request<ProjectUpdated>("PATCH", `/projects/${slug}`, body),
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
  // ---- agents (Agents screen): custom sub-agents per workspace (`scope: "project"`) or user-wide
  agents: (project: string | null) => request<AgentsOverview>("GET", `/agents?project=${encodeURIComponent(project ?? "")}`),
  agent: (project: string | null, scope: AgentScope, name: string) =>
    request<AgentDetail>("GET", `/agents/${encodeURIComponent(name)}?project=${encodeURIComponent(project ?? "")}&scope=${scope}`),
  /** Writes one file per target CLI and removes the others' files. `dryRun` only returns what it would do. */
  saveAgent: (project: string | null, scope: AgentScope, def: AgentDef, dryRun = false) =>
    request<AgentPlan>("PUT", `/agents${dryRun ? "?dry_run=true" : ""}`, { project: project ?? "", scope, def }),
  deleteAgent: (project: string | null, scope: AgentScope, name: string) =>
    request<{ removed: string[] }>("DELETE", `/agents/${encodeURIComponent(name)}?project=${encodeURIComponent(project ?? "")}&scope=${scope}`),
  // ---- project settings and provider sign-in
  projectDefaults: () => request<ProjectDefaults>("GET", "/projects/defaults"),
  /** Whether each installed CLI trusts the project's folder; `trustProject` marks it (as creating the project does). */
  /** What running `parallel` tasks side by side in worktrees would cost; `copy`/`setup` are the form's unsaved lists. */
  worktreeEstimate: (slug: string, body: { parallel?: number; copy?: string[]; setup?: string[] }) =>
    request<WorktreeEstimate>("POST", `/projects/${slug}/worktree-estimate`, body),
  projectTrust: (slug: string) => request<FolderTrust>("GET", `/projects/${slug}/trust`),
  trustProject: (slug: string) => request<FolderTrusted>("POST", `/projects/${slug}/trust`, {}),
  inspectSource: (body: { kind: SourceKind; location: string; branch?: string }) => request<SourceInspection>("POST", "/projects/inspect", body),
  /** The project folder (or each of `[projects] roots`) the folder picker may browse. */
  fsRoots: () => request<FsRoot[]>("GET", "/fs/roots"),
  /** Sub-folders of `path` (relative to the root); never file contents. */
  fsBrowse: (root: string, path: string, hidden = false) =>
    request<FsListing>("GET", `/fs/browse?root=${encodeURIComponent(root)}&path=${encodeURIComponent(path)}${hidden ? "&hidden=true" : ""}`),
  /** Starts (or returns the running) helper session that runs the provider's login command. */
  loginStart: (provider: string) => request<LoginState>("POST", `/providers/${encodeURIComponent(provider)}/login`),
  loginState: (provider: string) => request<LoginState>("GET", `/providers/${encodeURIComponent(provider)}/login`),
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
  /** Tokens Reflex saved against what the fallbacks spent over the last `hours`; `project: null` is every project. */
  savings: (hours: number, project: string | null) => request<SavingsReport>("GET", `/reflex/savings?hours=${hours}&project=${encodeURIComponent(project ?? "")}`),
  // ---- templates: lay a development process into a project's folder, then queue the init agent
  templates: () => request<TemplateInfo[]>("GET", "/templates"),
  /** What each file would become (create, skip, append); writes nothing. */
  initPlan: (slug: string, template: string) => request<InitPlan>("POST", `/projects/${slug}/init/plan`, { template }),
  /** Writes the template (never overwriting a file) and queues the "Init project" task pinned to `provider`;
   * with `draft_corpus` it also drafts the project's Reflex corpus from the description. */
  initProject: (slug: string, body: { template: string; description: string; provider: string; model?: string; set_verify: boolean; draft_corpus: boolean }) =>
    request<InitStarted>("POST", `/projects/${slug}/init`, body),
};

/** `merge`, `pull_request`, `send_back` act on a task in `review`; `discard` removes its worktree and (unless merged
 * or pushed) its branch. */
export type TaskAction = "retry" | "cancel" | "merge" | "pull_request" | "send_back" | "discard";

/** The platform hosts one repo per project: the Reflex corpus. The code stays in the project's own folder. */
export type RepoName = "corpus";
/** `reflex_gated`: Reflex at or above the threshold only, never the fallback (what the orchestrator's Reflex gates use). */
export type Route = "auto" | "reflex" | "llm" | "reflex_gated";
export type ProjectUpdate = {
  name?: string;
  description?: string;
  threshold?: number | null;
  fallback?: string;
  model?: string | null;
  questions?: Question[];
  /** Providers, verify, audit, paused, the Reflex gates and branches; the source is kept. */
  settings?: ProjectSettings;
  paused?: boolean;
};

/** Clone URL for a project's corpus repo; git asks for the API token as the password when one is set. */
export function cloneUrl(slug: string, repo: RepoName): string {
  return `${location.origin}/git/${slug}/${repo}.git`;
}

// ---- git access (backend git_access.rs) and a project folder's git (project_git.rs). Only public keys come back;
// nothing here ever sends a private key, token or password.

export type GitOp = "fetch" | "pull" | "push";

export const gitApi = {
  access: () => request<GitAccess>("GET", "/git/access"),
  /** Creates MAP's ed25519 key; `replace` moves the old one aside (409 without it when one exists). */
  createKey: (replace = false) => request<GitKey>("POST", "/git/key", { replace }),
  /** `ssh -T git@<host>` with MAP's key: `github.com`, `gitlab.com`, `git@host:2222`. */
  test: (host: string) => request<GitTestResult>("POST", "/git/test", { host }),
  setIdentity: (identity: GitIdentity) => request<GitIdentity>("PUT", "/git/identity", identity),
  /** MAP's own key, or this machine's `~/.ssh` and ssh-agent; the gitconfig is regenerated at once. */
  setSettings: (settings: GitSettings) => request<GitSettings>("PUT", "/git/settings", settings),
  /** Forgets a host recorded on first use (409 for a pinned one); returns the known hosts left. */
  removeKnownHost: (host: string) => request<KnownHost[]>("DELETE", `/git/known-hosts/${encodeURIComponent(host)}`),
  /** The Settings screen's read-only facts: server settings, agent binaries, gh/glab and whether their tokens are set. */
  settings: () => request<SettingsInfo>("GET", "/settings"),
  status: (slug: string) => request<GitStatus>("GET", `/projects/${slug}/git`),
  /** Fetch; pull (fast-forward only); push (never forced; sets the upstream on a first push). */
  run: (slug: string, op: GitOp) => request<GitOpResult>("POST", `/projects/${slug}/git/${op}`, {}),
  /** Sets or replaces `origin`, running `git init -b main` first when the folder is not a repo. */
  setRemote: (slug: string, url: string) => request<GitOpResult>("POST", `/projects/${slug}/git/remote`, { url }),
};

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
