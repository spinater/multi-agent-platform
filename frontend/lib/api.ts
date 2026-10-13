import type { Event } from "./generated/Event";
import type { Overview } from "./generated/Overview";
import type { Session } from "./generated/Session";
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
  createSession: (body: { provider: string; model?: string; name?: string; prompt?: string; supervisor?: boolean }) =>
    request<Session>("POST", "/sessions", body),
  deleteSession: (name: string, force = false) =>
    request<{ message: string }>("DELETE", `/sessions/${encodeURIComponent(name)}${force ? "?force=true" : ""}`),
  prompt: (name: string, text: string) => request("POST", `/sessions/${encodeURIComponent(name)}/prompt`, { text }),
  key: (name: string, key: string) => request("POST", `/sessions/${encodeURIComponent(name)}/key`, { key }),
  createTask: (body: { title: string; prompt: string; depends_on: number[]; provider?: string; model?: string }) =>
    request<Task>("POST", "/tasks", body),
  taskAction: (id: number, action: "retry" | "cancel") => request<Task>("PATCH", `/tasks/${id}`, { action }),
  deleteTask: (id: number) => request("DELETE", `/tasks/${id}`),
  setOrchestrator: (running: boolean) => request("POST", "/orchestrator", { running }),
};
