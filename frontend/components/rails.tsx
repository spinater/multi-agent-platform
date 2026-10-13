"use client";

import clsx from "clsx";
import { Pause, Play, RotateCw } from "lucide-react";
import { useState } from "react";

import { api } from "@/lib/api";
import type { OrchestratorStatus } from "@/lib/generated/OrchestratorStatus";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { ReflexState } from "@/lib/generated/ReflexState";
import type { Session } from "@/lib/generated/Session";
import type { Task } from "@/lib/generated/Task";

import { Dot, Meter, Section, relative, type Tone } from "./ui";

const pad = (n: number) => String(n).padStart(2, "0");

function isoWeek(d: Date): number {
  const t = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = t.getUTCDay() || 7;
  t.setUTCDate(t.getUTCDate() + 4 - day);
  const start = new Date(Date.UTC(t.getUTCFullYear(), 0, 1));
  return Math.ceil(((t.getTime() - start.getTime()) / 86400000 + 1) / 7);
}

function Clock({ now }: { now: number }) {
  const d = new Date(now);
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const offset = -d.getTimezoneOffset() / 60;
  const angle = (deg: number) => ({ transform: `rotate(${deg}deg)` });
  return (
    <div className="flex items-center gap-4">
      <div className="relative size-14 shrink-0 rounded-full border border-line">
        <span className="absolute bottom-1/2 left-1/2 h-4 w-0.5 origin-bottom -translate-x-1/2 bg-text" style={angle((d.getHours() % 12) * 30 + d.getMinutes() / 2)} />
        <span className="absolute bottom-1/2 left-1/2 h-5.5 w-px origin-bottom -translate-x-1/2 bg-text" style={angle(d.getMinutes() * 6)} />
        <span className="absolute bottom-1/2 left-1/2 h-6 w-px origin-bottom -translate-x-1/2 bg-accent" style={angle(d.getSeconds() * 6)} />
      </div>
      <div>
        <p className="text-4xl leading-none font-semibold tracking-tight tabular-nums">
          {pad(d.getHours())}:{pad(d.getMinutes())}
          <span className="text-lg text-accent">:{pad(d.getSeconds())}</span>
        </p>
        <p className="label mt-1">
          {tz.split("/").pop()?.replace("_", " ")} · UTC{offset >= 0 ? "+" : ""}
          {offset}
        </p>
        <p className="mt-0.5 text-[11px] text-muted">
          UTC {pad(d.getUTCHours())}:{pad(d.getUTCMinutes())}
        </p>
      </div>
    </div>
  );
}

export function LeftRail({
  now,
  tasks,
  sessions,
  projects,
  orchestrator,
  onOpenTask,
}: {
  now: number;
  tasks: Task[];
  sessions: Session[];
  projects: ProjectInfo[];
  orchestrator: OrchestratorStatus | null;
  onOpenTask: (id: number) => void;
}) {
  const d = new Date(now);
  const count = (s: string) => tasks.filter((t) => t.status === s).length;
  const finished = tasks.filter((t) => ["done", "failed", "cancelled"].includes(t.status)).length;
  const decisions = projects.reduce((acc, p) => ({ total: acc.total + p.stats.total, reflex: acc.reflex + p.stats.reflex }), { total: 0, reflex: 0 });
  const backlog = tasks.filter((t) => t.status === "pending" || t.status === "failed");
  const live = sessions.filter((s) => s.state !== "exited");

  return (
    <aside className="flex min-w-0 flex-col border-line lg:border-r">
      <Section title="Today" note={`Wk${isoWeek(d)} · ${d.toLocaleDateString(undefined, { weekday: "short", day: "2-digit", month: "short", year: "numeric" })}`}>
        <Clock now={now} />
      </Section>

      <Section title="Orchestrator">
        <div className="flex items-center justify-between gap-2">
          <p className="text-[15px]">
            {orchestrator?.running ? "Running" : "Paused"}
            <span className="ml-2 text-[11px] text-muted">
              {orchestrator?.active ?? 0}/{orchestrator?.max_parallel ?? 1} slots
            </span>
          </p>
          {orchestrator && (
            <button
              onClick={() => api.setOrchestrator(!orchestrator.running)}
              className={clsx("flex h-7 items-center gap-1.5 rounded border px-2 text-[11px]", orchestrator.running ? "border-line hover:border-muted" : "border-accent text-accent")}
            >
              {orchestrator.running ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}
              {orchestrator.running ? "pause" : "run"}
            </button>
          )}
        </div>
        <div className="mt-3 grid grid-cols-[4.5rem_1fr] items-center gap-x-2 gap-y-1.5 text-[11px] text-muted">
          {(["pending", "running", "verifying", "done", "failed"] as const).map((s) => (
            <div key={s} className="contents">
              <span>{s}</span>
              <span className="flex items-center gap-1">
                {Array.from({ length: Math.min(24, Math.max(1, count(s))) }, (_, i) => (
                  <span key={i} className={clsx("h-2 w-2.5 rounded-[1px]", count(s) === 0 ? "bg-panel-2" : s === "failed" ? "bg-bad" : s === "done" ? "bg-muted" : "bg-accent")} />
                ))}
                <span className="ml-1 tabular-nums">{count(s)}</span>
              </span>
            </div>
          ))}
        </div>
      </Section>

      <Section title="Activity record" note="since this database was created">
        <div className="grid gap-2.5 text-[13px]">
          <div>
            <div className="flex justify-between">
              <span>tasks finished</span>
              <span className="tabular-nums">
                {finished} / {tasks.length}
              </span>
            </div>
            <Meter value={finished} max={tasks.length} tone="info" />
          </div>
          <div>
            <div className="flex justify-between">
              <span>answered by Reflex</span>
              <span className="tabular-nums">
                {decisions.reflex} / {decisions.total}
              </span>
            </div>
            <Meter value={decisions.reflex} max={decisions.total} tone="ok" />
          </div>
          <p className="text-[11px] text-muted">
            sessions live {live.length} · projects {projects.length} · LLM calls avoided{" "}
            {decisions.total ? Math.round((100 * decisions.reflex) / decisions.total) : 0}%
          </p>
        </div>
      </Section>

      <Section title="Backlog" note="pending and failed tasks" className="border-b-0">
        <div className="flex items-end gap-3">
          <span className="text-5xl leading-none font-light text-accent tabular-nums">{backlog.length}</span>
          <span className="label pb-1 leading-tight">
            waiting
            <br />
            actions
          </span>
        </div>
        <ul className="mt-3 grid gap-1.5">
          {backlog.slice(0, 8).map((t) => (
            <li key={t.id}>
              <button className="flex w-full items-baseline gap-2 text-left text-[12px] hover:text-accent" onClick={() => onOpenTask(t.id)}>
                <span className="text-muted">#{t.id}</span>
                <span className="truncate">{t.title}</span>
                <span className={clsx("ml-auto shrink-0 text-[10px] uppercase", t.status === "failed" ? "text-bad" : "text-muted")}>{t.status}</span>
              </button>
            </li>
          ))}
        </ul>
      </Section>
    </aside>
  );
}

const reflexTone: Record<ReflexState, Tone> = { running: "ok", starting: "info", no_corpus: "warn", error: "bad", stopped: "idle" };
const attention = ["waiting_input", "rate_limited", "needs_auth", "stalled", "error"];

export function RightRail({
  now,
  sessions,
  providers,
  projects,
  onOpenSession,
  onStartSession,
  onOpenProject,
}: {
  now: number;
  sessions: Session[];
  providers: ProviderInfo[];
  projects: ProjectInfo[];
  onOpenSession: (name: string) => void;
  onStartSession: (provider: string) => void;
  onOpenProject: (slug: string) => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const waiting = sessions.filter((s) => attention.includes(s.state));
  const working = sessions.filter((s) => s.state === "working" || s.state === "starting");
  const totals = projects.reduce((a, p) => ({ reflex: a.reflex + p.stats.reflex, llm: a.llm + p.stats.llm, abstain: a.abstain + p.stats.abstain }), { reflex: 0, llm: 0, abstain: 0 });

  const restart = async (slug: string) => {
    setBusy(slug);
    try {
      await api.restartReflex(slug);
    } finally {
      setBusy(null);
    }
  };

  return (
    <aside className="flex min-w-0 flex-col border-line lg:border-l">
      <Section title="Now working">
        <p className="text-4xl leading-none font-semibold tabular-nums">
          {working.length}
          <span className="ml-2 text-base font-normal text-muted">agent{working.length === 1 ? "" : "s"}</span>
        </p>
        <ul className="mt-2 grid gap-1">
          {working.slice(0, 4).map((s) => (
            <li key={s.name}>
              <button className="w-full truncate text-left text-[12px] text-muted hover:text-text" onClick={() => onOpenSession(s.name)}>
                › {s.name} <span className="opacity-70">{s.last_line ?? ""}</span>
              </button>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Waiting">
        {waiting.length === 0 ? (
          <p className="text-[12px] text-muted">No session needs you.</p>
        ) : (
          <ul className="grid gap-1.5">
            {waiting.map((s) => (
              <li key={s.name}>
                <button className="flex w-full items-center gap-2 text-left text-[12px] hover:text-accent" onClick={() => onOpenSession(s.name)}>
                  <Dot tone={s.state === "rate_limited" || s.state === "stalled" ? "warn" : s.state === "waiting_input" ? "accent" : "bad"} />
                  <span className="truncate">{s.name}</span>
                  <span className="ml-auto shrink-0 text-[10px] text-muted uppercase">
                    {s.state === "rate_limited" && s.rate_limit_reset_at ? relative(s.rate_limit_reset_at, now) : s.state.replace("_", " ")}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Section>

      <Section title="Agents deck" note="start a session">
        <div className="grid grid-cols-3 gap-2">
          {providers.map((p) => (
            <button
              key={p.name}
              disabled={!p.available}
              onClick={() => onStartSession(p.name)}
              title={p.available ? `Start ${p.name}` : `${p.name} is not installed`}
              className="group flex flex-col items-center gap-1.5 disabled:opacity-40"
            >
              <span className="flex size-9 items-center justify-center rounded border border-line bg-panel-2 text-accent group-hover:border-accent group-disabled:text-muted">
                <Play className="size-3.5 fill-current" />
              </span>
              <span className="text-[10px] text-muted">/{p.name}</span>
              {p.cooldown_until && <span className="text-[9px] text-warn">back {relative(p.cooldown_until, now)}</span>}
            </button>
          ))}
        </div>
      </Section>

      <Section title="Projects" note={`${projects.filter((p) => p.reflex.state === "running").length}/${projects.length} Reflex engines running`} className="border-b-0">
        <table className="w-full text-[12px]">
          <thead>
            <tr className="label text-left">
              <th className="w-6 pb-1.5 font-medium" />
              <th className="pb-1.5 font-medium">project</th>
              <th className="pb-1.5 text-right font-medium">reflex</th>
            </tr>
          </thead>
          <tbody>
            {projects.map(({ project: p, reflex }) => (
              <tr key={p.slug} className="border-t border-line/60">
                <td className="py-1.5">
                  <button onClick={() => restart(p.slug)} disabled={busy === p.slug} title="Restart Reflex" className="text-muted hover:text-accent disabled:animate-spin">
                    <RotateCw className="size-3.5" />
                  </button>
                </td>
                <td className="py-1.5">
                  <button className="truncate hover:text-accent" onClick={() => onOpenProject(p.slug)}>
                    {p.slug}
                  </button>
                </td>
                <td className="py-1.5 text-right">
                  <span className="inline-flex items-center gap-1.5 text-[10px] uppercase">
                    <Dot tone={reflexTone[reflex.state]} />
                    {reflex.state.replace("_", " ")}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {projects.length === 0 && <p className="text-[12px] text-muted">No projects yet.</p>}
        <p className="label mt-4">
          decisions · reflex {totals.reflex} · llm {totals.llm} · abstain {totals.abstain}
        </p>
      </Section>
    </aside>
  );
}
