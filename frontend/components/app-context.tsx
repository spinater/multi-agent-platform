"use client";

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import { formatHash, parseHash, type Route, type Screen } from "@/lib/nav";
import { useDashboard, type DashboardState } from "@/lib/use-dashboard";

export type Scope = "project" | "all";
export type ToastTone = "ok" | "warn" | "bad";

export type AppState = DashboardState & {
  /** Milliseconds since the epoch, ticking every second. */
  now: number;
  route: Route;
  go: (screen: Screen, arg?: string | null) => void;
  /** The current project's slug; `null` is the default workspace (`workspace.dir`, tasks without a project). */
  project: string | null;
  setProject: (slug: string | null) => void;
  /** `all` merges every project in the views that support it (command centre). */
  scope: Scope;
  setScope: (scope: Scope) => void;
  /** The current project's info, `null` for the default workspace or while loading. */
  current: ProjectInfo | null;
  /** True when a task or session with this `project` field belongs to the current project. */
  inProject: (project: string | null | undefined) => boolean;
  theme: string;
  toggleTheme: () => void;
  /** Opens the provider sign-in modal (rendered once by the shell). */
  openAuth: (provider: string) => void;
  authProvider: string | null;
  closeAuth: () => void;
  toast: (text: string, tone?: ToastTone) => void;
  toasts: { id: number; text: string; tone: ToastTone }[];
};

const AppContext = createContext<AppState | null>(null);

export function useApp(): AppState {
  const app = useContext(AppContext);
  if (!app) throw new Error("useApp outside <AppProvider>");
  return app;
}

function stored(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private mode: the choice lasts for this page.
  }
}

/** Shared state of every screen: live data, the route, the current project, theme, toasts. Rendered only after mount,
 * so it can read `localStorage` and the clock directly. */
export function AppProvider({ children, now }: { children: ReactNode; now: number }) {
  const dash = useDashboard();
  const [route, setRoute] = useState<Route>(() => parseHash(location.hash));
  // "" is the default workspace in storage.
  const [project, setProjectState] = useState<string | null>(() => stored("map.project", "") || null);
  const [scope, setScopeState] = useState<Scope>(() => (stored("map.scope", "project") === "all" ? "all" : "project"));
  const [theme, setTheme] = useState(() => stored("map.theme", "dark"));
  const [authProvider, setAuthProvider] = useState<string | null>(null);
  const [toasts, setToasts] = useState<AppState["toasts"]>([]);

  useEffect(() => {
    const onHash = () => setRoute(parseHash(location.hash));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    store("map.theme", theme);
  }, [theme]);

  const go = useCallback((screen: Screen, arg: string | null = null) => {
    const next = { screen, arg };
    history.pushState(null, "", formatHash(next));
    setRoute(next);
    window.scrollTo({ top: 0 });
  }, []);

  const setProject = useCallback((slug: string | null) => {
    setProjectState(slug);
    store("map.project", slug ?? "");
  }, []);

  const setScope = useCallback((s: Scope) => {
    setScopeState(s);
    store("map.scope", s);
  }, []);

  const toast = useCallback((text: string, tone: ToastTone = "ok") => {
    const id = Date.now() + Math.random();
    setToasts((list) => [...list, { id, text, tone }]);
    window.setTimeout(() => setToasts((list) => list.filter((t) => t.id !== id)), 4000);
  }, []);

  // A project removed elsewhere falls back to the default workspace.
  const current = dash.projects.find((p) => p.project.slug === project) ?? null;
  const effective = project !== null && dash.projects.length > 0 && !current ? null : project;

  const value = useMemo<AppState>(
    () => ({
      ...dash,
      now,
      route,
      go,
      project: effective,
      setProject,
      scope,
      setScope,
      current,
      inProject: (p) => (p ?? null) === effective,
      theme,
      toggleTheme: () => setTheme((t) => (t === "dark" ? "light" : "dark")),
      openAuth: setAuthProvider,
      authProvider,
      closeAuth: () => setAuthProvider(null),
      toast,
      toasts,
    }),
    [dash, now, route, go, effective, setProject, scope, setScope, current, theme, authProvider, toast, toasts],
  );

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
