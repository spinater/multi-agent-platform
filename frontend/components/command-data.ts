"use client";

import { useEffect, useMemo, useState } from "react";

import { api } from "@/lib/api";
import { projectColor, providerColor } from "@/lib/colors";
import type { Event } from "@/lib/generated/Event";
import type { PipelineInfo } from "@/lib/generated/PipelineInfo";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Session } from "@/lib/generated/Session";
import type { Task } from "@/lib/generated/Task";

import { useApp } from "./app-context";
import { SESSION_TONE, type Tone } from "./command-ui";
import { QUEUE, buildGraph, buildStages, type Graph, type Group } from "./graph/model";
import { workspaceName } from "./shell/top-bar";

/** Key of a project in the all-projects groups; the default workspace has no slug. */
export const WS_KEY = "~";
export const projectKey = (slug: string | null | undefined) => slug ?? WS_KEY;
export const keySlug = (key: string) => (key === WS_KEY ? null : key);

const LIVE_BUSY = ["running", "verifying", "auditing"];
export const NEEDS_YOU = ["waiting_input", "needs_auth", "stalled", "error"] as const;
const NEED_TAG: Record<string, string> = { waiting_input: "input", needs_auth: "sign in", stalled: "stalled", error: "error" };

export type ProviderState = { word: string; tone: Tone; signIn: boolean; signedIn: boolean | null };

/** From the provider's credential file; `null` when this provider has no known one. */
export function signedIn(p: ProviderInfo): boolean | null {
  return p.signed_in;
}

export type Need = { id: string; title: string; reason: string; tag: string; tone: Tone };

export type ProjectSummary = {
  key: string;
  slug: string | null;
  name: string;
  color: string;
  state: "running" | "paused" | "idle";
  live: number;
  running: number;
  done: number;
  total: number;
  need: number;
  counts: Record<string, number>;
};

/** Everything the command centre shows, scoped to the current project or merged across all of them. */
export function useCommand() {
  const app = useApp();
  const { sessions: allSessions, tasks: allTasks, events: allEvents, providers, projects, server, scope, project, orchestrator } = app;
  const all = scope === "all";
  const [pipeline, setPipeline] = useState<PipelineInfo | null>(null);

  useEffect(() => {
    let stop = false;
    // Older servers have no /pipeline: the column then shows the phases without commands.
    api.pipeline().then(
      (p) => !stop && setPipeline(p),
      () => {},
    );
    return () => {
      stop = true;
    };
  }, []);

  return useMemo(() => {
    const inProject = (p: string | null | undefined) => (p ?? null) === project;
    const projectOrder: (string | null)[] = [null, ...projects.map((p) => p.project.slug)];
    const projectName = (slug: string | null | undefined) =>
      slug == null ? workspaceName(server?.workspace_dir) : (projects.find((p) => p.project.slug === slug)?.project.name ?? slug);
    const colorOfProject = (slug: string | null | undefined) => projectColor(slug ?? null, projectOrder);

    const sessions = all ? allSessions : allSessions.filter((s) => inProject(s.project));
    const tasks = all ? allTasks : allTasks.filter((t) => inProject(t.project));
    const byName = new Map(allSessions.map((s) => [s.name, s]));
    const byId = new Map(allTasks.map((t) => [t.id, t]));
    /** `undefined`: an event of the whole platform (orchestrator, a removed session). */
    const eventProject = (e: Event): string | null | undefined => {
      const s = e.session ? byName.get(e.session) : undefined;
      if (s) return s.project;
      const t = e.task_id != null ? byId.get(e.task_id) : undefined;
      if (t) return t.project;
      return undefined;
    };
    const events = all ? allEvents : allEvents.filter((e) => {
      const p = eventProject(e);
      return p === undefined || inProject(p);
    });

    // Providers in the orchestrator's order, then the rest of the configured ones.
    const order = pipeline?.provider_order ?? [];
    const provs = [...providers].sort((a, b) => {
      const ia = order.indexOf(a.name);
      const ib = order.indexOf(b.name);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
    });
    const providerState = (p: ProviderInfo, now: number): ProviderState => {
      const mine = sessions.filter((s) => s.provider === p.name && s.state !== "exited");
      const signed = signedIn(p);
      if (!p.available) return { word: "missing", tone: "idle", signIn: false, signedIn: signed };
      if (signed === false || mine.some((s) => s.state === "needs_auth")) return { word: "signed out", tone: "bad", signIn: true, signedIn: signed };
      if (mine.some((s) => s.state === "rate_limited")) return { word: "rate limited", tone: "warn", signIn: false, signedIn: signed };
      if (p.cooldown_until && new Date(p.cooldown_until).getTime() > now) return { word: "cooldown", tone: "warn", signIn: false, signedIn: signed };
      return { word: "ready", tone: "ok", signIn: false, signedIn: signed };
    };

    const sessionLabel = (s: Session) => s.name;
    const taskLabel = (t: Task) => (all ? `${projectName(t.project)} #${t.id}` : `#${t.id}`);

    let groups: Group[];
    if (all) {
      groups = projectOrder.map((slug) => ({ key: projectKey(slug), label: projectName(slug), color: colorOfProject(slug), kind: "project" as const }));
    } else {
      groups = provs.map((p) => ({ key: p.name, label: p.name, color: providerColor(p.name), kind: "provider" as const, available: p.available }));
      for (const s of sessions) if (!groups.some((g) => g.key === s.provider)) groups.push({ key: s.provider, label: s.provider, color: providerColor(s.provider), kind: "provider", available: false });
      groups.push({ key: QUEUE, label: "queue", color: "var(--idle)", kind: "queue" });
    }
    const groupKeys = new Set(groups.map((g) => g.key));
    const stages = buildStages(pipeline, server?.database);
    const graph: Graph = buildGraph({
      sessions,
      tasks,
      events,
      groups,
      stages,
      groupOfSession: (s) => (all ? projectKey(s.project) : s.provider),
      groupOfLooseTask: (t) => (all ? projectKey(t.project) : t.provider && groupKeys.has(t.provider) && t.status === "done" ? t.provider : QUEUE),
      taskLabel,
      sessionLabel,
    });

    const needs: Need[] = [];
    for (const s of sessions)
      if ((NEEDS_YOU as readonly string[]).includes(s.state))
        needs.push({ id: `s:${s.name}`, title: s.name, reason: s.detail || s.state.replace("_", " "), tag: NEED_TAG[s.state], tone: SESSION_TONE[s.state] });
    for (const t of tasks) if (t.status === "failed") needs.push({ id: `t:${t.id}`, title: `${taskLabel(t)} ${t.title}`, reason: t.result?.split("\n")[0] || "failed", tag: "failed", tone: "bad" });

    const summary = (slug: string | null): ProjectSummary => {
      const ts = allTasks.filter((t) => (t.project ?? null) === slug);
      const ss = allSessions.filter((s) => (s.project ?? null) === slug);
      const live = ss.filter((s) => s.state !== "exited");
      const counts: Record<string, number> = {};
      for (const t of ts) counts[t.status] = (counts[t.status] ?? 0) + 1;
      const busy = live.length > 0 || ts.some((t) => LIVE_BUSY.includes(t.status));
      const state = !ts.length && !live.length ? "idle" : !orchestrator?.running ? "paused" : busy ? "running" : "idle";
      const need = ss.filter((s) => (NEEDS_YOU as readonly string[]).includes(s.state)).length + (counts.failed ?? 0);
      return { key: projectKey(slug), slug, name: projectName(slug), color: colorOfProject(slug), state, live: live.length, running: counts.running ?? 0, done: counts.done ?? 0, total: ts.length, need, counts };
    };

    return {
      all,
      sessions,
      tasks,
      events,
      providers: provs,
      providerState,
      pipeline,
      graph,
      groups,
      needs,
      projectOrder,
      projectName,
      colorOfProject,
      taskLabel,
      eventProject,
      summary,
      active: tasks.filter((t) => LIVE_BUSY.includes(t.status)).length,
    };
  }, [all, allSessions, allTasks, allEvents, providers, projects, server, project, orchestrator, pipeline]);
}

export type Command = ReturnType<typeof useCommand>;
