/** Pure helpers for the Decisions screen: where each task sits in the orchestrator's decision graph, colours, and
 * what the "now deciding" panel shows. The graph itself (nodes, edges, event kinds) comes from
 * `GET /api/decisions/model`, so these only map live data onto it. */

import type { DecisionKind } from "@/lib/generated/DecisionKind";
import type { DecisionModel } from "@/lib/generated/DecisionModel";
import type { DecisionNode } from "@/lib/generated/DecisionNode";
import type { Event } from "@/lib/generated/Event";
import type { Session } from "@/lib/generated/Session";
import type { Task } from "@/lib/generated/Task";
import { providerColor } from "@/lib/colors";

export const DECISION_PREFIX = "decision.";
export const isDecision = (e: Event) => e.kind.startsWith(DECISION_PREFIX) && e.task_id !== null;

/** Grid position of each node: column, row (0 happy path, 1 recovery). */
export const LAYOUT: Record<DecisionNode, { c: number; r: number }> = {
  ready: { c: 0, r: 0 },
  pick: { c: 1, r: 0 },
  launch: { c: 2, r: 0 },
  watch: { c: 3, r: 0 },
  verify: { c: 4, r: 0 },
  audit: { c: 5, r: 0 },
  post: { c: 6, r: 0 },
  end: { c: 7, r: 0 },
  done: { c: 8, r: 0 },
  blocked: { c: 0, r: 1 },
  retry: { c: 3.5, r: 1 },
  failed: { c: 6.5, r: 1 },
};

/** List order: the furthest along first. */
export const ORDER: DecisionNode[] = ["ready", "pick", "launch", "watch", "verify", "audit", "post", "end", "retry", "blocked", "failed", "done"];
/** The chips row above the canvas. */
export const CHIPS: DecisionNode[] = ["watch", "verify", "audit", "pick", "ready", "retry", "blocked", "failed", "done"];

export const edgeKey = (from: DecisionNode, to: DecisionNode) => `${from}>${to}`;
export const isFailEdge = (to: DecisionNode) => to === "retry" || to === "failed" || to === "blocked";

export function nodeTone(n: DecisionNode): string {
  if (n === "done") return "var(--accent)";
  if (n === "failed") return "var(--bad)";
  if (n === "blocked" || n === "retry") return "var(--warn)";
  if (n === "watch") return "var(--ok)";
  if (n === "verify" || n === "audit") return "var(--info)";
  if (n === "pick" || n === "ready") return "var(--idle)";
  return "var(--text-2)";
}

export const tint = (color: string, pct: number) => `color-mix(in srgb, ${color} ${pct}%, transparent)`;

export const levelColor: Record<Event["level"], string> = { info: "var(--text-2)", success: "var(--ok)", warning: "var(--warn)", error: "var(--bad)" };

/** Colour of a decision in a trace: what the step led to. */
export function decisionColor(e: Event, kind: DecisionKind | undefined): string {
  if (!kind) return levelColor[e.level];
  if (kind.to === "failed" || e.level === "error") return "var(--bad)";
  if (isFailEdge(kind.to) || e.level === "warning") return "var(--warn)";
  if (kind.from === kind.to) return "var(--info)";
  return "var(--ok)";
}

export type KindMap = Map<string, DecisionKind>;
export const kindMap = (model: DecisionModel | null): KindMap => new Map((model?.kinds ?? []).map((k) => [k.kind, k]));

/** Where the task is now: its status, refined by its newest decision (post_done and the end decision run while the
 * status still reads verifying/auditing). `null` hides cancelled tasks. */
export function nodeOf(task: Task, last: Event | undefined, kinds: KindMap): DecisionNode | null {
  const to = last ? kinds.get(last.kind)?.to : undefined;
  switch (task.status) {
    case "cancelled":
      return null;
    case "done":
      return "done";
    case "failed":
      return "failed";
    case "pending":
      if (task.result?.startsWith("Blocked")) return "blocked";
      if (last?.kind === "decision.pick.none") return "pick";
      return "ready";
    case "running":
      return "watch";
    case "verifying":
    case "auditing":
      if (to && ["verify", "audit", "post", "end"].includes(to)) return to;
      return task.status === "verifying" ? "verify" : "audit";
  }
}

/** The provider working on the task: its session's, else the pinned one (`assigned` false: drawn hollow). */
export function providerOf(task: Task, sessions: Session[]): { name: string | null; color: string; assigned: boolean } {
  const session = task.session ? sessions.find((s) => s.name === task.session) : undefined;
  const active = task.status !== "pending" && !!session;
  const name = active ? session!.provider : task.provider;
  return { name: name ?? null, color: name ? providerColor(name) : "var(--muted)", assigned: active };
}

/** A session state that needs a person (dashed ring on the token). */
export function attention(task: Task, node: DecisionNode | null, sessions: Session[], last: Event | undefined): string | null {
  if (node === "pick" && last?.kind === "decision.pick.none") return "no provider available";
  if (node !== "watch" && node !== "end") return null;
  const s = sessions.find((x) => x.name === task.session);
  if (!s) return null;
  if (s.state === "waiting_input") return "agent is asking a question";
  if (s.state === "rate_limited") return "rate limited";
  if (s.state === "needs_auth") return "needs sign-in";
  if (s.state === "needs_trust") return "asks to trust its folder";
  if (s.state === "stalled") return "stalled";
  if (s.state === "error") return "session error";
  if (last?.kind === "decision.retry.kept") return "a person is in the session";
  return null;
}

export const hms = (iso: string) => new Date(iso).toTimeString().slice(0, 8);

/** Local midnight, for "×n today". */
export function startOfToday(now: number): string {
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  return d.toISOString();
}

/** The backticked command in a decision message: "`bun test` exit 0 · 1 of 4" → "bun test". */
export const commandIn = (message: string) => /`([^`]*)`/.exec(message)?.[1] ?? null;

export type Check = { mark: string; color: string; text: string };

const OK = "var(--ok)";
const WARN = "var(--warn)";
const BAD = "var(--bad)";
const IDLE = "var(--idle)";
const INFO = "var(--info)";

/** Decisions of the current attempt (since the newest `launch.started`), oldest first. */
function currentAttempt(trace: Event[]): Event[] {
  const i = trace.findIndex((e) => e.kind === "decision.launch.started");
  return (i < 0 ? trace : trace.slice(0, i + 1)).slice().reverse();
}

/** Progress through a list of commands from `<prefix>.cmd_start` / `.cmd_pass` events. */
function commandChecks(commands: string[], events: Event[], prefix: "verify" | "post", substitute = false): Check[] {
  const passed = new Set<string>();
  let running: string | null = null;
  let failed: string | null = null;
  for (const e of events) {
    const cmd = commandIn(e.message);
    if (!cmd) continue;
    if (e.kind === `decision.${prefix}.cmd_start`) running = cmd;
    if (e.kind === `decision.${prefix}.cmd_pass`) {
      passed.add(cmd);
      if (running === cmd) running = null;
    }
    if (e.kind === `decision.${prefix}.fail`) failed = cmd;
  }
  // post_done templates get `{id}`/`{title}` filled in; match them by position instead.
  const passedList = [...passed];
  return commands.map((c, i) => {
    const done = substitute ? i < passedList.length : passed.has(c);
    const isFailed = failed !== null && (substitute ? i === passedList.length : failed === c);
    const isRunning = !done && !isFailed && (substitute ? i === passedList.length && running !== null : running === c);
    if (done) return { mark: "✓", color: OK, text: `${c} · exit 0` };
    if (isFailed) return { mark: "✕", color: BAD, text: `${c} · failed` };
    if (isRunning) return { mark: "▸", color: INFO, text: `${c} · running` };
    return { mark: "·", color: IDLE, text: c };
  });
}

function minutesSince(iso: string, now: number) {
  return Math.max(0, Math.round((now - new Date(iso).getTime()) / 60000));
}

/** The checks behind the task's current decision, from live state and its trace (newest first). */
export function nowChecks(args: {
  task: Task;
  node: DecisionNode;
  trace: Event[];
  tasks: Task[];
  sessions: Session[];
  model: DecisionModel;
  active: number;
  now: number;
  nodeOfTask: (t: Task) => DecisionNode | null;
}): Check[] {
  const { task, node, trace, tasks, sessions, model, active, now } = args;
  const attempt = currentAttempt(trace);
  const label = (n: DecisionNode | null) => model.nodes.find((x) => x.id === n)?.label ?? n ?? "hidden";
  switch (node) {
    case "ready": {
      const deps = task.depends_on.map((d): Check => {
        const t = tasks.find((x) => x.id === d);
        const done = t?.status === "done";
        return { mark: done ? "✓" : "…", color: done ? OK : IDLE, text: `#${d} ${t?.title ?? ""} · ${t ? (done ? "done" : `at ${label(args.nodeOfTask(t))}`) : "missing"}` };
      });
      if (!deps.length) deps.push({ mark: "✓", color: OK, text: "no dependencies" });
      return [...deps, { mark: "·", color: active >= model.max_parallel ? WARN : IDLE, text: `slots ${active} of ${model.max_parallel} in use` }];
    }
    case "pick": {
      const last = trace.find((e) => e.kind === "decision.pick.none");
      return [
        { mark: "·", color: IDLE, text: task.provider ? `pinned to ${task.provider}` : `provider_order ${model.provider_order.join(" → ")}` },
        ...(last ? [{ mark: "✕", color: BAD, text: last.message }] : []),
        { mark: "…", color: WARN, text: `result: "${task.result ?? "waiting"}"` },
      ];
    }
    case "watch": {
      const s = sessions.find((x) => x.name === task.session);
      const rows: Check[] = [
        { mark: "·", color: IDLE, text: `session ${task.session ?? "?"} · ${(s?.state ?? "unknown").replace("_", " ")}` },
        { mark: "…", color: IDLE, text: "no $MAP_RESULT_FILE yet" },
      ];
      if (s) rows.push({ mark: "·", color: IDLE, text: `${minutesSince(s.created_at, now)}m of task_timeout ${model.task_timeout_minutes}m` });
      if (s?.state === "rate_limited") rows.push({ mark: "!", color: WARN, text: `rate limited${s.detail ? ` · ${s.detail}` : ""} · the watcher wakes it at reset; the orchestrator keeps waiting` });
      if (s?.state === "waiting_input") rows.push({ mark: "!", color: "var(--accent)", text: "agent is asking a question · a person must answer in the session" });
      if (s?.state === "stalled") rows.push({ mark: "!", color: WARN, text: `${s.detail ?? "no pane change"} · still under the timeout, so no decision yet` });
      if (s?.state === "needs_auth") rows.push({ mark: "!", color: BAD, text: "needs a new sign-in · the next check moves the task to another provider" });
      if (s?.state === "needs_trust") rows.push({ mark: "!", color: WARN, text: `${s.detail ?? "asks to trust its folder"} · waits for you (no timeout): Trust this folder on the session card` });
      if (trace[0]?.kind === "decision.retry.kept") rows.push({ mark: "!", color: WARN, text: trace[0].message });
      return rows;
    }
    case "verify":
      if (!model.verify.length) return [{ mark: "·", color: IDLE, text: "no verify commands configured" }];
      return commandChecks(model.verify, attempt, "verify");
    case "audit": {
      const since = attempt.slice(attempt.findIndex((e) => e.kind === "decision.verify.pass") + 1);
      const status = new Map<string, Check>();
      for (const e of since) {
        const who = e.message.split(" · ")[0].split(":")[0];
        if (e.kind === "decision.audit.unavailable") status.set(who, { mark: "✕", color: WARN, text: e.message });
        if (e.kind === "decision.audit.ask") status.set(who, { mark: "▸", color: INFO, text: e.message });
      }
      // The project's Reflex audit gate, asked before any auditor (yes, unsure or not running: the audit runs).
      const gate = since.find((e) => e.kind.startsWith("decision.audit.reflex_"));
      return [
        ...(gate ? [{ mark: "·", color: INFO, text: `Reflex gate · ${gate.message}` }] : []),
        ...model.audit_chain.map((p) => status.get(p) ?? { mark: "·", color: IDLE, text: `${p} · next in chain` }),
        { mark: "·", color: IDLE, text: `audit.required = ${model.audit_required}` },
      ];
    }
    case "post":
      if (!model.post_done.length) return [{ mark: "·", color: IDLE, text: "no post_done hooks configured" }];
      return commandChecks(model.post_done, attempt, "post", true);
    case "end": {
      const s = sessions.find((x) => x.name === task.session);
      return [
        { mark: "·", color: IDLE, text: `tmux client attached: ${s?.attached ? "yes" : "no"}` },
        { mark: "·", color: IDLE, text: `session ${task.session ?? "?"} · ${(s?.state ?? "unknown").replace("_", " ")}` },
      ];
    }
    case "retry":
      return [{ mark: "·", color: IDLE, text: `attempts ${task.attempts} of ${model.max_attempts}` }];
    case "blocked": {
      const dead = task.depends_on.find((d) => ["failed", "cancelled"].includes(tasks.find((t) => t.id === d)?.status ?? ""));
      return [
        { mark: "✕", color: BAD, text: task.result ?? "a dependency failed or was cancelled" },
        { mark: "·", color: IDLE, text: dead ? `retry #${dead} to unblock` : "retry the dependency to unblock" },
      ];
    }
    case "failed":
      return [
        { mark: "✕", color: BAD, text: `attempts ${task.attempts} of ${model.max_attempts} used` },
        ...(task.result ? [{ mark: "·", color: IDLE, text: task.result.split("\n")[0] }] : []),
      ];
    case "done": {
      const end = trace.find((e) => e.kind.startsWith("decision.end.") || e.kind === "decision.post.pass");
      return [
        { mark: "✓", color: OK, text: end?.message ?? "done" },
        ...(task.result ? [{ mark: "·", color: IDLE, text: task.result.split("\n")[0] }] : []),
      ];
    }
    case "launch":
      return [{ mark: "▸", color: INFO, text: "starting the worker session" }];
  }
}
