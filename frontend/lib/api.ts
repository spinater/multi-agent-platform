import type { ActivityGraph } from "./generated/ActivityGraph";
import type { AgentDef } from "./generated/AgentDef";
import type { AgentDetail } from "./generated/AgentDetail";
import type { AgentPlan } from "./generated/AgentPlan";
import type { AgentScope } from "./generated/AgentScope";
import type { AgentsOverview } from "./generated/AgentsOverview";
import type { DecideResponse } from "./generated/DecideResponse";
import type { Decision } from "./generated/Decision";
import type { DecisionCounts } from "./generated/DecisionCounts";
import type { DecisionModel } from "./generated/DecisionModel";
import type { EnvList } from "./generated/EnvList";
import type { KnowledgeGraph } from "./generated/KnowledgeGraph";
import type { ConfigAskRequest } from "./generated/ConfigAskRequest";
import type { ConfigAskResult } from "./generated/ConfigAskResult";
import type { Workflow } from "./generated/Workflow";
import type { WorkflowList } from "./generated/WorkflowList";
import type { WorkflowStats } from "./generated/WorkflowStats";
import type { ConfigOp as GenConfigOp } from "./generated/ConfigOp";
import type { Hook } from "./generated/Hook";
import type { HookRun } from "./generated/HookRun";
import type { HooksOverview } from "./generated/HooksOverview";
import type { McpServerInput } from "./generated/McpServerInput";
import type { McpTestResult } from "./generated/McpTestResult";
import type { McpView } from "./generated/McpView";
import type { PermChange } from "./generated/PermChange";
import type { PermissionsView } from "./generated/PermissionsView";
import type { RulesOverview } from "./generated/RulesOverview";
import type { EnvVarInput } from "./generated/EnvVarInput";
import type { Event } from "./generated/Event";
import type { SessionTrace } from "./generated/SessionTrace";
import type { ProviderInfo } from "./generated/ProviderInfo";
import type { FolderTrust } from "./generated/FolderTrust";
import type { FsListing } from "./generated/FsListing";
import type { FsRoot } from "./generated/FsRoot";
import type { FolderTrusted } from "./generated/FolderTrusted";
import type { FitResult } from "./generated/FitResult";
import type { GatesInfo } from "./generated/GatesInfo";
import type { GitAccess } from "./generated/GitAccess";
import type { GitIdentity } from "./generated/GitIdentity";
import type { GitKey } from "./generated/GitKey";
import type { GitOpResult } from "./generated/GitOpResult";
import type { GitSettings } from "./generated/GitSettings";
import type { GitStatus } from "./generated/GitStatus";
import type { KnowledgeDocBody } from "./generated/KnowledgeDocBody";
import type { KnowledgeKind } from "./generated/KnowledgeKind";
import type { KnowledgeOverview } from "./generated/KnowledgeOverview";
import type { KnowledgeSearch } from "./generated/KnowledgeSearch";
import type { KnownHost } from "./generated/KnownHost";
import type { SettingsInfo } from "./generated/SettingsInfo";
import type { GitTestResult } from "./generated/GitTestResult";
import type { InitPlan } from "./generated/InitPlan";
import type { InitStarted } from "./generated/InitStarted";
import type { LoginState } from "./generated/LoginState";
import type { OrchestratorStatus } from "./generated/OrchestratorStatus";
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
import type { TaskPage } from "./generated/TaskPage";
import type { TaskStatus } from "./generated/TaskStatus";
import type { TemplateInfo } from "./generated/TemplateInfo";
import type { ThaiPreview } from "./generated/ThaiPreview";
import type { ThaiReport } from "./generated/ThaiReport";
import type { TrustAnswer } from "./generated/TrustAnswer";
import type { WorktreeEstimate } from "./generated/WorktreeEstimate";
// Reflex screen.
import type { ArchiveResult } from "./generated/ArchiveResult";
import type { DecisionFeedback } from "./generated/DecisionFeedback";
import type { DecisionSummary } from "./generated/DecisionSummary";
import type { ReflexCase } from "./generated/ReflexCase";
import type { SavingsReport } from "./generated/SavingsReport";
// Lanes screen and Settings → Telegram.
import type { LaneImport } from "./generated/LaneImport";
import type { LaneInfo } from "./generated/LaneInfo";
import type { ManagerInfo } from "./generated/ManagerInfo";
import type { ManagerSettings } from "./generated/ManagerSettings";
import type { OpenLane } from "./generated/OpenLane";
import type { OwnedLaneRecord } from "./generated/OwnedLaneRecord";
import type { LaneScreen } from "./generated/LaneScreen";
import type { LaneTell } from "./generated/LaneTell";
import type { TelegramInfo } from "./generated/TelegramInfo";
import type { TellVerb } from "./generated/TellVerb";

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
  /** `note` (send_back, clarify) goes into the agent's next prompt. 409 with the reason when the task's status refuses it. */
  taskAction: (id: number, action: TaskAction, note?: string) => request<Task>("PATCH", `/tasks/${id}`, note ? { action, note } : { action }),
  deleteTask: (id: number) => request("DELETE", `/tasks/${id}`),
  /** One page of tasks, newest change first. `project`: `undefined` every project, `null` the default workspace. */
  taskPage: (f: { status?: TaskStatus; project?: string | null; q?: string; limit?: number; offset?: number }) => {
    const p = new URLSearchParams();
    if (f.status) p.set("status", f.status);
    if (f.project !== undefined) p.set("project", f.project ?? "");
    if (f.q?.trim()) p.set("q", f.q.trim());
    if (f.limit != null) p.set("limit", String(f.limit));
    if (f.offset != null) p.set("offset", String(f.offset));
    return request<TaskPage>("GET", `/tasks/page?${p}`);
  },
  /** Pause or run the orchestrator. */
  setOrchestrator: (running: boolean) => request<OrchestratorStatus>("POST", "/orchestrator", { running }),
  /** Live changes, no restart: `max_parallel` is tasks at once across all projects (0 resets to map.toml's value). */
  updateOrchestrator: (body: { running?: boolean; max_parallel?: number }) => request<OrchestratorStatus>("POST", "/orchestrator", body),
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
  // ---- Thai for Reflex: the built-in glossary plus the project's own words (Thai → English, `-` skips a word)
  thai: (slug: string) => request<ThaiReport>("GET", `/projects/${slug}/thai`),
  /** What Reflex reads for `text`; `words` tries the editor's unsaved table instead of the saved one. */
  thaiPreview: (slug: string, text: string, words?: Record<string, string>) => request<ThaiPreview>("POST", `/projects/${slug}/thai/preview`, words ? { text, words } : { text }),
  /** Replaces the project's words; 400 with the reason for a bad one. */
  setThaiWords: (slug: string, words: Record<string, string>) => request<ThaiReport>("PUT", `/projects/${slug}/thai/words`, { words }),
  // ---- the orchestrator gates' own Reflex, shared by every project
  gates: () => request<GatesInfo>("GET", "/gates"),
  /** `null` goes back to map.toml's `[reflex] gates_threshold` (unset there: shadow). */
  setGatesThreshold: (threshold: number | null) => request<GatesInfo>("POST", "/gates", { threshold }),
  restartGates: () => request<GatesInfo>("POST", "/gates/restart", {}),
  gatesFile: (path: string) => request<{ path: string; content: string }>("GET", `/gates/file?path=${encodeURIComponent(path)}`),
  /** Saves one domain's file and restarts the gates Reflex. */
  saveGatesFile: (path: string, content: string) => request<GatesInfo>("PUT", "/gates/file", { path, content }),

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
  /** Runs the project's parallel analyst now; `started: false` says why not (`reason`). */
  analyzeParallel: (slug: string) => request<{ ok: boolean; started: boolean; reason: string }>("POST", `/projects/${slug}/parallel/analyze`, {}),
  projectTrust: (slug: string) => request<FolderTrust>("GET", `/projects/${slug}/trust`),
  trustProject: (slug: string) => request<FolderTrusted>("POST", `/projects/${slug}/trust`, {}),
  inspectSource: (body: { kind: SourceKind; location: string; branch?: string }) => request<SourceInspection>("POST", "/projects/inspect", body),
  /** The project folder (or each of `[projects] roots`) the folder picker may browse. */
  fsRoots: () => request<FsRoot[]>("GET", "/fs/roots"),
  /** Sub-folders of `path` (relative to the root); never file contents. */
  fsBrowse: (root: string, path: string, hidden = false) =>
    request<FsListing>("GET", `/fs/browse?root=${encodeURIComponent(root)}&path=${encodeURIComponent(path)}${hidden ? "&hidden=true" : ""}`),
  /** Starts (or returns the running) helper session that runs the provider's login command. */
  providers: () => request<ProviderInfo[]>("GET", "/providers"),
  loginStart: (provider: string) => request<LoginState>("POST", `/providers/${encodeURIComponent(provider)}/login`),
  loginState: (provider: string) => request<LoginState>("GET", `/providers/${encodeURIComponent(provider)}/login`),
  /** Types the pasted code into the sign-in session, only while it asks for one (409 otherwise). */
  loginCode: (provider: string, code: string) => request<LoginState>("POST", `/providers/${encodeURIComponent(provider)}/login/code`, { code }),
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
  // ---- knowledge (Knowledge screen): the project's docs, their health, search; fix tasks go through createTask
  /** Reindexes first when the index is stale. */
  knowledge: (slug: string) => request<KnowledgeOverview>("GET", `/projects/${slug}/knowledge`),
  reindexKnowledge: (slug: string) => request<KnowledgeOverview>("POST", `/projects/${slug}/knowledge/reindex`, {}),
  /** Accepts the knowledge suggestion: queues the setup task (once) and returns it. */
  acceptKnowledge: (slug: string) => request<Task>("POST", `/projects/${slug}/knowledge/accept`, {}),
  /** `dismissed: false` shows the suggestion again. */
  dismissKnowledge: (slug: string, dismissed: boolean) => request<KnowledgeOverview>("POST", `/projects/${slug}/knowledge/dismiss`, { dismissed }),
  /** Substring search (Thai or English); recorded as an owner query. `kind: null` searches every kind. */
  searchKnowledge: (slug: string, q: string, kind: KnowledgeKind | null, limit = 30) =>
    request<KnowledgeSearch>("GET", `/projects/${slug}/knowledge/search?q=${encodeURIComponent(q)}&kind=${kind ?? ""}&limit=${limit}`),
  /** Only indexed paths (including `map://tasks/<id>`, `map://plans/<id>`) are readable. */
  knowledgeGraph: (slug: string) => request<KnowledgeGraph>("GET", `/projects/${slug}/knowledge/graph`),
  knowledgeDoc: (slug: string, path: string) => request<KnowledgeDocBody>("GET", `/projects/${slug}/knowledge/doc?path=${encodeURIComponent(path)}`),
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
 * or pushed) its branch. `start_anyway` releases a task the intake gate holds; `clarify` (with a note) adds to a
 * pending task's prompt and the intake gate looks again. */
export type TaskAction = "retry" | "cancel" | "merge" | "pull_request" | "send_back" | "discard" | "start_anyway" | "clarify";

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
  /** Providers, verify, audit, paused, the Reflex gates, branches, knowledge and parallel; the source is kept. */
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

// ---- lanes (backend lanes.rs): tmux sessions someone else owns that MAP watches, and Telegram. Only booleans come
// back for the bot token, never its value.

export type LanePatch = { watch?: boolean; actions?: boolean; clear_enabled?: boolean; status_prompt?: string };

const lanePath = (name: string) => `/lanes/${encodeURIComponent(name)}`;
const sessionQuery = (session?: string | null) => (session ? `session=${encodeURIComponent(session)}` : "");

export const lanesApi = {
  list: () => request<LaneInfo[]>("GET", "/lanes"),
  /** The dashboard's changes over map.toml: watch, actions, `/clear` on or off, the status prompt. */
  update: (name: string, body: LanePatch) => request<LaneInfo>("PATCH", lanePath(name), body),
  /** The tail of the last capture (ANSI stripped); without `session`, the lane's first session. */
  screen: (name: string, session?: string | null, lines = 60) =>
    request<LaneScreen>("GET", `${lanePath(name)}/screen?lines=${lines}${session ? `&${sessionQuery(session)}` : ""}`),
  /** Queues an instruction, done on the next tick the session is idle and nobody is attached; expires after 3 h. */
  tell: (name: string, body: { verb: TellVerb; note?: string; mins?: number; session?: string }) => request<LaneTell>("POST", `${lanePath(name)}/tell`, body),
  tells: (name: string) => request<LaneTell[]>("GET", `${lanePath(name)}/tells`),
  /** Cancels a queued tell; `session` empty: the one for any session. */
  cancelTell: (name: string, verb: TellVerb, session?: string | null) => {
    const q = sessionQuery(session);
    return request<{ ok: boolean }>("DELETE", `${lanePath(name)}/tells/${verb}${q ? `?${q}` : ""}`);
  },
  /** Fills lane settings (and the Telegram chat) from an agent-watcher `config.json` on this machine. */
  importWatcher: (path: string) => request<LaneImport>("POST", "/lanes/import-watcher", { path }),
  telegram: () => request<TelegramInfo>("GET", "/telegram"),
  /** Sends a test message to the configured chat. */
  telegramTest: () => request<{ ok: boolean; message: string }>("POST", "/telegram/test", {}),
};

// ---- a project's manager and its persistent lanes (backend manager.rs)

const projPath = (slug: string) => `/projects/${encodeURIComponent(slug)}`;

/** The activity graph (backend trace.rs): a project's task flow and what each agent used. */
export const activityApi = {
  /** `since`: tasks changed since then plus every open one (default: the last 24 h); `all`: every task. */
  project: (slug: string, opts: { since?: string; all?: boolean; task?: number } = {}) => {
    const q = new URLSearchParams();
    if (opts.since) q.set("since", opts.since);
    if (opts.all) q.set("all", "true");
    if (opts.task != null) q.set("task", String(opts.task));
    const qs = q.toString();
    return request<ActivityGraph>("GET", `${projPath(slug)}/activity${qs ? `?${qs}` : ""}`);
  },
  session: (name: string) => request<SessionTrace>("GET", `/sessions/${encodeURIComponent(name)}/trace`),
};

export const managerApi = {
  get: (slug: string) => request<ManagerInfo>("GET", `${projPath(slug)}/manager`),
  /** On/off, provider, model. Turning it off ends the session (409 while someone uses it, unless `force`). */
  set: (slug: string, body: ManagerSettings & { force?: boolean }) => request<ManagerInfo>("PUT", `${projPath(slug)}/manager`, body),
  /** Start now, also after MAP stopped restarting it. */
  start: (slug: string) => request<Session>("POST", `${projPath(slug)}/manager/start`, {}),
  lanes: (slug: string) => request<LaneInfo[]>("GET", `${projPath(slug)}/lanes`),
  openLane: (slug: string, body: OpenLane) => request<OwnedLaneRecord>("POST", `${projPath(slug)}/lanes`, body),
  startLane: (slug: string, name: string) => request<Session>("POST", `${projPath(slug)}/lanes/${encodeURIComponent(name)}/start`, {}),
  closeLane: (slug: string, name: string, force = false) => request<{ ok: boolean }>("DELETE", `${projPath(slug)}/lanes/${encodeURIComponent(name)}${force ? "?force=true" : ""}`),
};

// ----

export const fmtConf =(c: number | null | undefined) => (c == null ? "—" : c === 0 ? "0" : c < 0.001 ? c.toExponential(2) : c.toFixed(4));

export type ConfigOp = GenConfigOp;
export type AskReply = ConfigAskResult;
export type AskBody = ConfigAskRequest;

const scopeQ = (project: string | null) => (project ? `?project=${encodeURIComponent(project)}` : "");

/** Configure › Env & secrets: secret values are write-only (`value: null` keeps a stored secret). */
export const envApi = {
  list: (project: string | null) => request<EnvList>("GET", `/env${scopeQ(project)}`),
  save: (project: string | null, vars: EnvVarInput[]) => request<EnvList>("PUT", "/env", { project: project ?? undefined, vars }),
};

export const workflowsApi = {
  list: (project: string | null) => request<WorkflowList>("GET", `/workflows${scopeQ(project)}`),
  save: (project: string | null, workflow: Workflow) => request<Workflow>("PUT", "/workflows", { project: project ?? undefined, workflow }),
  remove: (project: string | null, id: string) => request<WorkflowList>("DELETE", "/workflows", { project: project ?? undefined, id }),
  stats: (id: string, project: string | null) => request<WorkflowStats>("GET", `/workflows/${encodeURIComponent(id)}/stats${scopeQ(project)}`),
};

/** Configure › Rules: the context files agents read at session start. */
export const rulesApi = {
  list: (project: string | null) => request<RulesOverview>("GET", `/rules${scopeQ(project)}`),
  write: (project: string | null, path: string, body: string) => request<RulesOverview>("PUT", "/rules", { project: project ?? undefined, path, body }),
  remove: (project: string | null, path: string) => request<RulesOverview>("DELETE", "/rules", { project: project ?? undefined, path }),
  readers: (project: string | null, path: string, readers: string[]) => request<RulesOverview>("PUT", "/rules/readers", { project: project ?? undefined, path, readers }),
};

/** Configure › Hooks: commands MAP runs around a task. */
export const hooksApi = {
  list: (project: string | null) => request<HooksOverview>("GET", `/hooks${scopeQ(project)}`),
  save: (project: string | null, hooks: Hook[]) => request<HooksOverview>("PUT", "/hooks", { project: project ?? undefined, hooks }),
  test: (project: string | null, command: string, timeout_secs: number) => request<HookRun>("POST", "/hooks/test", { project: project ?? undefined, command, timeout_secs }),
};

/** Configure › Permissions: tool rules × CLI, written to each CLI's settings. */
export const permsApi = {
  get: (project: string | null) => request<PermissionsView>("GET", `/permissions${scopeQ(project)}`),
  set: (project: string | null, cells: PermChange[]) => request<PermissionsView>("PUT", "/permissions", { project: project ?? undefined, cells }),
};

/** Configure › MCP servers, across the CLIs' own files. */
export const mcpApi = {
  list: (project: string | null) => request<McpView>("GET", `/mcp${scopeQ(project)}`),
  save: (project: string | null, server: McpServerInput, providers: string[]) => request<McpView>("PUT", "/mcp", { project, server, providers }),
  remove: (project: string | null, name: string) => request<McpView>("DELETE", "/mcp", { project, name }),
  test: (project: string | null, name: string) => request<McpTestResult>("POST", "/mcp/test", { project, name }),
};

export const configureApi = {
  ask: (body: AskBody) => request<AskReply>("POST", "/configure/ask", body),
};

export type { Workflow, WorkflowStats };
export type { WorkflowNode } from "./generated/WorkflowNode";
export type { WorkflowEdge } from "./generated/WorkflowEdge";
