"use client";

import clsx from "clsx";
import type { CSSProperties } from "react";

import { PIPELINE_STEPS, ageOf, stepIndex, trackingSince, useActivity, type Activity } from "@/lib/activity";
import { providerColor } from "@/lib/colors";
import type { Event } from "@/lib/generated/Event";
import type { TaskStatus } from "@/lib/generated/TaskStatus";

import type { Command } from "./command-data";
import { rel } from "./command-ui";

/** The live parts of the command centre: the activity feed, the canvas ticker and the pipeline bar. */

const STEP_COLOR: Partial<Record<TaskStatus, string>> = { verifying: "var(--info)", auditing: "var(--info)", landing: "var(--info)", review: "var(--warn)", done: "var(--ok)" };

/** run · verify · audit · land · done, the current step sweeping; `review` waits at land. `labels` names the steps
 * under the bar. */
export function PhaseBar({ status, color, labels }: { status: TaskStatus; color: string; labels?: boolean }) {
  const at = stepIndex(status);
  if (at < 0) return null;
  const done = status === "done";
  const current = STEP_COLOR[status] ?? color;
  return (
    <span className="flex gap-[3px]" title={PIPELINE_STEPS.map((s) => s.label).join(" → ")} aria-label={`pipeline: ${status}`}>
      {PIPELINE_STEPS.map((s, i) => (
        <span key={s.key} className="flex flex-1 flex-col gap-1">
          <span
            className={clsx("map-step h-[3px] rounded-full", i === at && !done && "map-step-live")}
            style={{ backgroundColor: done || i < at ? "color-mix(in srgb, var(--ok) 70%, transparent)" : i === at ? current : "var(--line-2)" }}
          />
          {labels && (
            <span className={clsx("font-mono text-[9.5px] leading-none tracking-[0.08em] uppercase", i === at ? "text-text" : "text-muted")}>{i === 3 && status === "review" ? "review" : s.label}</span>
          )}
        </span>
      ))}
    </span>
  );
}

const SEED_KIND: Record<string, Activity["kind"]> = { "task.started": "spawn", "task.done": "done", "task.failed": "failed", "task.cancelled": "cancelled", "task.review": "phase" };

/** Items from the event log for what happened before the page loaded, so a reload does not start empty. */
function seeds(events: Event[], cmd: Command): Activity[] {
  const out: Activity[] = [];
  for (const e of events) {
    const kind = SEED_KIND[e.kind];
    if (!kind || new Date(e.at).getTime() >= trackingSince) continue;
    const t = e.task_id != null ? cmd.tasks.find((x) => x.id === e.task_id) : undefined;
    if (!t) continue;
    const on = /on (\S+) \(([^)]+)\)/.exec(e.message);
    out.push({
      id: -e.id,
      at: new Date(e.at).getTime(),
      kind,
      taskId: t.id,
      session: e.session,
      project: t.project,
      provider: on?.[1] ?? t.provider,
      model: on?.[2] ?? t.model,
      title: t.title,
      from: null,
      to: e.kind === "task.review" ? "review" : null,
      attempt: Number(/attempt (\d+)/.exec(e.message)?.[1] ?? 1),
    });
    if (out.length >= 12) break;
  }
  return out;
}

const LANE_EVENT: Record<string, string> = {
  "lane.card_done": "card done",
  "lane.ask": "prompted",
  "lane.tell": "tell queued",
  "lane.clear": "context cleared",
  "lane.resume": "resumed",
  "lane.stall": "stalled",
  "lane.rate_limited": "rate limited",
};

/** What the lanes in scope did, from the event log (live over the WebSocket): cards done, prompts, tells, clears. */
function laneEvents(cmd: Command): Activity[] {
  const names = new Set(cmd.lanes.map((l) => l.name));
  const out: Activity[] = [];
  for (const e of cmd.events) {
    const what = LANE_EVENT[e.kind];
    if (!what || !e.session || !names.has(e.session)) continue;
    // The message starts with an icon and `<lane>: `; the rest is the news.
    const text = e.message.replace(/^\P{L}*/u, "").replace(new RegExp(`^${e.session}(\\s›\\s\\S+)?:\\s*`), "");
    out.push({ id: -e.id, at: new Date(e.at).getTime(), kind: "lane", taskId: null, session: e.session, project: null, provider: "claude", model: null, title: `${what} · ${text}`, from: null, to: null, attempt: 1 });
    if (out.length >= 12) break;
  }
  return out;
}

function glyph(a: Activity): [string, string] {
  switch (a.kind) {
    case "spawn":
      return ["↗", providerColor(a.provider)];
    case "session":
      return ["○", providerColor(a.provider)];
    case "lane":
      return ["▸", "var(--p-claude)"];
    case "done":
      return ["✓", "var(--ok)"];
    case "failed":
      return ["✕", "var(--bad)"];
    case "cancelled":
      return ["–", "var(--idle)"];
    default:
      return ["◆", (a.to && STEP_COLOR[a.to]) ?? "var(--accent)"];
  }
}

function headline(a: Activity, label: (id: number) => string): string {
  const who = a.taskId != null ? label(a.taskId) : (a.session ?? "");
  const on = a.provider ? `${a.provider}${a.model ? ` (${a.model})` : ""}` : "";
  switch (a.kind) {
    case "spawn":
      return `${who} → ${on} spawned${a.attempt > 1 ? ` · try ${a.attempt}` : ""}`;
    case "session":
      return `${a.session} started · ${on}`;
    case "lane":
      return a.session ?? "lane";
    case "phase":
      return a.from ? `${who} ${a.from} → ${a.to}` : `${who} ${a.to}`;
    default:
      return `${who} ${a.kind}`;
  }
}

const labeller = (cmd: Command) => (id: number) => {
  const t = cmd.tasks.find((x) => x.id === id);
  return t ? cmd.taskLabel(t) : `#${id}`;
};

/** Where a click on an item goes: the new session for a spawn, else the task. */
function target(a: Activity, cmd: Command): string {
  if (a.kind === "lane") return `l:${a.session}`;
  if (a.session && (a.kind === "spawn" || a.taskId == null) && cmd.sessions.some((s) => s.name === a.session)) return `s:${a.session}`;
  return a.taskId != null ? `t:${a.taskId}` : `s:${a.session}`;
}

/** In scope (the current project, or everything) and still on screen. */
function useScoped(cmd: Command) {
  const { items } = useActivity();
  const tasks = new Set(cmd.tasks.map((t) => t.id));
  const sessions = new Set([...cmd.sessions.map((s) => s.name), ...cmd.lanes.map((l) => l.name)]);
  return items.filter((a) => (a.taskId != null ? tasks.has(a.taskId) : !!a.session && sessions.has(a.session)));
}

/** Spawns, phase changes and results, newest first; each new one slides in tinted by its colour. */
export function ActivityFeed({ cmd, now, onOpen, limit = 8 }: { cmd: Command; now: number; onOpen: (id: string) => void; limit?: number }) {
  const live = useScoped(cmd);
  const list = [...live, ...laneEvents(cmd), ...seeds(cmd.events, cmd)].sort((a, b) => b.at - a.at).slice(0, limit);
  const label = labeller(cmd);
  if (list.length === 0) return <p className="font-mono text-[11px] text-muted">Quiet. Spawns, phase changes and results show up here as they happen.</p>;
  return (
    <div className="flex flex-col gap-0.5">
      {list.map((a) => {
        const [g, color] = glyph(a);
        const fresh = a.id > 0 && ageOf(a.at, now) < 4000;
        return (
          <button
            key={a.id}
            onClick={() => onOpen(target(a, cmd))}
            className={clsx(
              "-mx-2 grid cursor-pointer grid-cols-[14px_minmax(0,1fr)_auto] items-baseline gap-2 rounded-md px-2 py-1.5 text-left text-text hover:bg-white/[0.03]",
              fresh && "map-feed-in",
            )}
            style={{ "--pc": color } as CSSProperties}
          >
            <span className="text-center font-mono text-[11px] leading-none" style={{ color }}>
              {g}
            </span>
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="truncate font-mono text-[11.5px] leading-[1.25] font-medium">{headline(a, label)}</span>
              <span className="truncate text-[11.5px] leading-[1.3] text-text-2">{a.title}</span>
            </span>
            <time className="font-mono text-[10px] leading-none whitespace-nowrap text-muted/80">{now - a.at < 1500 ? "now" : rel(new Date(a.at).toISOString(), now)}</time>
          </button>
        );
      })}
    </div>
  );
}

/** The newest few changes over the canvas, gone after a few seconds; nothing when quiet. */
export function ActivityTicker({ cmd, now }: { cmd: Command; now: number }) {
  const live = useScoped(cmd)
    .filter((a) => ageOf(a.at, now) < 8000)
    .slice(0, 3);
  const label = labeller(cmd);
  if (live.length === 0) return null;
  return (
    <div className="pointer-events-none absolute top-3 left-3 z-10 flex max-w-[min(340px,calc(100%-24px))] flex-col gap-1.5" aria-live="polite">
      {live.map((a) => {
        const [g, color] = glyph(a);
        const fading = ageOf(a.at, now) > 6000;
        return (
          <div
            key={a.id}
            className={clsx(
              "map-feed-in flex items-center gap-2 rounded-md border bg-menu/90 px-2.5 py-1.5 font-mono text-[11px] shadow-[0_8px_24px_rgba(0,0,0,.35)] transition-opacity duration-1000",
              fading && "opacity-0",
            )}
            style={{ "--pc": color, borderColor: `color-mix(in srgb, ${color} 45%, transparent)` } as CSSProperties}
          >
            <span style={{ color }}>{g}</span>
            <span className="flex-none text-text">{headline(a, label)}</span>
            <span className="truncate text-muted">{a.title}</span>
          </div>
        );
      })}
    </div>
  );
}
