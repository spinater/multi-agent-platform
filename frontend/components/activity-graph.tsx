"use client";

/** The Activity view of the command centre (`#command/activity`, backend trace.rs): the project's task flow, live.
 * Each task is one row in fixed columns: who created it (you, the manager, the planner) → the task → the
 * orchestrator's pick with the gates that routed it → the agent session → verify → audit → land → done or failed.
 * A retry loops from verify back to the pick; dashed edges in the left gutter lead to the tasks a task unblocked;
 * lanes the manager opened sit in a band above the rows. Under each agent are counts of what it used (from its
 * CLI transcript); clicking the agent expands them into pills, focuses its row and opens its card. It reloads when
 * a WebSocket event touches the project; running agents and the active stage pulse, and a dot runs along the
 * hand-off in progress. */

import { useEffect, useMemo, useRef, useState } from "react";

import { activityApi } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { ActivityGraph } from "@/lib/generated/ActivityGraph";
import type { ActivityNode } from "@/lib/generated/ActivityNode";
import type { Task } from "@/lib/generated/Task";
import { isHeld } from "@/lib/intake";

import { useApp } from "./app-context";

// Columns: [x, width] on the 1250-wide canvas.
const COL = {
  creator: [20, 132],
  task: [190, 196],
  pick: [424, 176],
  agent: [638, 186],
  verify: [862, 86],
  audit: [960, 86],
  land: [1058, 86],
  end: [1156, 74],
} as const;
type ColKey = keyof typeof COL;
const W = 1250;
const ROW = 78;
const TRACE_LINE = 32;
const TOP = 34;
const NH = 44;
const LANE_BAND = 64;
const STAGES = ["verify", "audit", "land"] as const;
type Stage = (typeof STAGES)[number];

type Range = "1h" | "24h" | "all";
const RANGE_MS: Record<Range, number> = { "1h": 3600_000, "24h": 86_400_000, all: Infinity };

/** Usage kinds drawn as pills, with their glyph, colour and name; `tool` (Read, Edit…) is left out. */
const USE = {
  command: { glyph: "⌘", color: "var(--text)", ring: "var(--idle)", name: "commands" },
  skill: { glyph: "✦", color: "var(--p-gemini)", ring: "var(--p-gemini)", name: "skills" },
  knowledge: { glyph: "◆", color: "var(--accent)", ring: "var(--accent)", name: "knowledge" },
  subagent: { glyph: "⇢", color: "var(--info)", ring: "var(--info)", name: "sub-agents" },
} as const;
type UseKind = keyof typeof USE;
const USE_ORDER: UseKind[] = ["command", "skill", "knowledge", "subagent"];

const TASK_COLOR: Record<string, string> = {
  pending: "var(--idle)",
  running: "var(--ok)",
  verifying: "var(--info)",
  auditing: "var(--info)",
  landing: "var(--info)",
  review: "var(--warn)",
  done: "var(--ok)",
  failed: "var(--bad)",
  cancelled: "var(--idle)",
};

const tint = (c: string, pct: number) => `color-mix(in srgb, ${c} ${pct}%, transparent)`;

function elapsed(from: string | null, now: number): string {
  if (!from) return "";
  const s = Math.max(0, Math.round((now - new Date(from).getTime()) / 1000));
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
}

function ago(at: string | null | undefined, now: number): string {
  if (!at) return "";
  const m = Math.max(0, Math.round((now - new Date(at).getTime()) / 60_000));
  return m < 60 ? `${m}m ago` : m < 1440 ? `${Math.round(m / 60)}h ago` : `${Math.round(m / 1440)}d ago`;
}

const bez = (x1: number, y1: number, x2: number, y2: number) => {
  const dx = Math.max(30, (x2 - x1) * 0.5);
  return `M${x1} ${y1} C${x1 + dx} ${y1},${x2 - dx} ${y2},${x2} ${y2}`;
};
const line = (x1: number, x2: number, y: number) => `M${x1} ${y} L${x2 - 2} ${y}`;

type StageState = "done" | "active" | "fail" | "skip" | "future" | "waiting";

type Row = {
  task: ActivityNode;
  t: Task | undefined;
  pick: ActivityNode | undefined;
  agent: ActivityNode | undefined;
  attempts: number;
  stages: Record<Stage, { node?: ActivityNode; state: StageState; label: string }>;
  end: ActivityNode | undefined;
  uses: ActivityNode[];
  y: number;
  open: boolean;
  lines: number;
};

type Pop =
  | { kind: "use"; use: ActivityNode; agent: ActivityNode }
  | { kind: "manager" }
  | { kind: "lane"; lane: ActivityNode };

/** The latest attempt's state at each stage. A done task passed every stage it has no event for. */
function stageStates(t: Task | undefined, nodes: ActivityNode[], status: string): Row["stages"] {
  const out = {} as Row["stages"];
  for (const g of STAGES) {
    const n = nodes.findLast((x) => x.label === g || (g === "land" && x.label === "review"));
    const state: StageState = !n
      ? status === "done"
        ? "skip"
        : "future"
      : n.state === "ok"
        ? "done"
        : n.state === "fail"
          ? "fail"
          : n.state === "waiting" || n.label === "review"
            ? "waiting"
            : "active";
    const label = { done: `✓ ${g}`, active: `${g}…`, fail: `✕ ${g}`, skip: g, future: g, waiting: n?.label === "review" ? "review" : `${g}…` }[state];
    out[g] = { node: n, state, label };
  }
  // A failed task without a failed stage failed at the one it reached (or verify).
  if (t?.status === "failed" && !STAGES.some((g) => out[g].state === "fail")) {
    const at = STAGES.find((g) => out[g].state === "active" || out[g].state === "future") ?? "verify";
    out[at] = { ...out[at], state: "fail", label: `✕ ${at}` };
    for (const g of STAGES.slice(STAGES.indexOf(at) + 1)) out[g] = { ...out[g], state: "future", label: g };
  }
  return out;
}

export function ActivityGraphView({
  slug,
  motion,
  onSelect,
  onOpenConsole,
}: {
  slug: string;
  motion: boolean;
  /** Opens a node's card in the command centre: `t:<id>` or `s:<session>`. */
  onSelect: (id: string) => void;
  onOpenConsole: (session: string) => void;
}) {
  const { events, now, go, tasks: allTasks } = useApp();
  const [range, setRange] = useState<Range>("24h");
  const [showDone, setShowDone] = useState(false);
  const [graph, setGraph] = useState<ActivityGraph | null>(null);
  const [error, setError] = useState("");
  const [focus, setFocus] = useState<string | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [pop, setPop] = useState<Pop | null>(null);
  const [zoom, setZoom] = useState<number | null>(null);
  const [fit, setFit] = useState(1);
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = wrap.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setFit(Math.max(0.6, Math.min(1, (el.clientWidth - 16) / W))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Reload on the range, and whenever a WebSocket event arrives: no polling of its own.
  const latest = events[0]?.id ?? 0;
  const loadedFor = useRef("");
  const timer = useRef<number | null>(null);
  useEffect(() => {
    const key = `${slug}|${range}`;
    const first = loadedFor.current !== key;
    loadedFor.current = key;
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(
      () => {
        const since = range === "all" ? undefined : new Date(Date.now() - RANGE_MS[range]).toISOString();
        activityApi.project(slug, { since, all: range === "all" }).then(
          (g) => {
            setGraph(g);
            setError("");
          },
          (e: Error) => setError(e.message),
        );
      },
      first ? 0 : 800,
    );
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
    };
  }, [slug, range, latest]);

  const projectTasks = useMemo(() => allTasks.filter((t) => t.project === slug), [allTasks, slug]);
  const taskById = useMemo(() => new Map(projectTasks.map((t) => [t.id, t])), [projectTasks]);

  const view = useMemo(() => {
    if (!graph) return null;
    const byRow = new Map<number, ActivityNode[]>();
    const usesOf = new Map<string, ActivityNode[]>();
    for (const n of graph.nodes) {
      if (n.parent) {
        if (n.kind in USE) usesOf.set(n.parent, [...(usesOf.get(n.parent) ?? []), n]);
        continue;
      }
      byRow.set(n.row, [...(byRow.get(n.row) ?? []), n]);
    }
    for (const list of usesOf.values())
      list.sort((a, b) => USE_ORDER.indexOf(a.kind as UseKind) - USE_ORDER.indexOf(b.kind as UseKind) || b.count - a.count);
    const creators = graph.nodes.filter((n) => n.layer === 0 && ["owner", "manager", "planner"].includes(n.kind));
    const lanes = graph.nodes.filter((n) => n.kind === "agent" && n.task_id == null && !n.parent);
    const laneFrom = new Map(graph.edges.filter((e) => e.kind === "spawned" && e.label === "lane").map((e) => [e.to, e.from]));
    const creatorOf = new Map(graph.edges.filter((e) => e.kind === "created").map((e) => [e.to, e.from]));

    let hiddenDone = 0;
    const taskNodes = graph.nodes
      .filter((n) => n.kind === "task")
      .filter((n) => {
        const finished = n.state === "done" || n.state === "cancelled";
        // Done tasks stay two minutes after they finish, then hide unless "show done".
        const fresh = n.last_at && now - new Date(n.last_at).getTime() < 120_000;
        if (finished && !showDone && !fresh) {
          hiddenDone++;
          return false;
        }
        return true;
      })
      .sort((a, b) => (a.task_id ?? 0) - (b.task_id ?? 0));

    let y = TOP + 30;
    const laneY = y;
    if (lanes.length) y += LANE_BAND;
    const rows: Row[] = [];
    for (const task of taskNodes) {
      const list = (byRow.get(task.row) ?? []).sort((a, b) => a.layer - b.layer);
      const agents = list.filter((n) => n.kind === "agent");
      const agent = agents.at(-1);
      const pick = list.filter((n) => n.kind === "orchestrator").at(-1);
      const after = list.filter((n) => n.kind === "step" && n.layer > (agent?.layer ?? 0));
      const end = list.find((n) => n.kind === "step" && ["done", "failed", "cancelled"].includes(n.label));
      const t = task.task_id != null ? taskById.get(task.task_id) : undefined;
      const status = t?.status ?? task.state ?? "pending";
      const uses = agent ? (usesOf.get(agent.id) ?? []) : [];
      const isOpen = !!(agent && open[agent.id]);
      // Pills wrap at the canvas edge; count the lines they take.
      let lines = 0;
      if (isOpen && uses.length) {
        let x = COL.agent[0] - 40;
        lines = 1;
        for (const u of uses) {
          const w = pillWidth(u);
          if (x + w > W - 10) {
            x = COL.agent[0] - 40;
            lines++;
          }
          x += w + 10;
        }
      }
      const row: Row = {
        task,
        t,
        pick,
        agent,
        attempts: Math.max(agents.length, t?.attempts ?? 0),
        stages: stageStates(t, after, status),
        end,
        uses,
        y,
        open: isOpen,
        lines,
      };
      y += ROW + (lines ? 30 + lines * TRACE_LINE : 0);
      rows.push(row);
    }
    const height = Math.max(y + 20, 320);

    // Creators sit level with the tasks they made, at least 64 apart; the manager below the lanes band.
    const cy = (r: Row) => r.y + NH / 2;
    const cpos = new Map<string, number>();
    for (const c of creators) {
      const rs = rows.filter((r) => (creatorOf.get(r.task.id) ?? "owner") === c.id);
      if (rs.length) cpos.set(c.id, Math.min(rs.reduce((a, r) => a + cy(r), 0) / rs.length, cy(rs[0]) + 40));
      else if (c.id === "manager" && lanes.length) cpos.set(c.id, laneY + 70);
      else if (c.id !== "owner" || lanes.some((l) => laneFrom.get(l.id) === "owner")) cpos.set(c.id, laneY + 70);
    }
    if (cpos.has("manager") && lanes.length) cpos.set("manager", Math.max(cpos.get("manager")!, laneY + 70));
    const order = [...cpos.keys()].sort((a, b) => cpos.get(a)! - cpos.get(b)!);
    for (let i = 1; i < order.length; i++)
      if (cpos.get(order[i])! - cpos.get(order[i - 1])! < 64) cpos.set(order[i], cpos.get(order[i - 1])! + 64);
    const maxC = Math.max(0, ...[...cpos.values()].map((v) => v + 40));

    return {
      rows,
      creators: creators.filter((c) => cpos.has(c.id)),
      cpos,
      creatorOf,
      lanes,
      laneFrom,
      laneY,
      height: Math.max(height, maxC),
      hiddenDone,
    };
  }, [graph, now, showDone, open, taskById]);

  const sinceMs = RANGE_MS[range];
  const hiddenOld = projectTasks.filter(
    (t) => ["done", "failed", "cancelled"].includes(t.status) && now - new Date(t.updated_at).getTime() > sinceMs,
  ).length;

  const scale = zoom ?? fit;
  const agentsInView = view?.rows.filter((r) => r.agent) ?? [];
  const allOpen = agentsInView.length > 0 && agentsInView.every((r) => r.open);

  const toggleBtn = (label: string, on: boolean, fn: () => void) => (
    <button
      key={label}
      onClick={fn}
      aria-pressed={on}
      className="flex h-[26px] cursor-pointer items-center gap-1.5 rounded-md border px-2.5 font-mono text-[11px] font-medium whitespace-nowrap"
      style={{
        borderColor: on ? "var(--accent)" : "var(--line-2)",
        background: on ? "var(--accent-tint)" : "transparent",
        color: on ? "var(--accent-soft)" : "var(--text-2)",
      }}
    >
      <span>{on ? "✓" : "+"}</span>
      {label}
    </button>
  );

  const focusRow = view?.rows.find((r) => r.agent?.id === focus);
  const toolbar = (
    <div className="flex flex-none flex-wrap items-center gap-x-3 gap-y-2 border-b border-line px-3.5 py-2.5">
      <div className="flex gap-0.5 rounded-[7px] border border-line-2 p-0.5">
        {(["1h", "24h", "all"] as Range[]).map((r) => (
          <button
            key={r}
            onClick={() => setRange(r)}
            className="h-[22px] cursor-pointer rounded-[5px] px-2.5 font-mono text-[11px] font-medium"
            style={{
              background: range === r ? "color-mix(in srgb, var(--text) 8%, transparent)" : "transparent",
              color: range === r ? "var(--text)" : "var(--muted)",
            }}
          >
            {r}
          </button>
        ))}
      </div>
      {toggleBtn("show done", showDone, () => setShowDone(!showDone))}
      {toggleBtn("expand all", allOpen, () => {
        const o: Record<string, boolean> = {};
        for (const r of agentsInView) o[r.agent!.id] = !allOpen;
        setOpen(o);
      })}
      {view && (
        <span className="font-mono text-[11px] text-muted">
          {view.rows.length} tasks · {agentsInView.length} agents · {view.hiddenDone + hiddenOld} hidden
        </span>
      )}
      {error && <span className="font-mono text-[11px] text-bad">{error}</span>}
      <div className="ml-auto flex items-center gap-1">
        {focusRow?.agent && (
          <button
            onClick={() => setFocus(null)}
            className="h-[26px] cursor-pointer rounded-md border px-2.5 font-mono text-[11px] font-medium whitespace-nowrap"
            style={{ borderColor: tint("var(--accent)", 50), background: tint("var(--accent)", 10), color: "var(--accent-soft)" }}
          >
            agent · {focusRow.agent.provider} {focusRow.agent.session ?? focusRow.agent.label} ×
          </button>
        )}
        {(
          [
            ["−", () => setZoom(Math.max(0.45, scale - 0.1)), "Zoom out"],
            ["fit", () => setZoom(null), "Fit to width"],
            ["+", () => setZoom(Math.min(1.5, scale + 0.1)), "Zoom in"],
          ] as const
        ).map(([l, fn, title]) => (
          <button
            key={l}
            onClick={fn}
            title={title}
            className="h-[26px] min-w-[26px] cursor-pointer rounded-md border border-line-2 px-1.5 font-mono text-[11px] font-medium text-nav"
          >
            {l}
          </button>
        ))}
      </div>
    </div>
  );

  let body: React.ReactNode;
  if (!view) body = <p className="p-6 font-mono text-[12px] text-muted">{error ? "" : "loading the activity…"}</p>;
  else if (view.rows.length === 0 && view.lanes.length === 0) {
    const text =
      projectTasks.length === 0
        ? "No tasks in this project yet."
        : `Nothing in the last ${range === "all" ? "while" : range}${
            view.hiddenDone && !showDone ? `: ${view.hiddenDone} done task${view.hiddenDone === 1 ? " is" : "s are"} hidden` : ""
          }${hiddenOld && range !== "all" ? `${view.hiddenDone && !showDone ? ", " : ": "}${hiddenOld} older` : ""}.`;
    const btns: [string, () => void][] = [];
    if (view.hiddenDone && !showDone) btns.push(["Show done", () => setShowDone(true)]);
    if (range !== "all" && hiddenOld) btns.push(["Show all time", () => setRange("all")]);
    if (view.hiddenDone && !showDone && range !== "all")
      btns.push([
        "Both",
        () => {
          setRange("all");
          setShowDone(true);
        },
      ]);
    body = (
      <div className="flex flex-1 flex-col items-center justify-center gap-3.5 p-6">
        <span className="max-w-[420px] text-center text-[13px] leading-[1.55] text-text-2">{text}</span>
        <div className="flex flex-wrap justify-center gap-2">
          {btns.map(([l, fn]) => (
            <button key={l} onClick={fn} className="h-[30px] cursor-pointer rounded-[7px] border border-line-3 px-3 text-[12px] font-medium text-text">
              {l}
            </button>
          ))}
        </div>
      </div>
    );
  } else body = canvas();

  function canvas() {
    const v = view!;
    const nodes: React.ReactNode[] = [];
    const edges: React.ReactNode[] = [];
    const labels: React.ReactNode[] = [];
    const extras: React.ReactNode[] = [];
    let key = 0;
    const edge = (
      d: string,
      kind: "done" | "active" | "future" | "fail" | "dep" | "creator",
      op: number,
      extra: { stroke?: string; dash?: string; sw?: number; marker?: boolean } = {},
    ) => {
      const s = {
        done: { stroke: "var(--line-3)", dash: undefined, sw: 1.3, m: "ag-arr" },
        active: { stroke: "var(--accent)", dash: undefined, sw: 1.8, m: "ag-arr-a" },
        future: { stroke: "var(--line-2)", dash: "3 4", sw: 1.3, m: "" },
        fail: { stroke: "var(--bad)", dash: undefined, sw: 1.3, m: "ag-arr-r" },
        dep: { stroke: tint("var(--p-gemini)", 60), dash: "4 4", sw: 1.3, m: "ag-arr" },
        creator: { stroke: "var(--line-3)", dash: undefined, sw: 1.1, m: "ag-arr" },
      }[kind];
      const marker = extra.marker === false ? "" : s.m;
      edges.push(
        <path
          key={key++}
          d={d}
          fill="none"
          style={{ stroke: extra.stroke ?? s.stroke }}
          strokeWidth={extra.sw ?? s.sw}
          strokeDasharray={extra.dash ?? s.dash}
          opacity={op}
          markerEnd={marker ? `url(#${marker})` : undefined}
        />,
      );
      if (kind === "active" && motion)
        edges.push(
          <circle key={key++} r={3.5} style={{ fill: "var(--accent)" }}>
            <animateMotion dur="1.6s" repeatCount="indefinite" path={d} />
          </circle>,
        );
    };
    const pulse = (x: number, y: number, r: number, color: string) => {
      if (!motion) return;
      edges.push(
        <circle key={key++} cx={x} cy={y} r={r} fill="none" style={{ stroke: color }} strokeWidth={1.5}>
          <animate attributeName="r" values={`${r};${r + 9}`} dur="1.8s" repeatCount="indefinite" />
          <animate attributeName="opacity" values="0.7;0" dur="1.8s" repeatCount="indefinite" />
        </circle>,
      );
    };
    const label = (x: number, y: number, text: string, color: string, border: string, op = 1) =>
      labels.push(
        <span
          key={key++}
          className="pointer-events-none absolute -translate-x-1/2 -translate-y-1/2 rounded px-1.5 font-mono text-[9.5px] leading-[1.3] font-medium whitespace-nowrap"
          style={{ left: x, top: y, background: "var(--inset)", border: `1px solid ${border}`, color, opacity: op }}
        >
          {text}
        </span>,
      );
    const box = (o: {
      x: number;
      y: number;
      w: number;
      h: number;
      glyph: string;
      title: string;
      meta: string;
      color: string;
      metaColor?: string;
      border?: string;
      dashed?: boolean;
      ring?: boolean;
      op?: number;
      badge?: { text: string; color: string };
      counts?: { t: string; color: string }[];
      hint?: string;
      tip?: string;
      onClick?: () => void;
      onHover?: (on: boolean) => void;
    }) =>
      nodes.push(
        <button
          key={key++}
          onClick={o.onClick}
          onMouseEnter={o.onHover && (() => o.onHover!(true))}
          onMouseLeave={o.onHover && (() => o.onHover!(false))}
          title={o.tip}
          className="absolute flex cursor-pointer items-center gap-2 rounded-[9px] px-2 text-left"
          style={{
            left: o.x,
            top: o.y,
            width: o.w,
            height: o.h,
            border: `1px ${o.dashed ? "dashed" : "solid"} ${o.border ?? "var(--line-3)"}`,
            background: "var(--menu)",
            boxShadow: o.ring ? `0 0 0 3px ${tint("var(--accent)", 18)}` : undefined,
            opacity: o.op ?? 1,
          }}
        >
          <span
            className="flex size-[22px] flex-none items-center justify-center rounded-md font-mono text-[12px] font-semibold"
            style={{ background: tint(o.color, 16), color: o.color }}
          >
            {o.glyph}
          </span>
          <span className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="truncate text-[12px] leading-[1.15] font-semibold text-text">{o.title}</span>
            <span className="truncate font-mono text-[10px] leading-[1.15]" style={{ color: o.metaColor ?? "var(--muted)" }}>
              {o.meta}
            </span>
          </span>
          {o.badge && (
            <span
              className="absolute -top-2 right-1.5 rounded px-1 py-0.5 font-mono text-[9px] leading-none font-semibold tracking-[0.06em]"
              style={{ background: "var(--inset)", border: `1px solid ${o.badge.color}`, color: o.badge.color }}
            >
              {o.badge.text}
            </span>
          )}
          {o.counts && (
            <span className="absolute top-[calc(100%+5px)] left-2 flex gap-2 font-mono text-[10px] leading-none font-medium whitespace-nowrap">
              {o.counts.map((c) => (
                <span key={c.t} style={{ color: c.color }}>
                  {c.t}
                </span>
              ))}
              <span className="text-muted">{o.hint}</span>
            </span>
          )}
        </button>,
      );
    const stage = (g: ColKey, mid: number, state: StageState, text: string, op: number, onClick: () => void, tip: string) => {
      const S = {
        done: [tint("var(--ok)", 40), tint("var(--ok)", 8), "var(--ok)", "solid"],
        active: ["var(--accent)", "var(--accent-tint)", "var(--accent-soft)", "solid"],
        waiting: [tint("var(--warn)", 55), tint("var(--warn)", 10), "var(--warn)", "solid"],
        fail: [tint("var(--bad)", 55), tint("var(--bad)", 10), "var(--bad)", "solid"],
        skip: ["var(--line-2)", "transparent", "var(--muted)", "solid"],
        future: ["var(--line-2)", "transparent", "var(--muted)", "dashed"],
      }[state];
      nodes.push(
        <button
          key={key++}
          onClick={onClick}
          title={tip}
          className="absolute flex h-[30px] cursor-pointer items-center justify-center gap-1 rounded-[15px] font-mono text-[10.5px] leading-none font-medium whitespace-nowrap"
          style={{
            left: COL[g][0],
            top: mid - 15,
            width: COL[g][1],
            border: `1px ${S[3]} ${S[0]}`,
            background: S[1],
            color: S[2],
            opacity: state === "future" ? op * 0.7 : op,
          }}
        >
          {text}
        </button>,
      );
    };

    const dim = (r: Row) => !!focus && r.agent?.id !== focus;
    const cy = (r: Row) => r.y + NH / 2;
    const bands = v.rows.map((r, i) =>
      i % 2 ? (
        <span
          key={`b${i}`}
          className="absolute inset-x-0"
          style={{
            top: r.y - 17,
            height: ROW + (r.lines ? 30 + r.lines * TRACE_LINE : 0),
            background: "color-mix(in srgb, var(--text) 1.2%, transparent)",
            borderTop: "1px solid color-mix(in srgb, var(--text) 3.5%, transparent)",
          }}
        />
      ) : null,
    );

    // Creators.
    for (const c of v.creators) {
      const y = v.cpos.get(c.id)!;
      const made = v.rows.filter((r) => (v.creatorOf.get(r.task.id) ?? "owner") === c.id).length;
      const meta =
        c.kind === "manager"
          ? [c.provider, c.model, c.session ?? c.state].filter(Boolean).join(" · ") || "manager"
          : c.kind === "planner"
            ? c.session ? `${c.provider ?? ""} · ${c.session}` : "approved plan"
            : "owner · dashboard";
      const color = c.kind === "manager" ? "var(--p-claude)" : c.kind === "planner" ? "var(--info)" : "var(--text)";
      box({
        x: COL.creator[0],
        y: y - 26,
        w: COL.creator[1],
        h: 52,
        glyph: c.kind === "manager" ? "🤵" : c.kind === "planner" ? "▤" : "◉",
        title: c.kind === "manager" ? "Manager" : c.kind === "planner" ? "Planner" : "You",
        meta,
        color,
        op: focus ? 0.35 : 1,
        tip:
          c.kind === "manager"
            ? `Always-on manager session; makes tasks and opens lanes (${made} in view)`
            : c.kind === "planner"
              ? "Tasks from an approved plan"
              : "Tasks you created",
        onClick: c.kind === "manager" ? () => setPop({ kind: "manager" }) : undefined,
      });
      if (c.kind === "manager" && ["running", "working"].includes(c.state ?? "")) pulse(COL.creator[0] + 20, y, 11, "var(--p-claude)");
    }

    // Lanes the manager (or you) opened, in a band above the rows.
    if (v.lanes.length) {
      v.lanes.forEach((l, i) => {
        const lx = 190 + i * 210;
        if (lx + 196 > W) return;
        const from = v.laneFrom.get(l.id) ?? "manager";
        const fy = v.cpos.get(from);
        const working = ["running", "working"].includes(l.state ?? "");
        box({
          x: lx,
          y: v.laneY,
          w: 196,
          h: 40,
          glyph: "≡",
          title: `lane · ${l.label}`,
          meta: [l.provider, l.state, ago(l.last_at, now)].filter(Boolean).join(" · "),
          color: "var(--p-claude)",
          border: tint("var(--p-claude)", 40),
          dashed: true,
          op: focus ? 0.3 : 1,
          badge: working ? { text: "working", color: "var(--ok)" } : undefined,
          tip: `Persistent lane opened by the ${from === "manager" ? "manager" : "owner"}`,
          onClick: () => setPop({ kind: "lane", lane: l }),
        });
        if (fy !== undefined) {
          const mx = COL.creator[0] + COL.creator[1] / 2;
          edge(`M${mx} ${fy - 26} C${mx} ${v.laneY + 20},${lx - 40} ${v.laneY + 20},${lx} ${v.laneY + 20}`, "creator", focus ? 0.2 : 0.9, {
            stroke: tint("var(--p-claude)", 55),
            dash: "4 3",
          });
        }
      });
      label(380, v.laneY - 12, `opened by the ${v.laneFrom.get(v.lanes[0].id) === "owner" ? "owner" : "manager"}`, "var(--p-claude)", tint("var(--p-claude)", 30), focus ? 0.3 : 1);
    }

    const rowById = new Map<number, Row>();
    for (const r of v.rows) if (r.task.task_id != null) rowById.set(r.task.task_id, r);

    for (const r of v.rows) {
      const { task, t, agent } = r;
      const op = dim(r) ? 0.22 : 1;
      const mid = cy(r);
      const id = task.task_id!;
      const status = t?.status ?? task.state ?? "pending";
      const sel = () => onSelect(`t:${id}`);
      const held = !!t && isHeld(t);
      const deps = t?.depends_on ?? [];

      const from = v.creatorOf.get(task.id) ?? "owner";
      const fy = v.cpos.get(from);
      if (fy !== undefined) edge(bez(COL.creator[0] + COL.creator[1], fy, COL.task[0], mid), "creator", op * (focus ? 0.3 : 0.8));

      const tc = TASK_COLOR[status] ?? "var(--idle)";
      box({
        x: COL.task[0],
        y: r.y,
        w: COL.task[1],
        h: NH,
        glyph: "◇",
        title: task.label,
        meta: [status, deps.length ? `after ${deps.map((d) => `#${d}`).join(", ")}` : "", ago(task.started_at, now)].filter(Boolean).join(" · "),
        color: tc,
        metaColor: status === "failed" ? "var(--bad)" : undefined,
        op,
        tip: "Open task card",
        onClick: sel,
      });

      // Pick · gates.
      const prov = agent?.provider ?? t?.provider ?? task.provider;
      const gates = (r.pick?.detail ?? "")
        .split(" · ")
        .filter((g) => g.startsWith("intake") || g.startsWith("tier"))
        .map((g) => g.replace(": ", " "));
      const waiting = deps.filter((d) => taskById.get(d)?.status !== "done");
      const pickMeta = held
        ? `held by Reflex · ${(t!.intake!.verdict ?? "").replace("_", " ")}`
        : status === "pending" && !agent
          ? waiting.length
            ? `waiting on ${waiting.map((d) => `#${d}`).join(", ")}`
            : "queued"
          : gates.join(" · ") || "picked";
      box({
        x: COL.pick[0],
        y: r.y,
        w: COL.pick[1],
        h: NH,
        glyph: "⑃",
        title: held ? "Held" : prov && (agent || r.pick) ? `→ ${prov}` : prov ? `pinned ${prov}` : "Not picked",
        meta: pickMeta,
        color: held ? "var(--warn)" : "var(--text)",
        border: held ? tint("var(--warn)", 45) : undefined,
        metaColor: held ? "var(--warn)" : undefined,
        op,
        tip: ["Gates that routed it: intake verdict, Reflex tier, provider chosen", r.pick?.detail].filter(Boolean).join("\n"),
        onClick: sel,
      });
      edge(line(COL.task[0] + COL.task[1], COL.pick[0], mid), agent || held || r.pick ? "done" : "future", op);

      if (!agent) {
        for (const g of STAGES) stage(g, mid, "future", g, op, sel, "not started");
        if (r.end) endNode(r, mid, op, COL.land[0] + COL.land[1]);
        continue;
      }

      // Agent.
      const pc = providerColor(agent.provider);
      const isF = focus === agent.id;
      const isH = hover === agent.id;
      const state = agent.state ?? "";
      const running = ["running", "verifying", "auditing", "landing"].includes(status) && !["exited", "idle"].includes(state);
      const count = (k: UseKind) => r.uses.filter((u) => u.kind === k).reduce((a, u) => a + (k === "command" ? u.count : 1), 0);
      const counts = USE_ORDER.filter((k) => k !== "subagent" || count(k) > 0).map((k) => ({ t: `${USE[k].glyph}${count(k)}`, color: USE[k].color }));
      box({
        x: COL.agent[0],
        y: r.y,
        w: COL.agent[1],
        h: NH,
        glyph: "○",
        title: [agent.provider, agent.model].filter(Boolean).join(" · ") || agent.label,
        meta: `${agent.session ?? agent.label}${status === "running" ? ` · ${elapsed(agent.started_at, now)}` : state ? ` · ${state.replace("_", " ")}` : ""}`,
        color: pc,
        border: isF ? "var(--accent)" : isH ? "var(--line-3)" : tint(pc, 50),
        ring: isF,
        op,
        badge: r.attempts > 1 ? { text: `attempt ${r.attempts}`, color: "var(--warn)" } : undefined,
        metaColor: status === "running" ? "var(--ok)" : undefined,
        counts,
        hint: r.uses.length ? (r.open ? "▴" : "▾") : "",
        tip: "Click: show what it used, focus its row, open its card",
        onHover: (on) => setHover(on ? agent.id : null),
        onClick: () => {
          setOpen((o) => ({ ...o, [agent.id]: !r.open }));
          setFocus(isF && r.open ? null : agent.id);
          if (agent.session) onSelect(`s:${agent.session}`);
        },
      });
      if (running) pulse(COL.agent[0] + 20, mid, 11, pc);
      edge(line(COL.pick[0] + COL.pick[1], COL.agent[0], mid), state === "starting" ? "active" : "done", op);
      label((COL.pick[0] + COL.pick[1] + COL.agent[0]) / 2, mid - 11, "spawn", "var(--muted)", "var(--line)", op * 0.9);

      // Stages.
      let prevX = COL.agent[0] + COL.agent[1];
      if (status === "running") edge(line(prevX, COL.verify[0], mid), "active", op);
      STAGES.forEach((g, i) => {
        const s = r.stages[g];
        stage(g, mid, s.state, s.label, op, sel, [`${g}: ${s.state}`, s.node?.detail, ...(s.node?.samples ?? [])].filter(Boolean).join("\n"));
        if (!(i === 0 && status === "running"))
          edge(
            line(prevX, COL[g][0], mid),
            s.state === "active" ? "active" : s.state === "fail" ? "fail" : s.state === "future" ? "future" : "done",
            op,
          );
        if (s.state === "active") pulse(COL[g][0] + COL[g][1] / 2, mid, 15, "var(--accent)");
        prevX = COL[g][0] + COL[g][1];
      });
      if (r.end) endNode(r, mid, op, prevX);

      // Retry loop: verify back to the pick.
      if (r.attempts > 1) {
        const x1 = COL.verify[0] + COL.verify[1] / 2;
        const x2 = COL.pick[0] + COL.pick[1] / 2;
        const top = r.y - 14;
        edge(`M${x1} ${mid - 15} C${x1} ${top},${x2} ${top},${x2} ${r.y - 1}`, "fail", op * 0.85, { dash: "4 3" });
        label((x1 + x2) / 2, top + 4, `↺ retry · attempt ${r.attempts}`, "var(--bad)", tint("var(--bad)", 35), op);
      }

      // What it used, as pills.
      if (r.open) {
        const ty = r.y + NH + 30;
        const ax = COL.agent[0] + 30;
        const ay = r.y + NH + 16;
        let x = COL.agent[0] - 40;
        let ln = 0;
        for (const u of r.uses) {
          const w = pillWidth(u);
          if (x + w > W - 10) {
            x = COL.agent[0] - 40;
            ln++;
          }
          const py = ty + ln * TRACE_LINE;
          const T = USE[u.kind as UseKind];
          edge(`M${ax} ${ay} C${ax} ${py - 6},${x + 14} ${py - 18},${x + 14} ${py - 1}`, "creator", op * 0.8, {
            stroke: tint(T.ring, 55),
            marker: false,
            sw: 1,
          });
          extras.push(
            <button
              key={key++}
              onClick={() => setPop({ kind: "use", use: u, agent })}
              title={`${T.name} · last ${ago(u.last_at, now)}`}
              className="absolute flex h-6 cursor-pointer items-center gap-1.5 rounded-xl px-2 font-mono text-[10.5px] leading-none font-medium whitespace-nowrap text-text"
              style={{ left: x, top: py, maxWidth: w + 24, border: `1px solid ${tint(T.ring, 45)}`, background: "var(--menu)", opacity: op }}
            >
              <span style={{ color: T.color }}>{T.glyph}</span>
              <span className="min-w-0 truncate">{u.label}</span>
              {pillCount(u) && <span className="text-muted">{pillCount(u)}</span>}
            </button>,
          );
          x += w + 10;
        }
      }
    }

    function endNode(r: Row, mid: number, op: number, fromX: number) {
      const e = r.end!;
      const ok = e.label === "done";
      const cancelled = e.label === "cancelled";
      const c = ok ? "var(--ok)" : cancelled ? "var(--idle)" : "var(--bad)";
      const result = r.t?.result?.split("\n")[0] ?? "";
      box({
        x: COL.end[0],
        y: r.y,
        w: COL.end[1],
        h: NH,
        glyph: ok ? "✓" : "✕",
        title: e.label,
        meta: ok ? (/committed\s+(\S+)/.exec(result)?.[1] ?? r.t?.git?.branch ?? result) || "done" : cancelled ? "cancelled" : "needs you",
        color: c,
        border: tint(c, 45),
        op,
        tip: [e.detail, result].filter(Boolean).join("\n"),
        onClick: () => onSelect(`t:${r.task.task_id}`),
      });
      edge(line(fromX, COL.end[0], mid), ok || cancelled ? "done" : "fail", op);
    }

    // Unblocks: a dashed edge in the left gutter to each task a task unblocked.
    for (const r of v.rows)
      for (const d of r.t?.depends_on ?? []) {
        const a = rowById.get(d);
        if (!a) continue;
        const x = COL.task[0] + 18;
        const y1 = a.y + NH;
        const y2 = r.y;
        edge(`M${x} ${y1} C${x - 26} ${y1 + 18},${x - 26} ${y2 - 18},${x} ${y2 - 1}`, "dep", focus ? 0.25 : 0.9);
      }

    const heads: [ColKey, string][] = [
      ["creator", "created by"],
      ["task", "task"],
      ["pick", "pick · gates"],
      ["agent", "agent"],
      ["verify", "verify"],
      ["audit", "audit"],
      ["land", "land"],
      ["end", "end"],
    ];
    const H = v.height;
    return (
      <>
        <div className="relative min-h-0 flex-1 overflow-auto">
          <div className="relative" style={{ width: Math.round(W * scale), height: Math.round(H * scale) }}>
            <div className="absolute top-0 left-0 origin-top-left" style={{ width: W, height: H, transform: `scale(${scale})` }}>
              {heads.map(([k, l]) => (
                <span
                  key={k}
                  className="absolute top-2.5 text-center font-mono text-[9.5px] leading-none font-medium tracking-[0.16em] text-muted/80 uppercase"
                  style={{ left: COL[k][0], width: COL[k][1] }}
                >
                  {l}
                </span>
              ))}
              {bands}
              <svg width={W} height={H} className="pointer-events-none absolute top-0 left-0 overflow-visible" aria-hidden>
                <defs>
                  {(
                    [
                      ["ag-arr", "var(--line-3)"],
                      ["ag-arr-a", "var(--accent)"],
                      ["ag-arr-r", "var(--bad)"],
                    ] as const
                  ).map(([id, c]) => (
                    <marker key={id} id={id} viewBox="0 0 8 8" refX={7} refY={4} markerWidth={6} markerHeight={6} orient="auto">
                      <path d="M0 0L8 4L0 8z" style={{ fill: c }} />
                    </marker>
                  ))}
                </defs>
                {edges}
              </svg>
              {labels}
              {nodes}
              {extras}
            </div>
          </div>
        </div>
        <div className="flex flex-none flex-wrap gap-x-3.5 gap-y-1 border-t border-line px-3.5 py-2 font-mono text-[10.5px] leading-[1.4] text-muted">
          {(
            [
              ["◇", "var(--idle)", "task"],
              ["○", "var(--p-claude)", "agent session"],
              ["⌘", "var(--text)", "command"],
              ["✦", "var(--p-gemini)", "skill"],
              ["◆", "var(--accent)", "knowledge card"],
              ["⇢", "var(--info)", "sub-agent"],
              ["- -", "var(--p-gemini)", "unblocks"],
              ["•", "var(--accent)", "hand-off in progress"],
            ] as const
          ).map(([g, c, t]) => (
            <span key={t} className="flex items-center gap-1.5">
              <span style={{ color: c }}>{g}</span>
              {t}
            </span>
          ))}
        </div>
      </>
    );
  }

  return (
    <div ref={wrap} className="relative flex h-full min-h-0 flex-col">
      {toolbar}
      {body}
      {pop && view && popover(pop, () => setPop(null))}
    </div>
  );

  function popover(pop: Pop, onClose: () => void) {
    const v = view!;
    let glyph = "";
    let color = "";
    let kicker = "";
    let title = "";
    let lines: { t: string; color?: string }[] = [];
    let link: { text: string; go: () => void } | null = null;
    let foot = "";
    if (pop.kind === "use") {
      const u = pop.use;
      const who = pop.agent.session ?? pop.agent.label;
      const T = USE[u.kind as UseKind];
      glyph = T.glyph;
      color = T.color;
      title = u.label;
      if (u.kind === "command") {
        kicker = `command · ${u.count} invocation${u.count === 1 ? "" : "s"} · ${who}`;
        const samples = u.samples.length ? u.samples : [u.detail ?? u.label];
        lines = samples
          .map((s, i) => ({ t: `$ ${s}${u.outputs[i] ? `\n${u.outputs[i]}` : ""}` }))
          .reverse();
        foot = `latest ${samples.length} of ${u.count}, read from the CLI transcript (read only); secrets masked`;
      } else if (u.kind === "knowledge") {
        kicker = `knowledge card · read by ${who}`;
        lines = [u.detail ?? u.label, ...u.samples.filter((s) => s !== u.detail)].map((t) => ({ t }));
        link = {
          text: "Open in Knowledge →",
          go: () => go("knowledge", u.label.startsWith(".docs/") ? `${slug}/doc/${u.label}` : slug),
        };
      } else if (u.kind === "skill") {
        kicker = `skill · loaded by ${who}`;
        lines = [u.detail || u.label].map((t) => ({ t }));
        link = { text: "Open in Skills →", go: () => go("skills") };
      } else {
        kicker = `sub-agent · started by ${who}`;
        lines = (u.samples.length ? u.samples : [u.detail ?? u.label]).map((t) => ({ t }));
        foot = "sub-agents run inside the parent CLI session";
      }
      if (u.kind !== "command") foot = foot || `×${u.count} · last ${ago(u.last_at, now)}`;
    } else if (pop.kind === "manager") {
      const m = v.creators.find((c) => c.kind === "manager");
      glyph = "🤵";
      color = "var(--p-claude)";
      kicker = `manager · ${slug}`;
      title = m?.session ?? "manager";
      const made = v.rows.filter((r) => v.creatorOf.get(r.task.id) === "manager").length;
      const opened = v.lanes.filter((l) => v.laneFrom.get(l.id) === "manager").length;
      lines = [
        { t: `created ${made} task${made === 1 ? "" : "s"} in view · opened ${opened} lane${opened === 1 ? "" : "s"}` },
        { t: [m?.provider, m?.model, m?.state, "token $MAP_MANAGER_TOKEN (project scope)"].filter(Boolean).join(" · "), color: "var(--muted)" },
      ];
      if (m?.session) link = { text: "Open its console →", go: () => onOpenConsole(m.session!) };
      foot = `tasks record created_by = manager; lanes record spawned_by = manager:${slug}`;
    } else {
      const l = pop.lane;
      glyph = "≡";
      color = "var(--p-claude)";
      kicker = `persistent lane · spawned_by ${v.laneFrom.get(l.id) === "manager" ? `manager:${slug}` : "owner"}`;
      title = l.session ?? l.label;
      lines = [{ t: [l.provider, l.model, l.state, `last active ${ago(l.last_at, now)}`].filter(Boolean).join(" · ") }];
      if (l.session) link = { text: "Open its console →", go: () => onOpenConsole(l.session!) };
      foot = "started and closed through the hub like any MAP session";
    }
    return (
      <div className="absolute bottom-11 left-3.5 z-10 flex max-h-[60%] w-[min(420px,calc(100%-28px))] flex-col gap-2 overflow-auto rounded-[10px] border border-line-3 bg-menu p-3 shadow-[0_16px_44px_rgba(0,0,0,.55)]">
        <div className="flex items-center gap-2">
          <span className="font-mono text-[12px] leading-none font-semibold" style={{ color }}>
            {glyph}
          </span>
          <span className="font-mono text-[10px] leading-none font-medium tracking-[0.12em] text-muted uppercase">{kicker}</span>
          <button onClick={onClose} className="ml-auto cursor-pointer text-[13px] leading-none text-muted" aria-label="Close">
            ×
          </button>
        </div>
        <span className="font-mono text-[14px] leading-[1.25] font-semibold [overflow-wrap:anywhere]">{title}</span>
        {lines.map((l, i) => (
          <code
            key={i}
            className="rounded-md border border-line bg-inset px-2 py-1.5 font-mono text-[11px] leading-[1.45] whitespace-pre-wrap [overflow-wrap:anywhere]"
            style={{ color: l.color ?? "var(--nav)" }}
          >
            {l.t}
          </code>
        ))}
        {link && (
          <button onClick={link.go} className="cursor-pointer self-start text-[12px] font-medium text-link hover:text-link-hover">
            {link.text}
          </button>
        )}
        {foot && <span className="font-mono text-[10.5px] leading-[1.4] text-muted">{foot}</span>}
      </div>
    );
  }
}

const pillCount = (u: ActivityNode) => (u.kind === "command" || u.kind === "subagent" ? `×${u.count}` : "");
/** A pill's width estimate, for wrapping (10.5px mono is about 6.4px a character; a little more for fallbacks). */
const pillWidth = (u: ActivityNode) => Math.min(340, 34 + (Math.min(u.label.length, 44) + pillCount(u).length) * 7);
