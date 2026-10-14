"use client";

import { useSyncExternalStore } from "react";

import type { Session } from "./generated/Session";
import type { SessionState } from "./generated/SessionState";
import type { Task } from "./generated/Task";
import type { TaskStatus } from "./generated/TaskStatus";

/** Live activity: what changed since the page loaded, from the WebSocket's `task` and `session` messages (see
 * `use-dashboard.ts`). A full load (`/api/overview`, also after a reconnect) only sets the baseline, so nothing
 * animates for state that was already there. The feed, the rows and the canvas read the marks; the canvas reads
 * them every frame through `marks()` without re-rendering. Times are `Date.now()` milliseconds. */

export type ActivityKind = "spawn" | "phase" | "done" | "failed" | "cancelled" | "session";

export type Activity = {
  id: number;
  at: number;
  kind: ActivityKind;
  taskId: number | null;
  session: string | null;
  /** The task's or session's project; `null` is the default workspace. */
  project: string | null;
  provider: string | null;
  model: string | null;
  title: string;
  from: TaskStatus | null;
  to: TaskStatus | null;
  attempt: number;
};

/** The latest change of a task (status) or session (state, spawn, output line). */
export type TaskMark = { at: number; from: TaskStatus | null; to: TaskStatus };
export type SessionMark = { at: number; from: SessionState | null; to: SessionState };
export type Marks = {
  task: Map<number, TaskMark>;
  /** Sessions started after the page loaded, by name: when they appeared. */
  spawn: Map<string, number>;
  session: Map<string, SessionMark>;
  /** When each session's `last_line` last changed: the heartbeat. */
  line: Map<string, number>;
};

const MAX_ITEMS = 40;

let tasks = new Map<number, Task>();
let sessions = new Map<string, Session>();
const marks: Marks = { task: new Map(), spawn: new Map(), session: new Map(), line: new Map() };
let items: Activity[] = [];
let seq = 0;
/** When tracking started: events older than this seed the feed. */
export const trackingSince = Date.now();
let snapshot = { items, version: 0 };
const listeners = new Set<() => void>();
const spawnListeners = new Set<(a: Activity) => void>();

function changed() {
  snapshot = { items, version: snapshot.version + 1 };
  listeners.forEach((l) => l());
}

function push(a: Omit<Activity, "id">) {
  const item = { ...a, id: ++seq };
  items = [item, ...items].slice(0, MAX_ITEMS);
  if (item.kind === "spawn") spawnListeners.forEach((l) => l(item));
}

/** A full load: remember what is there without marking anything. */
export function baseline(ts: Task[], ss: Session[]) {
  tasks = new Map(ts.map((t) => [t.id, t]));
  sessions = new Map(ss.map((s) => [s.name, s]));
}

export function trackSession(s: Session) {
  const before = sessions.get(s.name);
  sessions.set(s.name, s);
  const now = Date.now();
  if (!before) {
    marks.spawn.set(s.name, now);
    marks.session.set(s.name, { at: now, from: null, to: s.state });
    const t = s.task_id != null ? tasks.get(s.task_id) : undefined;
    push({
      at: now,
      kind: s.task_id != null ? "spawn" : "session",
      taskId: s.task_id,
      session: s.name,
      project: s.project,
      provider: s.provider,
      model: s.model,
      title: t?.title ?? (s.task_id != null ? `task #${s.task_id}` : `${s.role} session`),
      from: null,
      to: null,
      // Workers are named `<prefix>task-<id>-<attempt>`.
      attempt: Number(/task-\d+-(\d+)$/.exec(s.name)?.[1] ?? t?.attempts ?? 1) || 1,
    });
  } else if (before.state !== s.state) marks.session.set(s.name, { at: now, from: before.state, to: s.state });
  if (s.last_line && s.last_line !== before?.last_line) marks.line.set(s.name, now);
  changed();
}

export function forgetSession(name: string) {
  sessions.delete(name);
  marks.spawn.delete(name);
  marks.session.delete(name);
  marks.line.delete(name);
  changed();
}

const KIND: Partial<Record<TaskStatus, ActivityKind>> = { done: "done", failed: "failed", cancelled: "cancelled" };

export function trackTask(t: Task) {
  const before = tasks.get(t.id);
  tasks.set(t.id, t);
  if (!before || before.status === t.status) return;
  const now = Date.now();
  marks.task.set(t.id, { at: now, from: before.status, to: t.status });
  // pending → running is the spawn, already in the feed from the session that appeared.
  if (!(before.status === "pending" && t.status === "running")) {
    const s = t.session ? sessions.get(t.session) : undefined;
    push({
      at: now,
      kind: KIND[t.status] ?? "phase",
      taskId: t.id,
      session: t.session,
      project: t.project,
      provider: s?.provider ?? t.provider,
      model: s?.model ?? t.model,
      title: t.title,
      from: before.status,
      to: t.status,
      attempt: t.attempts,
    });
  }
  changed();
}

export function forgetTask(id: number) {
  tasks.delete(id);
  marks.task.delete(id);
  items = items.filter((a) => a.taskId !== id);
  changed();
}

/** The marks, for the canvas loop (read every frame; do not mutate). */
export function activityMarks(): Marks {
  return marks;
}

/** Calls `listener` for each task worker that starts after the page loaded; returns the unsubscribe function. */
export function onSpawn(listener: (a: Activity) => void): () => void {
  spawnListeners.add(listener);
  return () => {
    spawnListeners.delete(listener);
  };
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/** The feed (newest first) and a version that changes with every mark; re-renders the caller on each change. */
export function useActivity(): { items: Activity[]; version: number } {
  return useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => snapshot,
  );
}

/** Milliseconds since `at`, or `Infinity` with no mark. */
export const ageOf = (at: number | undefined, now: number) => (at === undefined ? Infinity : now - at);

/** The pipeline as a row of steps; `review` waits at the land step. */
export const PIPELINE_STEPS: { key: TaskStatus; label: string }[] = [
  { key: "running", label: "run" },
  { key: "verifying", label: "verify" },
  { key: "auditing", label: "audit" },
  { key: "landing", label: "land" },
  { key: "done", label: "done" },
];

export function stepIndex(status: TaskStatus): number {
  if (status === "review") return 3;
  return PIPELINE_STEPS.findIndex((s) => s.key === status);
}

/** True when the user asked the system for less motion: animations then only change colour and opacity. */
export function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}
