"use client";

import clsx from "clsx";
import { Activity as ActivityIcon, CircleDot, Flag, Hexagon } from "lucide-react";
import type { ReactNode } from "react";

import { providerColor } from "@/lib/colors";
import type { Event } from "@/lib/generated/Event";
import type { OrchestratorStatus } from "@/lib/generated/OrchestratorStatus";

import { ActivityFeed } from "./activity";
import type { Command } from "./command-data";
import { Btn, Pill, SideTitle, TONE_TEXT } from "./command-ui";
import { EventLog } from "./events";
import { SessionRows } from "./sessions";
import { TaskRows } from "./tasks";

/** The command centre's side columns (design: the two `<aside>`s of Agent Command Centre.dc.html). */

function Column({ children, side }: { children: ReactNode; side: "left" | "right" }) {
  return (
    <aside
      className={clsx(
        "flex min-w-0 flex-col gap-7 border-line px-5 pt-5 pb-7 lg:min-h-0 lg:overflow-auto",
        side === "left" ? "lg:border-r" : "border-t lg:border-t-0 lg:border-l",
      )}
    >
      {children}
    </aside>
  );
}

const big = "font-sans text-[36px] leading-none font-semibold tracking-[-0.02em]";
const sub = "font-mono text-[10.5px] leading-[1.4] tracking-[0.04em] text-muted";

export function LeftColumn({
  cmd,
  orchestrator,
  selected,
  select,
  onNewTask,
  onToggleRun,
  onSignIn,
  onOpenProject,
  now,
}: {
  cmd: Command;
  orchestrator: OrchestratorStatus | null;
  selected: string | null;
  select: (id: string) => void;
  onNewTask: () => void;
  onToggleRun: () => void;
  onSignIn: (provider: string) => void;
  onOpenProject: (slug: string | null) => void;
  now: number;
}) {
  const running = !!orchestrator?.running;
  const counts: Record<string, number> = {};
  for (const t of cmd.tasks) counts[t.status] = (counts[t.status] ?? 0) + 1;
  const taskSummary = ["running", "verifying", "auditing", "landing", "review", "pending", "done", "failed"]
    .filter((s) => counts[s])
    .map((s) => `${counts[s]} ${s}`)
    .join(" · ");
  const rowBg = (id: string) => (selected === id ? "bg-accent/[0.08]" : "hover:bg-white/[0.025]");

  return (
    <Column side="left">
      {cmd.all ? (
        <section className="flex flex-col gap-3.5">
          <SideTitle glyph={<CircleDot className="size-3" />} note={`${cmd.sessions.filter((s) => s.state !== "exited").length} live sessions`}>
            All projects
          </SideTitle>
          <div className="flex flex-col gap-2">
            <span className={clsx(big, "text-accent")}>
              {cmd.projectOrder.filter((slug) => cmd.summary(slug).state === "running").length} / {cmd.projectOrder.length} running
            </span>
            <span className={sub}>read-only overview · open a project to act</span>
          </div>
          <div className="flex flex-col overflow-hidden rounded-[10px] border border-line">
            {cmd.projectOrder.map((slug) => {
              const p = cmd.summary(slug);
              return (
                <div
                  key={p.key}
                  onClick={() => select(`p:${p.key}`)}
                  className={clsx("grid cursor-pointer grid-cols-[8px_minmax(0,1fr)_auto] items-center gap-2.5 border-b border-line px-3 py-2.5 last:border-b-0", rowBg(`p:${p.key}`))}
                >
                  <span className="size-2 rounded-full" style={{ background: p.color }} />
                  <span className="flex min-w-0 flex-col gap-1">
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate text-[13px] leading-[1.1] font-medium">{p.name}</span>
                      <Pill tone={p.state === "running" ? "ok" : p.state === "paused" ? "warn" : "idle"} small>
                        {p.state}
                      </Pill>
                    </span>
                    <span className="font-mono text-[10.5px] leading-[1.35] text-muted">
                      {p.live} live · {p.running} running · {p.done}/{p.total} done{p.need ? ` · ${p.need} need you` : ""}
                    </span>
                  </span>
                  <Btn
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      onOpenProject(p.slug);
                    }}
                  >
                    Open →
                  </Btn>
                </div>
              );
            })}
          </div>
        </section>
      ) : (
        <section className="flex flex-col gap-3.5">
          <SideTitle glyph={<Hexagon className="size-3" strokeWidth={2.5} />} note={`${cmd.active}/${orchestrator?.max_parallel ?? 1} active`}>
            Orchestrator
          </SideTitle>
          <div className="flex items-end justify-between gap-3">
            <div className="flex min-w-0 flex-col gap-2">
              <span className={clsx(big, running ? "text-accent" : "text-idle")}>{running ? "Running" : "Paused"}</span>
              <span className={sub}>
                order{" "}
                {(cmd.pipeline?.provider_order ?? cmd.providers.map((p) => p.name)).filter((n) => cmd.providers.find((p) => p.name === n)?.available).join(" → ") || "none available"}
              </span>
            </div>
            <Btn kind={running ? "ghost" : "primary"} className="h-[34px] px-3.5" onClick={onToggleRun} disabled={!orchestrator}>
              {running ? "❚❚ Pause" : "▶ Run"}
            </Btn>
          </div>
          <div className="flex flex-col overflow-hidden rounded-[10px] border border-line">
            {cmd.providers.map((p, i) => {
              const st = cmd.providerState(p, now);
              const live = cmd.sessions.filter((s) => s.provider === p.name && s.state !== "exited").length;
              return (
                <div
                  key={p.name}
                  onClick={() => select(`p:${p.name}`)}
                  className={clsx("grid cursor-pointer grid-cols-[12px_8px_minmax(0,1fr)_auto] items-center gap-2.5 border-b border-line px-3 py-2.5 text-text last:border-b-0", rowBg(`p:${p.name}`))}
                >
                  <span className="font-mono text-[10px] leading-none text-muted/70">{i + 1}</span>
                  <span className="size-2 rounded-full" style={{ background: providerColor(p.name) }} />
                  <span className="flex min-w-0 flex-col gap-1">
                    <span className="text-[13px] leading-[1.1] font-medium">{p.name}</span>
                    <span className="truncate font-mono text-[10.5px] leading-[1.2] text-muted">
                      {p.default_model} · {live} live{st.signedIn ? " · signed in" : ""}
                    </span>
                  </span>
                  {st.signIn ? (
                    <Btn
                      size="sm"
                      kind="accent-ghost"
                      onClick={(e) => {
                        e.stopPropagation();
                        onSignIn(p.name);
                      }}
                    >
                      Sign in
                    </Btn>
                  ) : (
                    <Pill tone={st.tone}>{st.word}</Pill>
                  )}
                </div>
              );
            })}
            {cmd.providers.length === 0 && <p className="px-3 py-2.5 font-mono text-[11px] text-muted">No providers configured.</p>}
          </div>
        </section>
      )}

      <section id="cc-tasks" className="flex flex-col gap-3">
        <SideTitle
          glyph="◇"
          note={
            !cmd.all && (
              <Btn size="sm" kind="primary" className="h-[26px] rounded-md" onClick={onNewTask}>
                + New task
              </Btn>
            )
          }
        >
          {cmd.all ? "All tasks" : "Task queue"}
        </SideTitle>
        {taskSummary && <span className="font-mono text-[10.5px] leading-none text-muted">{taskSummary}</span>}
        <TaskRows tasks={cmd.tasks} label={(t) => `#${t.id}`} selected={selected} onSelect={select} projectOf={cmd.all ? (t) => cmd.projectName(t.project) : undefined} />
      </section>
    </Column>
  );
}

export function RightColumn({
  cmd,
  selected,
  select,
  onNewSession,
  onOpenEvent,
  now,
}: {
  cmd: Command;
  selected: string | null;
  select: (id: string) => void;
  onNewSession: () => void;
  onOpenEvent: (e: Event) => void;
  now: number;
}) {
  const live = cmd.sessions.filter((s) => s.state !== "exited").length;
  const who = (e: Event) => {
    const p = cmd.all ? cmd.eventProject(e) : undefined;
    const head = cmd.all && p !== undefined ? `${cmd.projectName(p)} · ` : "";
    return head + (e.session ?? (e.task_id != null ? `#${e.task_id}` : "orchestrator"));
  };
  return (
    <Column side="right">
      <section className="flex flex-col gap-3">
        <SideTitle glyph={<Flag className="size-3" />} note="watcher can't fix these">
          Needs you
        </SideTitle>
        <div className="flex items-center gap-3.5">
          <span className="font-sans text-[46px] leading-none font-semibold tracking-[-0.02em] text-accent">{cmd.needs.length}</span>
          <span className="font-mono text-[10px] leading-[1.5] font-medium tracking-[0.14em] text-muted uppercase">{cmd.needs.length === 1 ? "item blocked on you" : "items blocked on you"}</span>
        </div>
        <div className="flex flex-col gap-0.5">
          {cmd.needs.map((n) => (
            <button key={n.id} onClick={() => select(n.id)} className="grid cursor-pointer grid-cols-[14px_minmax(0,1fr)] gap-2.5 border-t border-line py-[9px] text-left text-text">
              <Flag className={clsx("mt-0.5 size-3", TONE_TEXT[n.tone])} />
              <span className="flex min-w-0 flex-col gap-[3px]">
                <span className="flex items-baseline justify-between gap-2">
                  <span className="truncate font-mono text-[12.5px] leading-[1.2] font-medium">{n.title}</span>
                  <span className={clsx("flex-none font-mono text-[9.5px] leading-none font-medium tracking-[0.1em] uppercase", TONE_TEXT[n.tone])}>{n.tag}</span>
                </span>
                <span className="line-clamp-3 text-[12px] leading-[1.4] text-pretty text-text-2">{n.reason}</span>
              </span>
            </button>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-2.5">
        <SideTitle glyph={<ActivityIcon className="size-3" />} note="newest first">
          Live activity
        </SideTitle>
        <ActivityFeed cmd={cmd} now={now} onOpen={select} />
      </section>

      <section className="flex flex-col gap-2.5">
        <SideTitle
          glyph="○"
          note={
            <>
              <span>
                {live} live · {cmd.sessions.length - live} exited
              </span>
              {!cmd.all && (
                <Btn size="sm" className="rounded-md" onClick={onNewSession}>
                  + New
                </Btn>
              )}
            </>
          }
        >
          Sessions
        </SideTitle>
        <SessionRows
          sessions={cmd.sessions}
          selected={selected}
          onSelect={select}
          now={now}
          color={cmd.all ? (s) => cmd.colorOfProject(s.project) : undefined}
          prefix={cmd.all ? (s) => cmd.projectName(s.project) : undefined}
        />
      </section>

      <section className="flex flex-col gap-2.5">
        <SideTitle glyph="≡" note="newest first">
          Event log
        </SideTitle>
        <EventLog events={cmd.events} who={who} onOpen={onOpenEvent} />
      </section>
    </Column>
  );
}
