"use client";

import clsx from "clsx";
import {
  Bot,
  Hexagon,
  Maximize2,
  MessageSquareText,
  Minimize2,
  Scan,
  Search,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { api, managerApi } from "@/lib/api";
import type { Event } from "@/lib/generated/Event";
import type { ManagerInfo } from "@/lib/generated/ManagerInfo";
import { formatHash } from "@/lib/nav";
import type { Connection } from "@/lib/use-dashboard";
import { usePlans } from "@/lib/use-plans";

import { ActivityTicker } from "../activity";
import { ActivityGraphView } from "../activity-graph";
import { useT } from "@/lib/i18n";

import { useApp } from "../app-context";
import { useCommand } from "../command-data";
import { DoneTasksScreen } from "./done-tasks";
import { TONE_BG, type Tone } from "../command-ui";
import { ConsoleModal } from "../console";
import { DetailCard } from "../detail";
import { CommandCanvas, type CanvasControls } from "../graph/brain";
import type { NodeType } from "../graph/model";
import type { View } from "../graph/layouts";
import { LeftColumn, RightColumn } from "../rails";
import { RequirementsPanel } from "../requirements";
import { NewSessionModal } from "../sessions";
import { Modal as ShellModal } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";
import { NewTaskModal } from "../tasks";

const CONNECTION: Record<Connection, [string, Tone]> = {
  connecting: ["connecting", "info"],
  live: ["live", "ok"],
  offline: ["offline", "bad"],
  unauthorized: ["token required", "warn"],
};

const VIEW_TABS: [View, "cc.graph" | "cc.orbit" | "cc.circle"][] = [
  ["graph", "cc.graph"],
  ["circle", "cc.circle"],
  ["orbit", "cc.orbit"],
];

const HINTS: Record<View, string> = {
  circle: "LAYERS FROM THE CENTRE OUT · DRAG = PAN · CLICK = CARD · WHEEL = ZOOM",
  orbit:
    "DRAG = ORBIT · CLICK = CARD · WHEEL = ZOOM · CHIP ISOLATES A PROVIDER",
  graph:
    "LEFT → RIGHT · ORCHESTRATOR · PROVIDERS · SESSIONS · TASKS BY DEPENDENCY DEPTH · PIPELINE",
};

const TYPES: [NodeType, ReactNode, string][] = [
  ["task", "◇", "task"],
  ["session", "○", "session"],
  ["stage", <Hexagon key="h" className="size-3" />, "stage"],
  ["run", "•", "run"],
];

/** A deep link (`sessions/<name>`, `tasks/<id>`) as a node id. */
function nodeFromArg(arg: string | null): string | null {
  if (!arg) return null;
  const [head, ...rest] = arg.split("/");
  const tail = rest.join("/");
  if (head === "sessions" && tail && tail !== "new") return `s:${tail}`;
  if (head === "tasks" && /^\d+$/.test(tail)) return `t:${tail}`;
  if (head === "lanes" && tail) return `l:${tail}`;
  return null;
}

type Modal =
  | { kind: "task" }
  | { kind: "session"; provider?: string }
  | { kind: "console"; name: string }
  | { kind: "requirements" };

/** `tasks/new` and `sessions/new` open the matching form, `requirements` the planner and its plans. */
function modalFromArg(arg: string | null): Modal | null {
  if (arg === "tasks/new") return { kind: "task" };
  if (arg === "sessions/new") return { kind: "session" };
  if (arg === "requirements") return { kind: "requirements" };
  return null;
}

function argFromNode(id: string | null): string | null {
  if (!id) return null;
  if (id.startsWith("s:")) return `sessions/${id.slice(2)}`;
  if (id.startsWith("t:")) return `tasks/${id.slice(2)}`;
  if (id.startsWith("l:")) return `lanes/${id.slice(2)}`;
  return null;
}

/** Shortens `/…/data/.map/map.db` to `.map/map.db`, as in the design's status bar; the full path is the tooltip. */
function shortPath(path: string | undefined, keep = 2): string {
  if (!path) return "";
  const i = path.indexOf("/.map/");
  if (i >= 0) return path.slice(i + 1);
  const parts = path.split("/").filter(Boolean);
  return parts.length > keep ? `…/${parts.slice(-keep).join("/")}` : path;
}

/** Whether the graph shows finished work (exited sessions, cancelled tasks); hidden unless turned on (per browser). */
function storedShowDone(): boolean {
  try {
    return localStorage.getItem("map.command.done") === "1";
  } catch {
    return false;
  }
}

/** The last view picked in each scope; all projects start on the graph (a per-browser convenience). */
function storedView(scope: string): View {
  try {
    const v = localStorage.getItem(`map.command.view.${scope}`);
    if (v === "orbit" || v === "circle" || v === "graph") return v;
  } catch {
    // Storage blocked: use the default.
  }
  return scope === "all" ? "graph" : "orbit";
}

function StatusItem({
  tone,
  children,
  title,
}: {
  tone?: Tone;
  children: ReactNode;
  title?: string;
}) {
  return (
    <span
      className="flex items-center gap-[7px] whitespace-nowrap"
      title={title}
    >
      {tone && (
        <span className={clsx("size-1.5 rounded-full", TONE_BG[tone])} />
      )}
      {children}
    </span>
  );
}

const MANAGER_TONE: Record<string, Tone> = {
  starting: "info",
  running: "ok",
  restarting: "warn",
  stopped: "bad",
  failed: "bad",
  off: "idle",
};

/** The current project's manager (backend manager.rs): its state, re-read when its session changes over the
 * WebSocket. A click opens the project page, where its panel and console are. */
function ManagerChip({ slug }: { slug: string }) {
  const { sessions, server, current } = useApp();
  const enabled = current?.project.settings.manager.enabled ?? false;
  const name = `${server?.session_prefix ?? "map-"}mgr-${slug}`;
  const sessionState = sessions.find((s) => s.name === name)?.state ?? "none";
  const [info, setInfo] = useState<ManagerInfo | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let stop = false;
    managerApi.get(slug).then(
      (i) => !stop && setInfo(i),
      () => {},
    );
    return () => {
      stop = true;
    };
  }, [slug, enabled, sessionState]);
  if (!enabled) return null;
  const state =
    info?.project === slug
      ? info.state
      : sessionState === "none"
        ? "starting"
        : "running";
  const tone = MANAGER_TONE[state] ?? "idle";
  return (
    <a
      href={formatHash({ screen: "projects", arg: slug })}
      title={`This project's manager: ${state}${info?.warning ? ` — ${info.warning}` : ""}. Talk to it on the project page, in its terminal, or from Telegram with /ask ${slug}.`}
      className="flex h-[26px] items-center gap-1.5 self-center rounded-[7px] border border-line-2 px-2.5 font-mono text-[11px] font-semibold whitespace-nowrap text-nav hover:text-text"
    >
      <Bot className="size-3 text-accent" /> manager
      <span className={clsx("size-1.5 rounded-full", TONE_BG[tone])} />
      <span className="text-muted">{state}</span>
    </a>
  );
}

/** `#command/done` is the Done tasks page; every other `#command…` hash is the command centre. */
export function CommandScreen() {
  const { route } = useApp();
  return route.arg === "done" ? <DoneTasksScreen /> : <CommandCentre />;
}

/** The command centre (design: Agent Command Centre.dc.html). `route.arg` deep-links a card: `sessions/<name>`,
 * `tasks/<id>`; `tasks` scrolls to the queue. Done tasks are not shown here (see the Done tasks page), except one
 * opened by its link. */
function CommandCentre() {
  const app = useApp();
  const {
    orchestrator,
    connection,
    now,
    route,
    scope,
    setScope,
    project,
    setProject,
    server,
    toast,
    openAuth,
    theme,
    tasks: everyTask,
    current,
  } = app;
  const tr = useT();
  const [view, setViewState] = useState<View>(() => storedView(scope));
  const [selected, setSelected] = useState<string | null>(() =>
    nodeFromArg(route.arg),
  );
  const [filter, setFilter] = useState("all");
  const [hidden, setHidden] = useState<Partial<Record<NodeType, boolean>>>({});
  const [names, setNames] = useState(true);
  const [motion, setMotion] = useState(true);
  const [showDone, setShowDoneState] = useState(storedShowDone);
  const setShowDone = (on: boolean) => {
    setShowDoneState(on);
    try {
      localStorage.setItem("map.command.done", on ? "1" : "0");
    } catch {
      // Storage blocked: the choice lasts for this page only.
    }
  };
  const [search, setSearch] = useState("");
  const [full, setFull] = useState(false);
  const [modal, setModal] = useState<Modal | null>(() =>
    modalFromArg(route.arg),
  );
  const controls = useRef<CanvasControls | null>(null);
  const pinned = selected?.startsWith("t:") ? Number(selected.slice(2)) : null;
  const cmd = useCommand(pinned, showDone);
  const all = cmd.all;
  // `#command/activity`: the project's live task flow in place of the graph.
  const activity = route.arg === "activity";

  // A new deep link (another screen's "open session") selects its node.
  const [lastArg, setLastArg] = useState(route.arg);
  if (route.arg !== lastArg) {
    setLastArg(route.arg);
    const n = nodeFromArg(route.arg);
    if (n) setSelected(n);
    const m = modalFromArg(route.arg);
    if (m) setModal(m);
  }
  // Switching scope or project resets the view, the filter and cards that no longer apply.
  const [lastScope, setLastScope] = useState(`${scope}|${project ?? ""}`);
  if (`${scope}|${project ?? ""}` !== lastScope) {
    setLastScope(`${scope}|${project ?? ""}`);
    setViewState(storedView(scope));
    setFilter("all");
    if (!selected?.startsWith("s:") && !selected?.startsWith("t:"))
      setSelected(null);
  }

  useEffect(() => {
    if (route.arg === "tasks")
      document.getElementById("cc-tasks")?.scrollIntoView({ block: "start" });
  }, [route.arg]);

  const setView = (v: View) => {
    setViewState(v);
    try {
      localStorage.setItem(`map.command.view.${scope}`, v);
    } catch {
      // Storage blocked: the choice lasts for this page.
    }
  };

  const select = (id: string | null) => {
    setSelected(id);
    history.replaceState(
      null,
      "",
      formatHash({ screen: "command", arg: argFromNode(id) }),
    );
  };

  // A selected node that is gone (session removed, task deleted, out of scope) closes its card.
  const exists = !selected || cmd.graph.nodes.some((n) => n.id === selected);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (modal) setModal(null);
      else setSelected(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [modal]);

  const toggleRun = () => {
    if (!orchestrator) return;
    const next = !orchestrator.running;
    api.setOrchestrator(next).then(
      () =>
        toast(
          next
            ? "Orchestrator running · picks up ready tasks"
            : "Orchestrator paused · running tasks continue, no new starts",
          next ? "ok" : "warn",
        ),
      (err: Error) => toast(err.message, "bad"),
    );
  };
  const openProject = (slug: string | null) => {
    setProject(slug);
    setScope("project");
  };
  const openEvent = (e: Event) => {
    if (e.session && cmd.sessions.some((s) => s.name === e.session))
      select(`s:${e.session}`);
    else if (e.task_id != null && cmd.tasks.some((t) => t.id === e.task_id))
      select(`t:${e.task_id}`);
    else select("orch");
  };
  const fly = (id: string) => controls.current?.fly(id);
  const onSearchKey = (key: string) => {
    if (key !== "Enter") return;
    const q = search.trim().toLowerCase();
    if (!q) return;
    const n = cmd.graph.nodes.find(
      (x) => x.type !== "run" && x.label.toLowerCase().includes(q),
    );
    if (n) {
      select(n.id);
      fly(n.id);
    }
  };

  const drafts = (usePlans().plans ?? []).filter(
    (p) => p.status === "draft" && (p.project ?? null) === project,
  ).length;
  const [connWord, connTone] = CONNECTION[connection];
  const projectLabel =
    project === null
      ? workspaceName(server?.workspace_dir)
      : (current?.project.name ?? project);
  const live = cmd.sessions.filter((s) => s.state !== "exited").length;
  const runs = cmd.graph.nodes.filter((n) => n.type === "run").length;
  const statsLine = `${all ? `${cmd.projectOrder.length} projects · ` : ""}${cmd.tasks.length} tasks · ${live} live sessions · ${cmd.graph.links.length} links · ${runs} runs`;
  const total = cmd.sessions.length + cmd.tasks.length;
  const workspace = all
    ? "all projects"
    : shortPath(
        project === null ? server?.workspace_dir : current?.workspace,
        3,
      );

  const status = (
    <div className="flex flex-wrap items-center gap-x-[18px] gap-y-1.5 font-mono text-[10px] leading-[1.3] font-medium tracking-[0.1em] text-muted uppercase">
      <StatusItem tone={orchestrator?.running ? "accent" : "idle"}>
        core · {orchestrator?.running ? "running" : "paused"}
      </StatusItem>
      <StatusItem tone={connection === "live" ? "ok" : "idle"}>
        watcher · {server?.watcher_interval_secs ?? "–"}s
      </StatusItem>
      <StatusItem tone={connTone}>ws · {connWord}</StatusItem>
      <StatusItem title={server?.database}>
        sqlite · {shortPath(server?.database)}
      </StatusItem>
      <StatusItem
        title={
          all ? undefined : project === null ? server?.workspace_dir : workspace
        }
      >
        <span className="max-w-[260px] truncate">workspace · {workspace}</span>
      </StatusItem>
    </div>
  );

  const tb =
    "flex h-[30px] cursor-pointer items-center gap-1.5 rounded-[7px] border px-[11px] font-mono text-[11px] font-medium tracking-[0.04em] whitespace-nowrap";
  const main = (
    <main
      className={clsx(
        "order-first flex min-w-0 flex-col gap-3 px-4 pt-4 pb-3.5 sm:px-5 lg:order-none lg:min-h-0",
      )}
    >
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1.5">
          <span className="flex items-center gap-1.5 text-[22px] leading-[1.15] font-bold tracking-[0.08em] whitespace-nowrap">
            <Hexagon className="size-[18px] text-accent" strokeWidth={2.5} />{" "}
            MAP
          </span>
          <span className="text-[22px] leading-[1.15] font-medium tracking-[-0.01em] whitespace-nowrap text-accent">
            Multi-Agent Platform
          </span>
          <span className="font-mono text-[10px] leading-[1.15] font-medium tracking-[0.16em] whitespace-nowrap text-muted uppercase">
            command centre
          </span>
          <div
            className="flex self-center overflow-hidden rounded-[7px] border border-line-2"
            role="tablist"
            aria-label="Scope"
          >
            {(
              [
                ["project", projectLabel],
                ["all", "All projects"],
              ] as const
            ).map(([s, label]) => (
              <button
                key={s}
                role="tab"
                aria-selected={scope === s}
                onClick={() => setScope(s)}
                className={clsx(
                  "h-6 max-w-[180px] cursor-pointer truncate px-2.5 font-mono text-[10.5px] font-semibold whitespace-nowrap",
                  scope === s
                    ? "bg-accent text-accent-text"
                    : "text-nav hover:text-text",
                )}
              >
                {label}
              </button>
            ))}
          </div>
          {!all && (
            <button
              onClick={() => setModal({ kind: "requirements" })}
              title="Talk to this project's planner agent; approve the plans it drafts"
              className="flex h-[26px] cursor-pointer items-center gap-1.5 self-center rounded-[7px] border border-accent bg-accent-tint px-2.5 font-mono text-[11px] font-semibold whitespace-nowrap text-accent-soft hover:bg-accent/20"
            >
              <MessageSquareText className="size-3" /> Requirements
              {drafts > 0 && (
                <span className="rounded bg-accent px-1 text-[10px] leading-4 text-accent-text">
                  {drafts}
                </span>
              )}
            </button>
          )}
          {!all && project && <ManagerChip slug={project} />}
        </div>
        <span className="font-mono text-[10.5px] leading-[1.4] tracking-[0.04em] text-muted">
          {statsLine}
          {cmd.doneCount > 0 && (
            <>
              {" · "}
              <a
                href="#command/done"
                className="text-accent-soft hover:underline"
                title="Finished tasks, off the live views"
              >
                {cmd.doneCount} done →
              </a>
            </>
          )}
        </span>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-2.5">
        <div className="flex flex-wrap items-center gap-2.5">
          <span className="font-mono text-[10px] font-medium tracking-[0.2em] text-muted/70">
            {tr("cc.view")}
          </span>
          <div className="flex overflow-hidden rounded-lg border border-line-2">
            {VIEW_TABS.map(([v, label]) => (
              <button
                key={v}
                onClick={() => {
                  setView(v);
                  if (activity) app.go("command", null);
                }}
                className={clsx(
                  "h-[30px] cursor-pointer border-r border-line px-3.5 font-mono text-[11px] font-semibold tracking-[0.1em] whitespace-nowrap last:border-r-0",
                  view === v && !activity
                    ? "bg-accent text-accent-text"
                    : "text-nav hover:text-text",
                )}
              >
                {tr(label)}
              </button>
            ))}
            <button
              onClick={() => app.go("command", "activity")}
              title="The project's task flow, live: who created each task, how the orchestrator handed it on, what each agent used"
              className={clsx(
                "h-[30px] cursor-pointer px-3.5 font-mono text-[11px] font-semibold tracking-[0.1em] whitespace-nowrap",
                activity
                  ? "bg-accent text-accent-text"
                  : "text-nav hover:text-text",
              )}
            >
              {tr("cc.activity")}
            </button>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {(
            [
              ["names", names, setNames],
              ["motion", motion, setMotion],
              ["done", showDone, setShowDone],
            ] as const
          ).map(([label, on, set]) => (
            <button
              key={label}
              onClick={() => set(!on)}
              className={clsx(
                tb,
                on
                  ? "border-accent text-accent-soft"
                  : "border-line-2 text-nav",
              )}
              aria-pressed={on}
            >
              {label}
            </button>
          ))}
          <label className="flex h-[30px] items-center gap-1.5 rounded-[7px] border border-line-2 px-2.5">
            <Search className="size-3 text-muted" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              onKeyDown={(e) => onSearchKey(e.key)}
              placeholder="search"
              aria-label="Search the graph"
              className="w-[110px] bg-transparent font-mono text-[11.5px] text-text outline-none placeholder:text-muted/80"
            />
          </label>
          <button
            onClick={() => controls.current?.fit()}
            className={clsx(tb, "border-line-2 text-nav")}
          >
            <Scan className="size-3" /> fit
          </button>
          <button
            onClick={() => setFull(!full)}
            className={clsx(
              tb,
              full
                ? "border-accent text-accent-soft"
                : "border-line-2 text-nav",
            )}
          >
            {full ? (
              <Minimize2 className="size-3" />
            ) : (
              <Maximize2 className="size-3" />
            )}
            {full ? "exit full" : "full"}
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        {[
          { key: "all", label: "all", color: "var(--text)" },
          ...cmd.groups,
        ].map((g) => {
          const on = filter === g.key;
          const count = g.key === "all" ? total : (cmd.graph.count[g.key] ?? 0);
          return (
            <button
              key={g.key}
              onClick={() => setFilter(on && g.key !== "all" ? "all" : g.key)}
              className={clsx(
                "flex h-[26px] cursor-pointer items-center gap-[7px] rounded-md border px-2.5 font-mono text-[11px] font-medium whitespace-nowrap text-text",
                !on && "border-line-2",
              )}
              style={
                on
                  ? {
                      borderColor: g.color,
                      background: `color-mix(in srgb, ${g.color} 12%, transparent)`,
                    }
                  : undefined
              }
              aria-pressed={on}
            >
              <span
                className="size-[7px] rounded-full"
                style={{ background: g.color }}
              />
              {g.label}
              <span className="text-muted">{count}</span>
            </button>
          );
        })}
        <span className="mx-1.5 h-[18px] w-px bg-line-2" />
        {TYPES.map(([t, glyph, label]) => (
          <button
            key={t}
            onClick={() => setHidden({ ...hidden, [t]: !hidden[t] })}
            className={clsx(
              "flex h-[26px] cursor-pointer items-center gap-1.5 rounded-md border border-line bg-surface px-[9px] font-mono text-[11px] font-medium whitespace-nowrap text-nav",
              hidden[t] && "line-through opacity-35",
            )}
            aria-pressed={!hidden[t]}
          >
            <span className="text-text">{glyph}</span>
            {label}
          </button>
        ))}
      </div>

      <div className="relative flex min-h-0 flex-1 flex-col gap-3">
        <div
          className="relative h-[62vh] max-h-[560px] min-h-[380px] overflow-hidden rounded-[14px] border border-line bg-inset lg:h-auto lg:max-h-none lg:min-h-[420px] lg:flex-1"
          style={{
            backgroundImage:
              "radial-gradient(color-mix(in srgb, var(--text) 5%, transparent) 1px, transparent 1px)",
            backgroundSize: "22px 22px",
          }}
        >
          {activity ? (
            all || !project ? (
              <div className="flex h-full items-center justify-center p-6">
                <span className="max-w-[420px] text-center text-[13px] leading-[1.55] text-text-2">
                  Activity is per project: who made each task, which agent ran
                  it, and what that agent used. Switch the scope to one project
                  to see its flow.
                </span>
              </div>
            ) : (
              <ActivityGraphView
                slug={project}
                motion={motion}
                onSelect={select}
                onOpenConsole={(name) => setModal({ kind: "console", name })}
              />
            )
          ) : (
            <CommandCanvas
              graph={cmd.graph}
              view={view}
              all={all}
              running={!!orchestrator?.running}
              selected={exists ? selected : null}
              onSelect={select}
              filter={filter}
              hidden={hidden}
              names={names}
              motion={motion}
              search={search}
              theme={theme}
              controlsRef={controls}
            />
          )}
          {!activity && <ActivityTicker cmd={cmd} now={now} />}
          {!activity && (
            <div className="pointer-events-none absolute inset-x-0 bottom-3 px-4 text-center font-mono text-[9.5px] leading-[1.4] font-medium tracking-[0.2em] text-muted/70">
              {all
                ? "ALL PROJECTS · READ-ONLY OVERVIEW · CLICK A NODE, THEN OPEN ITS PROJECT TO ACT"
                : HINTS[view]}
            </div>
          )}
        </div>
        {selected && exists && (
          <div className="lg:absolute lg:top-3.5 lg:right-3.5 lg:max-h-[calc(100%-28px)] lg:w-[min(360px,calc(100%-28px))] lg:overflow-auto">
            <DetailCard
              key={selected}
              id={selected}
              cmd={cmd}
              orchestrator={orchestrator}
              select={select}
              fly={fly}
              onClose={() => select(null)}
              onNewTask={() => setModal({ kind: "task" })}
              onNewSession={(provider) =>
                setModal({ kind: "session", provider })
              }
              onOpenConsole={(name) => setModal({ kind: "console", name })}
              onToggleRun={toggleRun}
            />
          </div>
        )}
      </div>
    </main>
  );

  const consoleSession =
    modal?.kind === "console"
      ? app.sessions.find((s) => s.name === modal.name)
      : undefined;

  return (
    <div className="flex min-h-screen flex-col lg:h-screen">
      <TopBar extra={status} />
      <div
        className={clsx(
          "grid min-h-0 flex-1",
          !full && "lg:grid-cols-[300px_minmax(0,1fr)_320px]",
        )}
      >
        {!full && (
          <LeftColumn
            cmd={cmd}
            orchestrator={orchestrator}
            selected={selected}
            select={select}
            onNewTask={() => setModal({ kind: "task" })}
            onToggleRun={toggleRun}
            onSignIn={openAuth}
            onOpenProject={openProject}
            now={now}
          />
        )}
        {main}
        {!full && (
          <RightColumn
            cmd={cmd}
            selected={selected}
            select={select}
            onNewSession={() => setModal({ kind: "session" })}
            onOpenEvent={openEvent}
            now={now}
          />
        )}
      </div>

      {modal?.kind === "task" && (
        <NewTaskModal
          tasks={cmd.tasks}
          nextId={Math.max(0, ...everyTask.map((t) => t.id)) + 1}
          providers={cmd.providers}
          pipeline={cmd.pipeline}
          project={project}
          projectName={projectLabel}
          onClose={() => setModal(null)}
          onCreated={(t) => {
            setModal(null);
            select(`t:${t.id}`);
            toast(
              `#${t.id} ${t.title}${t.depends_on.length ? ` · after ${t.depends_on.map((d) => `#${d}`).join(", ")}` : ""}`,
            );
          }}
        />
      )}
      {modal?.kind === "session" && (
        <NewSessionModal
          providers={cmd.providers}
          stateOf={(p) => cmd.providerState(p, now)}
          prefix={server?.session_prefix ?? "map-"}
          initialProvider={modal.provider}
          project={project}
          projectName={projectLabel}
          onClose={() => setModal(null)}
          onSignIn={openAuth}
          onCreated={(s) => {
            setModal(null);
            select(`s:${s.name}`);
            toast(`Started ${s.name} on ${s.provider} · ${s.model}`);
          }}
        />
      )}
      {consoleSession && (
        <ConsoleModal
          session={consoleSession}
          readOnly={all}
          onClose={() => setModal(null)}
          toast={toast}
        />
      )}
      {modal?.kind === "requirements" && !all && (
        <ShellModal
          onClose={() => setModal(null)}
          label="Requirements"
          width={1240}
        >
          <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-3.5">
            <span className="flex min-w-0 items-center gap-2 font-mono text-[11px] font-semibold tracking-[0.14em] uppercase">
              <MessageSquareText className="size-3 text-accent" />
              <span className="truncate">Requirements · {projectLabel}</span>
            </span>
            <button
              onClick={() => setModal(null)}
              className="size-[26px] flex-none cursor-pointer rounded-md border border-line-2 text-nav"
              aria-label="Close"
            >
              ×
            </button>
          </div>
          <div className="p-[18px]">
            <RequirementsPanel project={project} />
          </div>
        </ShellModal>
      )}
    </div>
  );
}
