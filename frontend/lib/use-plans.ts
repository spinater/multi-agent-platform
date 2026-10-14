"use client";

import { useSyncExternalStore } from "react";

import { request } from "./api";
import type { Plan } from "./generated/Plan";
import type { PlanApproved } from "./generated/PlanApproved";
import type { PlanEdit } from "./generated/PlanEdit";
import type { PlannerInfo } from "./generated/PlannerInfo";
import type { PlannerInstructions } from "./generated/PlannerInstructions";
import type { Session } from "./generated/Session";

/** Every plan, for the whole page: fetched once from `/api/plans`, then kept current by `plan` messages from the
 * WebSocket (see `use-dashboard.ts`), so the Requirements panels and the draft toast share it. */
type PlansStore = { plans: Plan[] | null; error: string | null };

let state: PlansStore = { plans: null, error: null };
let fetched = false;
const listeners = new Set<() => void>();
/** Called with each draft that arrives after the first load (not for drafts already there). */
const draftListeners = new Set<(plan: Plan) => void>();

function set(next: Partial<PlansStore>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

const newestFirst = (a: Plan, b: Plan) => b.id - a.id;

/** A pushed or returned plan replaces the stored one. */
export function setPlan(plan: Plan) {
  const list = state.plans ?? [];
  const known = list.some((p) => p.id === plan.id);
  set({ plans: (known ? list.map((p) => (p.id === plan.id ? plan : p)) : [plan, ...list]).sort(newestFirst) });
  if (!known && state.plans && plan.status === "draft") draftListeners.forEach((l) => l(plan));
}

/** Subscribes to newly submitted drafts; returns the unsubscribe function. */
export function onNewDraft(listener: (plan: Plan) => void): () => void {
  draftListeners.add(listener);
  return () => {
    draftListeners.delete(listener);
  };
}

function load() {
  fetched = true;
  request<Plan[]>("GET", "/plans")
    .then((plans) => set({ plans: plans.sort(newestFirst), error: null }))
    .catch((err: unknown) => {
      fetched = false;
      set({ error: err instanceof Error ? err.message : String(err) });
    });
}

/** After a reconnect: messages may have been missed. */
export function refreshPlans() {
  if (fetched) load();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!fetched) load();
  return () => {
    listeners.delete(listener);
  };
}

export function usePlans(): PlansStore {
  return useSyncExternalStore(
    subscribe,
    () => state,
    () => state,
  );
}

/** `project: null` is the default workspace. */
const plannerPath = (project: string | null) => (project === null ? "/workspace/planner" : `/projects/${encodeURIComponent(project)}/planner`);

export const plansApi = {
  edit: (id: number, body: PlanEdit) => request<Plan>("PATCH", `/plans/${id}`, body),
  /** Creates the plan's tasks in dependency order; 409 when it is no longer a draft. */
  approve: (id: number) => request<PlanApproved>("POST", `/plans/${id}/approve`, {}),
  reject: (id: number, reason?: string) => request<Plan>("POST", `/plans/${id}/reject`, { reason }),
  planner: (project: string | null) => request<PlannerInfo>("GET", plannerPath(project)),
  /** Starts the planner session, or returns it while it runs. */
  startPlanner: (project: string | null, provider?: string) => request<Session>("POST", plannerPath(project), { provider }),
  instructions: () => request<PlannerInstructions>("GET", "/planner/instructions"),
};
