"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import { ApiError, api, wsUrl } from "./api";
import type { Event } from "./generated/Event";
import type { OrchestratorStatus } from "./generated/OrchestratorStatus";
import type { ProjectInfo } from "./generated/ProjectInfo";
import type { ProviderInfo } from "./generated/ProviderInfo";
import type { ServerMessage } from "./generated/ServerMessage";
import type { Session } from "./generated/Session";
import type { Task } from "./generated/Task";

export type Connection = "connecting" | "live" | "offline" | "unauthorized";

export type DashboardState = {
  sessions: Session[];
  tasks: Task[];
  providers: ProviderInfo[];
  orchestrator: OrchestratorStatus | null;
  projects: ProjectInfo[];
  events: Event[];
  connection: Connection;
  reload: () => Promise<void>;
};

const MAX_EVENTS = 300;

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
  const [connection, setConnection] = useState<Connection>("connecting");
  const retry = useRef(0);

  const reload = useCallback(async () => {
    try {
      const [overview, recent] = await Promise.all([api.overview(), api.events()]);
      setSessions(overview.sessions);
      setTasks(overview.tasks);
      setProviders(overview.providers);
      setOrchestrator(overview.orchestrator);
      setProjects(overview.projects);
      setEvents(recent);
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
          setSessions((list) => upsert(list, msg.session, (s) => s.name === msg.session.name));
          break;
        case "session_removed":
          setSessions((list) => list.filter((s) => s.name !== msg.name));
          break;
        case "task":
          setTasks((list) => upsert(list, msg.task, (t) => t.id === msg.task.id).sort((a, b) => a.id - b.id));
          break;
        case "task_removed":
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

  return { sessions, tasks, providers, orchestrator, projects, events, connection, reload };
}
