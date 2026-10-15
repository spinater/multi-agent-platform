"use client";

/** The Activity view of the command centre (`#command/activity`, backend trace.rs): the project's task flow, live.
 * Each task is a row, left to right: who created it (manager, planner, owner) → the task → the orchestrator's pick
 * (with the gates that routed it) → the agent session → verify → audit → land → done or failed, retries going round
 * again, and dashed edges to the tasks it unblocked. The commands, skills, knowledge and sub-agents each agent used
 * hang off it (collapsed to counts). It reloads when a WebSocket event touches the project; nodes the latest events
 * touched pulse, and a dot runs along a hand-off in progress. */

import clsx from "clsx";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  TerminalSquare,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { activityApi } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { ActivityEdge } from "@/lib/generated/ActivityEdge";
import type { ActivityGraph } from "@/lib/generated/ActivityGraph";
import type { ActivityNode } from "@/lib/generated/ActivityNode";

import { useApp } from "./app-context";

const COL = 196;
const NODE_W = 164;
const NODE_H = 54;
const ROW = 78;
const USAGE_ROW = 17;
const PAD = 16;

type Range = "1h" | "24h" | "all";
const RANGES: [Range, string][] = [
  ["1h", "1 h"],
  ["24h", "24 h"],
  ["all", "all"],
];

const USAGE = ["command", "skill", "knowledge", "subagent", "tool"] as const;
const USAGE_SHORT: Record<string, string> = {
  command: "cmd",
  skill: "skill",
  knowledge: "kb",
  subagent: "sub",
  tool: "tool",
};
const KIND_COLOR: Record<string, string> = {
  manager: "var(--accent)",
  planner: "var(--info)",
  owner: "var(--text-2)",
  orchestrator: "var(--accent-soft)",
  task: "var(--text)",
  step: "var(--info)",
};
const STATE_COLOR: Record<string, string> = {
  ok: "var(--ok)",
  done: "var(--ok)",
  fail: "var(--bad)",
  failed: "var(--bad)",
  cancelled: "var(--idle)",
  running: "var(--accent)",
  working: "var(--accent)",
  starting: "var(--info)",
  waiting: "var(--warn)",
  rate_limited: "var(--warn)",
  needs_auth: "var(--bad)",
  idle: "var(--idle)",
  exited: "var(--idle)",
  pending: "var(--idle)",
  stopped: "var(--bad)",
  restarting: "var(--warn)",
};

function since(range: Range, now: number): string | undefined {
  if (range === "all") return undefined;
  return new Date(now - (range === "1h" ? 3600_000 : 86_400_000)).toISOString();
}

function elapsed(from: string | null, now: number): string {
  if (!from) return "";
  const s = Math.max(0, Math.round((now - new Date(from).getTime()) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
}

function ago(at: string | null, now: number): string {
  return at ? `${elapsed(at, now)} ago` : "";
}

const RUNNING = new Set(["running", "working", "starting"]);

export function ActivityGraphView({
  slug,
  motion,
  onOpenConsole,
}: {
  slug: string;
  motion: boolean;
  onOpenConsole: (session: string) => void;
}) {
  const { events, now, server, go } = useApp();
  const [range, setRange] = useState<Range>("24h");
  const [live, setLive] = useState(true);
  const [showDone, setShowDone] = useState(false);
  const [graph, setGraph] = useState<ActivityGraph | null>(null);
  const [error, setError] = useState("");
  const [focus, setFocus] = useState<string | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [samples, setSamples] = useState<ActivityNode | null>(null);
  const prefix = server?.session_prefix ?? "map-";

  // Reload on the range, and (live) whenever a WebSocket event touches this project: no polling of its own.
  const latest = events[0]?.id ?? 0;
  const loadedFor = useRef<string>("");
  const timer = useRef<number | null>(null);
  useEffect(() => {
    const key = `${slug}|${range}`;
    const first = loadedFor.current !== key;
    if (!first && !live) return;
    loadedFor.current = key;
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(
      () => {
        activityApi
          .project(slug, {
            since: since(range, Date.now()),
            all: range === "all",
          })
          .then(
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
  }, [slug, range, live, latest]);

  // Nodes touched by the last few seconds of events pulse.
  const hot = useMemo(() => {
    const out = new Set<string>();
    for (const e of events.slice(0, 40)) {
      if (now - new Date(e.at).getTime() > 6000) break;
      if (e.task_id != null) out.add(`task:${e.task_id}`);
      if (e.session) {
        out.add(`agent:${e.session}`);
        if (e.session === `${prefix}mgr-${slug}`) out.add("manager");
      }
    }
    return out;
  }, [events, now, prefix, slug]);

  const view = useMemo(() => {
    if (!graph) return null;
    // Done tasks fade out two minutes after they finish, unless "show done".
    const hiddenRows = new Set<number>();
    const fading = new Set<number>();
    for (const n of graph.nodes) {
      if (n.kind !== "task" || n.state !== "done") continue;
      const age = n.last_at ? now - new Date(n.last_at).getTime() : Infinity;
      if (showDone) continue;
      if (age < 120_000) fading.add(n.row);
      else hiddenRows.add(n.row);
    }
    const flow = graph.nodes.filter(
      (n) => !n.parent && !(n.layer > 0 && hiddenRows.has(n.row)),
    );
    const usage = new Map<string, ActivityNode[]>();
    for (const n of graph.nodes) {
      if (!n.parent) continue;
      usage.set(n.parent, [...(usage.get(n.parent) ?? []), n]);
    }
    for (const list of usage.values())
      list.sort(
        (a, b) =>
          USAGE.indexOf(a.kind as (typeof USAGE)[number]) -
            USAGE.indexOf(b.kind as (typeof USAGE)[number]) ||
          b.count - a.count,
      );
    // Rows: compacted (hidden ones removed), each as tall as its tallest opened agent list.
    const rows = [
      ...new Set(
        flow.filter((n) => n.layer > 0 || n.kind === "agent").map((n) => n.row),
      ),
    ].sort((a, b) => a - b);
    const rowTop = new Map<number, number>();
    let y = PAD;
    for (const r of rows) {
      rowTop.set(r, y);
      const extra = Math.max(
        0,
        ...flow
          .filter((n) => n.row === r && open[n.id])
          .map((n) => (usage.get(n.id)?.length ?? 0) * USAGE_ROW + 8),
      );
      y += ROW + extra;
    }
    const height = Math.max(y, PAD + ROW);
    const pos = new Map<string, { x: number; y: number }>();
    for (const n of flow) {
      if (n.layer === 0) continue;
      pos.set(n.id, { x: PAD + n.layer * COL, y: rowTop.get(n.row) ?? PAD });
    }
    // Creators sit level with the tasks they made.
    const creators = flow.filter((n) => n.layer === 0);
    creators.forEach((c, i) => {
      const ys = graph.edges
        .filter((e) => e.from === c.id && pos.has(e.to))
        .map((e) => pos.get(e.to)!.y);
      const yy = ys.length
        ? ys.reduce((a, b) => a + b, 0) / ys.length
        : PAD + i * ROW;
      pos.set(c.id, { x: PAD, y: yy });
    });
    // Two creators at the same height are pushed apart.
    const placed = creators
      .map((c) => pos.get(c.id)!)
      .sort((a, b) => a.y - b.y);
    for (let i = 1; i < placed.length; i++)
      if (placed[i].y - placed[i - 1].y < NODE_H + 10)
        placed[i].y = placed[i - 1].y + NODE_H + 10;
    const edges = graph.edges.filter((e) => pos.has(e.from) && pos.has(e.to));
    const width =
      PAD * 2 + (Math.max(0, ...flow.map((n) => n.layer)) + 1) * COL;
    return {
      flow,
      usage,
      pos,
      edges,
      width,
      height: Math.max(height, ...placed.map((p) => p.y + NODE_H + PAD)),
      fading,
    };
  }, [graph, now, showDone, open]);

  const related = useMemo(() => {
    if (!focus || !view) return null;
    const ids = new Set([focus]);
    for (const e of view.edges) {
      if (e.from === focus) ids.add(e.to);
      if (e.to === focus) ids.add(e.from);
    }
    return ids;
  }, [focus, view]);

  const clickNode = (n: ActivityNode) => {
    if (n.kind === "task" && n.task_id != null) {
      go("command", `tasks/${n.task_id}`);
      return;
    }
    setFocus(focus === n.id ? null : n.id);
  };

  const clickUsage = (u: ActivityNode) => {
    if (u.kind === "knowledge" && u.label.startsWith(".docs/"))
      go("knowledge", `${slug}/doc/${u.label}`);
    else setSamples(u);
  };

  const toolbar = (
    <div className="flex flex-wrap items-center gap-1.5 px-3 pt-3">
      <div className="flex overflow-hidden rounded-md border border-line-2">
        {RANGES.map(([r, label]) => (
          <button
            key={r}
            onClick={() => setRange(r)}
            className={clsx(
              "h-[26px] cursor-pointer border-r border-line px-2.5 font-mono text-[10.5px] font-semibold last:border-r-0",
              range === r
                ? "bg-accent text-accent-text"
                : "text-nav hover:text-text",
            )}
          >
            {label}
          </button>
        ))}
      </div>
      {(
        [
          ["live", live, setLive],
          ["show done", showDone, setShowDone],
        ] as const
      ).map(([label, on, set]) => (
        <button
          key={label}
          onClick={() => set(!on)}
          aria-pressed={on}
          className={clsx(
            "flex h-[26px] cursor-pointer items-center gap-1.5 rounded-md border px-2.5 font-mono text-[10.5px] font-medium",
            on ? "border-accent text-accent-soft" : "border-line-2 text-nav",
          )}
        >
          {label === "live" && (
            <span
              className={clsx(
                "size-1.5 rounded-full",
                on ? "animate-pulse bg-ok" : "bg-idle",
              )}
            />
          )}
          {label}
        </button>
      ))}
      {focus && (
        <button
          onClick={() => setFocus(null)}
          className="flex h-[26px] cursor-pointer items-center gap-1 rounded-md border border-line-2 px-2 font-mono text-[10.5px] text-nav"
        >
          <X className="size-3" /> clear filter
        </button>
      )}
      {error && <span className="font-mono text-[11px] text-bad">{error}</span>}
    </div>
  );

  if (!view)
    return (
      <div className="flex h-full flex-col">
        {toolbar}
        <p className="p-6 font-mono text-[12px] text-muted">
          {error ? "" : "loading the activity…"}
        </p>
      </div>
    );
  if (view.flow.length === 0)
    return (
      <div className="flex h-full flex-col">
        {toolbar}
        <p className="p-6 font-mono text-[12px] text-muted">
          No tasks or agents in this range. Create a task, or ask the manager
          to.
        </p>
      </div>
    );

  const path = (e: ActivityEdge) => {
    const a = view.pos.get(e.from)!;
    const b = view.pos.get(e.to)!;
    const [x1, y1] = [a.x + NODE_W, a.y + NODE_H / 2];
    const [x2, y2] = [b.x, b.y + NODE_H / 2];
    if (x2 <= x1) {
      // Backwards (a retry, an unblocked task to the left): a loop below.
      const dy = Math.max(y1, y2) + NODE_H;
      return `M ${x1} ${y1} C ${x1 + 60} ${dy}, ${x2 - 60} ${dy}, ${x2} ${y2}`;
    }
    const mid = (x1 + x2) / 2;
    return `M ${x1} ${y1} C ${mid} ${y1}, ${mid} ${y2}, ${x2} ${y2}`;
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      {toolbar}
      <div className="relative min-h-0 flex-1 overflow-auto">
        <div
          className="relative"
          style={{ width: view.width, height: view.height + PAD }}
        >
          <svg
            className="pointer-events-none absolute inset-0"
            width={view.width}
            height={view.height + PAD}
            aria-hidden
          >
            {view.edges.map((e, i) => {
              const d = path(e);
              const dim =
                related && !(related.has(e.from) && related.has(e.to));
              const color =
                e.kind === "retry"
                  ? "var(--warn)"
                  : e.kind === "unblocks"
                    ? "var(--ok)"
                    : e.kind === "created" || e.kind === "spawned"
                      ? "var(--accent)"
                      : "var(--line-2)";
              return (
                <g key={i} opacity={dim ? 0.12 : 1}>
                  <path
                    d={d}
                    fill="none"
                    stroke={color}
                    strokeWidth={1.2 + Math.min(4, Math.log2(e.count + 1))}
                    strokeDasharray={e.kind === "unblocks" ? "5 4" : undefined}
                  />
                  {e.live && motion && (
                    <circle r={4} fill="var(--accent)">
                      <animateMotion
                        dur="1.6s"
                        repeatCount="indefinite"
                        path={d}
                      />
                    </circle>
                  )}
                </g>
              );
            })}
          </svg>
          {view.flow.map((n) => {
            const p = view.pos.get(n.id);
            if (!p) return null;
            const items = view.usage.get(n.id) ?? [];
            const counts = USAGE.map(
              (k) =>
                [
                  k,
                  items
                    .filter((u) => u.kind === k)
                    .reduce((a, u) => a + u.count, 0),
                ] as const,
            ).filter(([, c]) => c > 0);
            const color =
              n.kind === "agent"
                ? providerColor(n.provider ?? "")
                : (KIND_COLOR[n.kind] ?? "var(--text-2)");
            const state = n.state ?? "";
            const running = RUNNING.has(state);
            const dim = related && !related.has(n.id);
            const into = view.edges.filter((e) => e.to === n.id);
            const tip = [
              n.label,
              n.detail,
              n.provider && `${n.provider}${n.model ? ` · ${n.model}` : ""}`,
              state && `state: ${state}`,
              ...into.map(
                (e) =>
                  `${e.kind}${e.label ? ` (${e.label})` : ""}${e.count > 1 ? ` ×${e.count}` : ""} · ${ago(e.last_at, now)}`,
              ),
            ]
              .filter(Boolean)
              .join("\n");
            return (
              <div
                key={n.id}
                className={clsx(
                  "absolute",
                  view.fading.has(n.row) &&
                    n.layer > 0 &&
                    "opacity-40 transition-opacity duration-[2000ms]",
                )}
                style={{
                  left: p.x,
                  top: p.y,
                  width: NODE_W,
                  opacity: dim ? 0.25 : undefined,
                }}
              >
                <button
                  onClick={() => clickNode(n)}
                  title={tip}
                  className={clsx(
                    "flex w-full cursor-pointer flex-col items-start gap-0.5 overflow-hidden rounded-lg border bg-surface px-2 py-1.5 text-left",
                    focus === n.id ? "ring-2 ring-accent" : "",
                    hot.has(n.id) && "animate-pulse ring-2 ring-accent/70",
                  )}
                  style={{
                    height: NODE_H,
                    borderColor: color,
                    borderLeftWidth: 3,
                  }}
                >
                  <span className="flex w-full items-center gap-1 font-mono text-[9px] tracking-[0.12em] text-muted uppercase">
                    {n.kind === "manager" && (
                      <Bot className="size-2.5 text-accent" />
                    )}
                    {n.kind === "step" ? "step" : n.kind}
                    {state && (
                      <span
                        className="ml-auto size-[7px] flex-none rounded-full"
                        style={{
                          background: STATE_COLOR[state] ?? "var(--idle)",
                        }}
                        title={state}
                      />
                    )}
                  </span>
                  <span className="w-full truncate font-mono text-[11px] font-semibold text-text">
                    {n.label}
                  </span>
                  <span className="w-full truncate font-mono text-[9.5px] text-muted">
                    {running
                      ? `${state} · ${elapsed(n.started_at, now)}`
                      : n.kind === "agent"
                        ? `${n.provider ?? ""}${n.model ? ` · ${n.model}` : ""}`
                        : (n.detail ?? state)}
                  </span>
                </button>
                {counts.length > 0 && (
                  <button
                    onClick={() => setOpen({ ...open, [n.id]: !open[n.id] })}
                    className="mt-0.5 flex w-full cursor-pointer items-center gap-1 font-mono text-[9.5px] text-nav hover:text-text"
                    aria-expanded={!!open[n.id]}
                  >
                    {open[n.id] ? (
                      <ChevronDown className="size-2.5" />
                    ) : (
                      <ChevronRight className="size-2.5" />
                    )}
                    {counts
                      .map(([k, c]) => `${USAGE_SHORT[k]} ${c}`)
                      .join(" · ")}
                  </button>
                )}
                {focus === n.id && n.session && (
                  <button
                    onClick={() => onOpenConsole(n.session!)}
                    className="mt-0.5 flex cursor-pointer items-center gap-1 font-mono text-[9.5px] text-accent-soft hover:underline"
                  >
                    <TerminalSquare className="size-2.5" /> console
                  </button>
                )}
                {open[n.id] && (
                  <ul className="mt-0.5 flex flex-col">
                    {items.map((u) => (
                      <li key={u.id}>
                        <button
                          onClick={() => clickUsage(u)}
                          title={`${u.kind} ×${u.count} · last ${ago(u.last_at, now)}${u.detail ? `\n${u.detail}` : ""}`}
                          className="flex w-full cursor-pointer items-center gap-1 truncate text-left font-mono text-[10px] leading-[17px] text-text-2 hover:text-text"
                        >
                          <span className="w-8 flex-none text-muted">
                            {USAGE_SHORT[u.kind] ?? u.kind}
                          </span>
                          <span className="truncate">{u.label}</span>
                          <span className="ml-auto flex-none text-muted">
                            ×{u.count}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            );
          })}
        </div>
        {samples && (
          <div className="absolute top-3 right-3 z-10 w-[min(420px,calc(100%-24px))] rounded-lg border border-line-2 bg-surface p-3 shadow-lg">
            <div className="mb-2 flex items-center justify-between gap-2">
              <span className="truncate font-mono text-[11px] font-semibold">
                {samples.kind} · {samples.label} ×{samples.count}
              </span>
              <button
                onClick={() => setSamples(null)}
                className="cursor-pointer text-nav"
                aria-label="Close"
              >
                <X className="size-3.5" />
              </button>
            </div>
            <ul className="flex max-h-[240px] flex-col gap-1 overflow-auto">
              {(samples.samples.length
                ? samples.samples
                : [samples.detail ?? samples.label]
              ).map((s, i) => (
                <li
                  key={i}
                  className="font-mono text-[10.5px] break-all whitespace-pre-wrap text-text-2"
                >
                  {s}
                </li>
              ))}
            </ul>
            <p className="mt-2 font-mono text-[9.5px] text-muted">
              latest {samples.samples.length} invocations · secrets masked ·
              last {ago(samples.last_at, now)}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
