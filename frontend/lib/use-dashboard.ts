"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import { baseline, forgetSession, forgetTask, trackSession, trackTask } from "./activity";
import { ApiError, api, wsUrl } from "./api";
import type { Event } from "./generated/Event";
import type { OrchestratorStatus } from "./generated/OrchestratorStatus";
import type { ProjectInfo } from "./generated/ProjectInfo";
import type { ProviderInfo } from "./generated/ProviderInfo";
import type { ServerInfo } from "./generated/ServerInfo";
import type { ServerMessage } from "./generated/ServerMessage";
import type { Session } from "./generated/Session";
import type { Task } from "./generated/Task";
import { refreshPlans, setPlan } from "./use-plans";
import { setUpdatesReport } from "./use-updates";
import { setUsageReport } from "./use-usage";

export type Connection = "connecting" | "live" | "offline" | "unauthorized";

export type DashboardState = {
  sessions: Session[];
  tasks: Task[];
  providers: ProviderInfo[];
  orchestrator: OrchestratorStatus | null;
  projects: ProjectInfo[];
  events: Event[];
  server: ServerInfo | null;
  connection: Connection;
  reload: () => Promise<void>;
};

const MAX_EVENTS = 300;

/** The mounted dashboard's provider setter, so a sign-in dialog can refresh the list without a reload. */
let providersSink: ((p: ProviderInfo[]) => void) | null = null;

/** Reloads `/api/providers` into the dashboard (after a sign-in finishes). */
export async function refreshProviders(): Promise<void> {
  try {
    const list = await api.providers();
    providersSink?.(list);
  } catch {
    // The next `providers` message or reload brings it.
  }
}

function upsert<T>(list: T[], item: T, same: (a: T) => boolean): T[] {
  const i = list.findIndex(same);
  if (i < 0) return [item, ...list];
  const copy = list.slice();
  copy[i] = item;
  return copy;
}

/** Loads `/api/overview` once, then keeps it current from `/api/ws`, reconnecting with backoff. */
export function useDashboard(): DashboardState {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [orchestrator, setOrchestrator] = useState<OrchestratorStatus | null>(null);
  const [projects, setProjects] = useState<ProjectInfo[]>([]);
  const [events, setEvents] = useState<Event[]>([]);
  const [server, setServer] = useState<ServerInfo | null>(null);
  const [connection, setConnection] = useState<Connection>("connecting");
  const retry = useRef(0);

  useEffect(() => {
    providersSink = setProviders;
    return () => {
      if (providersSink === setProviders) providersSink = null;
    };
  }, []);

  const reload = useCallback(async () => {
    try {
      const [overview, recent] = await Promise.all([api.overview(), api.events()]);
      baseline(overview.tasks, overview.sessions);
      setSessions(overview.sessions);
      setTasks(overview.tasks);
      setProviders(overview.providers);
      setOrchestrator(overview.orchestrator);
      setProjects(overview.projects);
      setServer(overview.server);
      setEvents(recent);
      refreshPlans();
    } catch (err) {
      setConnection(err instanceof ApiError && err.status === 401 ? "unauthorized" : "offline");
      throw err;
    }
  }, []);

  useEffect(() => {
    let socket: WebSocket | null = null;
    let timer: number | undefined;
    let stopped = false;

    const apply = (msg: ServerMessage) => {
      switch (msg.type) {
        case "event":
          setEvents((list) => [msg.event, ...list].slice(0, MAX_EVENTS));
          break;
        case "session":
          trackSession(msg.session);
          setSessions((list) => upsert(list, msg.session, (s) => s.name === msg.session.name));
          break;
        case "session_removed":
          forgetSession(msg.name);
          setSessions((list) => list.filter((s) => s.name !== msg.name));
          break;
        case "task":
          trackTask(msg.task);
          setTasks((list) => upsert(list, msg.task, (t) => t.id === msg.task.id).sort((a, b) => a.id - b.id));
          break;
        case "task_removed":
          forgetTask(msg.id);
          setTasks((list) => list.filter((t) => t.id !== msg.id));
          break;
        case "orchestrator":
          setOrchestrator(msg.status);
          break;
        case "project":
          setProjects((list) => {
            const i = list.findIndex((p) => p.project.slug === msg.project.project.slug);
            if (i < 0) return [...list, msg.project];
            const copy = list.slice();
            copy[i] = msg.project;
            return copy;
          });
          break;
        case "project_removed":
          setProjects((list) => list.filter((p) => p.project.slug !== msg.slug));
          break;
        case "usage":
          setUsageReport(msg.report);
          break;
        case "updates":
          setUpdatesReport(msg.report);
          break;
        case "plan":
          setPlan(msg.plan);
          break;
        case "providers":
          setProviders(msg.providers);
          break;
        case "resync":
          void reload().catch(() => {});
          break;
      }
    };

    const connect = async () => {
      if (stopped) return;
      try {
        await reload();
      } catch {
        schedule();
        return;
      }
      socket = new WebSocket(wsUrl());
      socket.onopen = () => {
        retry.current = 0;
        setConnection("live");
      };
      socket.onmessage = (e) => {
        try {
          apply(JSON.parse(e.data) as ServerMessage);
        } catch {
          // Ignore a malformed frame rather than dropping the connection.
        }
      };
      socket.onclose = () => {
        if (stopped) return;
        setConnection((c) => (c === "unauthorized" ? c : "offline"));
        schedule();
      };
    };

    const schedule = () => {
      const delay = Math.min(15000, 1000 * 2 ** retry.current++);
      timer = window.setTimeout(connect, delay);
    };

    void connect();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
      socket?.close();
    };
  }, [reload]);

  return { sessions, tasks, providers, orchestrator, projects, events, server, connection, reload };
}
