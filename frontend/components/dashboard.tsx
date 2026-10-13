"use client";

import clsx from "clsx";
import { Hexagon, Moon, PanelsTopLeft, Sun } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { api, setToken } from "@/lib/api";
import { useDashboard, type Connection } from "@/lib/use-dashboard";

import { SessionConsole } from "./console";
import { EventFeed } from "./events";
import { Brain } from "./graph/brain";
import { buildGraph, type GraphNode } from "./graph/model";
import { ProjectsView } from "./projects";
import { LeftRail, RightRail } from "./rails";
import { NewSessionForm, SessionList } from "./sessions";
import { NewTaskForm, TaskBoard } from "./tasks";
import { Button, Dot, Input, type Tone } from "./ui";

const connectionLabel: Record<Connection, [string, Tone]> = {
  connecting: ["connecting", "info"],
  live: ["live", "ok"],
  offline: ["offline", "bad"],
  unauthorized: ["token required", "warn"],
};

const VIEWS = ["graph", "tasks", "sessions", "projects", "events"] as const;
type CenterView = (typeof VIEWS)[number];

function TokenGate({ onSaved }: { onSaved: () => void }) {
  const [value, setValue] = useState("");
  return (
    <form
      className="mx-auto mt-24 flex max-w-sm flex-col gap-3 rounded-md border border-line bg-panel p-6"
      onSubmit={(e) => {
        e.preventDefault();
        setToken(value.trim());
        onSaved();
      }}
    >
      <h1 className="label text-text">API token</h1>
      <p className="text-[12px] text-muted">This server requires the token from its config file (server.api_token).</p>
      <Input type="password" value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
      <Button variant="primary" type="submit" disabled={!value.trim()}>
        Connect
      </Button>
    </form>
  );
}

function useTheme(): [string, () => void] {
  const [theme, setTheme] = useState(() => {
    try {
      return localStorage.getItem("map.theme") ?? "dark";
    } catch {
      return "dark";
    }
  });
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem("map.theme", theme);
    } catch {
      // Private mode: the choice lasts for this page.
    }
  }, [theme]);
  return [theme, () => setTheme(theme === "dark" ? "light" : "dark")];
}

export function Dashboard() {
  const { sessions, tasks, providers, orchestrator, projects, events, connection } = useDashboard();
  const [view, setView] = useState<CenterView>("graph");
  const [selectedSession, setSelectedSession] = useState<string | null>(null);
  const [selectedProject, setSelectedProject] = useState<string | null>(null);
  const [focusTask, setFocusTask] = useState<number | null>(null);
  const [node, setNode] = useState<string | null>(null);
  const [form, setForm] = useState<{ kind: "session"; provider?: string } | { kind: "task" } | null>(null);
  const [focus, setFocus] = useState(false);
  // `null` until mounted: the static export is rendered at build time, so the clock must not render then.
  const [now, setNow] = useState<number | null>(null);
  const [theme, toggleTheme] = useTheme();
  const graph = useMemo(() => buildGraph(projects, tasks, sessions, providers), [projects, tasks, sessions, providers]);

  useEffect(() => {
    const tick = () => setNow(Date.now());
    // Deep links: #tasks, #sessions/<name>, #projects/<slug>.
    const first = window.setTimeout(() => {
      const [v, arg] = decodeURIComponent(location.hash.slice(1)).split("/");
      if ((VIEWS as readonly string[]).includes(v)) setView(v as CenterView);
      if (v === "projects" && arg) setSelectedProject(arg);
      if (v === "sessions" && arg) setSelectedSession(arg);
      tick();
    }, 0);
    const t = window.setInterval(tick, 1000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(t);
    };
  }, []);

  const mounted = now !== null;
  useEffect(() => {
    if (!mounted) return;
    const arg = view === "projects" ? selectedProject : view === "sessions" ? selectedSession : null;
    history.replaceState(null, "", `#${view}${arg ? `/${encodeURIComponent(arg)}` : ""}`);
  }, [view, selectedProject, selectedSession, mounted]);

  if (connection === "unauthorized") return <TokenGate onSaved={() => location.reload()} />;
  if (now === null) return <p className="label p-6">loading…</p>;

  const current = sessions.find((s) => s.name === selectedSession) ?? null;
  const [label, tone] = connectionLabel[connection];
  const reflexUp = projects.filter((p) => p.reflex.state === "running").length;
  const openSession = (name: string) => {
    setSelectedSession(name);
    setView("sessions");
    setForm(null);
  };
  const openProject = (slug: string) => {
    setSelectedProject(slug);
    setView("projects");
  };
  const openTask = (id: number) => {
    setFocusTask(id);
    setView("tasks");
  };

  const nodeActions = (n: GraphNode) => {
    const btn = (text: string, onClick: () => void, primary = false) => (
      <button key={text} onClick={onClick} className={clsx("h-6 rounded border px-2 text-[11px]", primary ? "border-accent text-accent" : "border-line hover:border-muted")}>
        {text}
      </button>
    );
    switch (n.kind) {
      case "project":
        return [btn("open project", () => openProject(n.ref), true), btn("restart reflex", () => void api.restartReflex(n.ref))];
      case "domain":
        return [btn("open corpus", () => openProject(n.ref.split("/")[0]), true)];
      case "task":
        return [btn("open task", () => openTask(Number(n.ref)), true)];
      case "session":
        return [btn("open console", () => openSession(n.ref), true)];
      case "provider":
        return [btn("start session", () => setForm({ kind: "session", provider: n.ref }), true)];
      default:
        return [btn("new task", () => setForm({ kind: "task" }), true)];
    }
  };

  const center = (
    // First on narrow screens, where the rails stack below it.
    <div className="order-first flex min-h-0 min-w-0 flex-1 flex-col lg:order-none">
      <div className="px-4 pt-4 text-center">
        <h1 className="flex items-center justify-center gap-2 text-lg tracking-[0.2em]">
          <Hexagon className="size-4 text-accent" />
          MULTI-AGENT
          <span className="text-[13px] tracking-normal text-accent">Reflex OS</span>
        </h1>
        <p className="label mt-1">agents · projects · decisions before LLM calls</p>
      </div>
      <nav className="mx-4 mt-3 flex gap-1 overflow-x-auto border-b border-line">
        {VIEWS.map((v) => (
          <button key={v} onClick={() => setView(v)} className={clsx("label px-3 py-2 whitespace-nowrap hover:text-text", view === v && "border-b border-accent text-text")}>
            {v}
            {v === "tasks" && tasks.length > 0 && <span className="ml-1.5 opacity-60">{tasks.length}</span>}
            {v === "sessions" && <span className="ml-1.5 opacity-60">{sessions.filter((s) => s.state !== "exited").length}</span>}
            {v === "projects" && projects.length > 0 && <span className="ml-1.5 opacity-60">{projects.length}</span>}
          </button>
        ))}
      </nav>

      <div className={clsx("min-h-0 flex-1", view === "graph" ? "flex flex-col p-4" : "overflow-auto p-4")}>
        {form?.kind === "session" && (
          <div className="mb-4">
            <NewSessionForm
              key={form.provider ?? ""}
              providers={providers}
              projects={projects}
              initialProvider={form.provider}
              onClose={() => setForm(null)}
              onDone={openSession}
            />
          </div>
        )}
        {form?.kind === "task" && (
          <div className="mb-4">
            <NewTaskForm tasks={tasks} providers={providers} projects={projects} onClose={() => setForm(null)} />
          </div>
        )}
        {view === "graph" && (
          <div className="min-h-[560px] flex-1 overflow-hidden rounded-md border border-line">
            <Brain
              graph={graph}
              selected={node}
              onSelect={(n) => setNode(n?.id ?? null)}
              actions={nodeActions}
              stats={
                <span className="text-muted">
                  · {tasks.length} tasks · {sessions.length} sessions · {projects.reduce((a, p) => a + p.stats.total, 0)} decisions
                </span>
              }
            />
          </div>
        )}
        {view === "tasks" && <TaskBoard tasks={tasks} focus={focusTask} onOpenSession={openSession} onCreate={() => setForm({ kind: "task" })} />}
        {view === "sessions" && (
          <div className="grid gap-4 2xl:grid-cols-2">
            <SessionList sessions={sessions} selected={selectedSession} onSelect={openSession} now={now} onCreate={() => setForm({ kind: "session" })} />
            <SessionConsole key={current?.name ?? "none"} session={current} onClosed={() => setSelectedSession(null)} />
          </div>
        )}
        {view === "projects" && <ProjectsView projects={projects} providers={providers} selected={selectedProject} onSelect={setSelectedProject} />}
        {view === "events" && <EventFeed events={events} onOpenSession={openSession} />}
      </div>
    </div>
  );

  return (
    <div className="flex min-h-screen flex-col">
      <header className="flex flex-wrap items-center gap-x-5 gap-y-1 border-b border-line px-4 py-2 text-[11px] tracking-[0.12em] uppercase">
        <span className="flex items-center gap-1.5">
          <Dot tone={orchestrator?.running ? "ok" : "idle"} />
          orchestrator · <span className="text-text">{orchestrator?.running ? "running" : "paused"}</span>
        </span>
        <span className="flex items-center gap-1.5 text-muted">
          <Dot tone={tone} />
          link · <span className="text-text">{label}</span>
        </span>
        <span className="flex items-center gap-1.5 text-muted">
          <Dot tone={projects.length === 0 ? "idle" : reflexUp === projects.length ? "ok" : "warn"} />
          reflex ·{" "}
          <span className="text-text">
            {reflexUp}/{projects.length}
          </span>
        </span>
        {providers.map((p) => (
          <span key={p.name} className="hidden items-center gap-1.5 text-muted md:flex">
            <Dot tone={!p.available ? "idle" : p.cooldown_until ? "warn" : "ok"} />
            {p.name}
          </span>
        ))}
        <span className="flex-1" />
        <span className="text-muted">{new Date(now).toLocaleDateString(undefined, { day: "2-digit", month: "2-digit" })}</span>
        <button className="flex items-center gap-1 rounded border border-line px-2 py-0.5 hover:border-muted" onClick={() => setForm({ kind: "task" })}>
          new task
        </button>
        <button className="flex items-center gap-1 rounded border border-line px-2 py-0.5 hover:border-muted" onClick={toggleTheme} aria-label="Toggle theme">
          {theme === "dark" ? <Sun className="size-3" /> : <Moon className="size-3" />}
        </button>
        <button className={clsx("flex items-center gap-1 rounded border px-2 py-0.5", focus ? "border-accent text-accent" : "border-line hover:border-muted")} onClick={() => setFocus(!focus)}>
          <PanelsTopLeft className="size-3" /> focus
        </button>
      </header>

      <div className={clsx("grid min-h-0 flex-1", focus ? "grid-cols-1" : "lg:grid-cols-[300px_minmax(0,1fr)_300px]")}>
        {!focus && <LeftRail now={now} tasks={tasks} sessions={sessions} projects={projects} orchestrator={orchestrator} onOpenTask={openTask} />}
        {center}
        {!focus && (
          <RightRail
            now={now}
            sessions={sessions}
            providers={providers}
            projects={projects}
            onOpenSession={openSession}
            onStartSession={(provider) => setForm({ kind: "session", provider })}
            onOpenProject={openProject}
          />
        )}
      </div>
    </div>
  );
}
