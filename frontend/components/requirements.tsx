"use client";

import clsx from "clsx";
import { ChevronDown, ChevronRight, MessageSquareText } from "lucide-react";
import { useEffect, useState } from "react";

import { providerColor } from "@/lib/colors";
import type { Plan } from "@/lib/generated/Plan";
import type { PlannerInfo } from "@/lib/generated/PlannerInfo";
import type { PlanStatus } from "@/lib/generated/PlanStatus";
import type { PlanTask } from "@/lib/generated/PlanTask";
import type { PlanTaskInput } from "@/lib/generated/PlanTaskInput";
import { plansApi, setPlan, usePlans } from "@/lib/use-plans";

import { useApp } from "./app-context";
import { Btn, Chip, Pill, SideTitle, TASK_TONE, rel, type Tone } from "./command-ui";
import { ConsoleView, closeSession } from "./console";

/** Requirements → plan → tasks (backend plans.rs): talk to the project's planner agent in its session, then approve,
 * edit or reject the draft plans it submits. Approving creates the tasks; the orchestrator runs them. */

const STATUS_TONE: Record<PlanStatus, Tone> = { draft: "accent", approved: "ok", rejected: "bad", superseded: "idle" };

const err = (e: unknown) => (e as Error).message;

/** The tasks in an order where each comes after its dependencies (ties as submitted), with their depth. */
export function planOrder(tasks: PlanTask[]): { task: PlanTask; depth: number }[] {
  const depth = new Map<string, number>();
  const out: { task: PlanTask; depth: number }[] = [];
  const left = [...tasks];
  while (left.length) {
    const i = left.findIndex((t) => t.depends_on.every((d) => depth.has(d)));
    // A cycle cannot be stored; show what is left as is rather than loop.
    const [task] = left.splice(i < 0 ? 0 : i, 1);
    const d = task.depends_on.length ? Math.max(...task.depends_on.map((k) => (depth.get(k) ?? 0) + 1)) : 0;
    depth.set(task.key, d);
    out.push({ task, depth: d });
  }
  return out;
}

const toInput = (t: PlanTask): PlanTaskInput => ({ key: t.key, title: t.title, prompt: t.prompt, depends_on: t.depends_on, provider: t.provider ?? undefined, model: t.model ?? undefined });

/** The planner (a chat in its session) and the project's plans. `project: null` is the default workspace. */
export function RequirementsPanel({ project }: { project: string | null }) {
  return (
    <div className="grid min-w-0 gap-6 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
      <PlannerChat project={project} />
      <PlanList project={project} />
    </div>
  );
}

function PlannerChat({ project }: { project: string | null }) {
  const { sessions, providers, server, toast, openAuth } = useApp();
  const [loaded, setLoaded] = useState<{ project: string | null; info: PlannerInfo } | null>(null);
  const [picked, setPicked] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let live = true;
    plansApi
      .planner(project)
      .then((info) => live && setLoaded({ project, info }))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [project]);

  const info = loaded?.project === project ? loaded.info : null;
  const name = info?.session_name ?? `${server?.session_prefix ?? "map-"}plan${project ? `-${project}` : ""}`;
  const session = sessions.find((s) => s.name === name);
  const live = !!session && session.state !== "exited";
  const usable = providers.filter((p) => p.available);
  const provider = picked || info?.default_provider || usable.find((p) => p.signed_in !== false)?.name || "";

  const start = async () => {
    setBusy(true);
    try {
      const s = await plansApi.startPlanner(project, provider || undefined);
      toast(`Planner started on ${s.provider} · it reads the project, then greets you`);
    } catch (e) {
      toast(err(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="flex min-w-0 flex-col gap-3">
      <SideTitle glyph={<MessageSquareText className="size-3" />} note={session ? <Pill tone={live ? "accent" : "idle"}>{live ? session.state.replace("_", " ") : "ended"}</Pill> : "built-in agent"}>
        Planner
      </SideTitle>
      <p className="text-[12.5px] leading-[1.55] text-pretty text-text-2">
        Tell it what you want, in your own words and language. It reads the project, asks only what changes the plan (each option with what happens if you pick it), then submits a draft plan of
        small tasks below. It never writes code; nothing runs until you approve.
      </p>
      {live && session ? (
        <>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 font-mono text-[10.5px] text-muted">
            <span className="flex items-center gap-1.5 text-text-2">
              <span className="size-1.5 rounded-full" style={{ background: providerColor(session.provider) }} />
              {session.name}
            </span>
            <span>
              {session.provider} · {session.model}
            </span>
            <span className="hidden sm:inline">tmux attach -t &apos;={session.name}&apos;</span>
            <Btn size="sm" kind="danger" className="ml-auto" onClick={() => void closeSession(session, toast)}>
              End session
            </Btn>
          </div>
          <ConsoleView session={session} readOnly={false} toast={toast} className="h-[min(58vh,540px)] min-h-[300px]" />
        </>
      ) : (
        <div className="flex flex-col gap-3 rounded-[10px] border border-line bg-inset p-3.5">
          <div className="flex flex-col gap-2">
            <span className="font-mono text-[10.5px] font-medium tracking-[0.08em] text-muted uppercase">Provider</span>
            <div className="flex flex-wrap gap-1.5">
              {usable.map((p) => (
                <Chip
                  key={p.name}
                  on={provider === p.name}
                  onClick={() => {
                    if (p.signed_in === false) openAuth(p.name);
                    setPicked(p.name);
                  }}
                >
                  <span className="size-1.5 rounded-full" style={{ background: providerColor(p.name) }} />
                  {p.name}
                  {p.signed_in === false ? " · sign in" : ""}
                </Chip>
              ))}
              {usable.length === 0 && <span className="font-mono text-[11px] text-muted">No provider CLI was found on this machine.</span>}
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2.5">
            <Btn kind="primary" size="lg" disabled={!provider || busy} onClick={() => void start()}>
              {busy ? "Starting…" : "Talk to the planner"}
            </Btn>
            <span className="min-w-0 font-mono text-[10.5px] leading-[1.4] text-muted">
              {session ? `${name} ended · a new session starts fresh; your plans stay` : `starts ${name} in ${project === null ? "the default workspace" : "this project's folder"}`}
            </span>
          </div>
          {session && <ConsoleView session={session} readOnly toast={toast} className="h-[180px]" />}
        </div>
      )}
    </section>
  );
}

function PlanList({ project }: { project: string | null }) {
  const { plans, error } = usePlans();
  const { now } = useApp();
  // A plan approved in the last few minutes stays open, so its new tasks are one click away.
  const fresh = (p: Plan) => p.status === "approved" && !!p.decided_at && now - new Date(p.decided_at).getTime() < 10 * 60_000;
  const [showOld, setShowOld] = useState(false);
  const mine = (plans ?? []).filter((p) => (p.project ?? null) === project);
  const drafts = mine.filter((p) => p.status === "draft");
  const decided = mine.filter((p) => p.status !== "draft");
  const shown = showOld ? decided : decided.filter((p) => p.status !== "superseded").slice(0, 6);
  const hidden = decided.length - shown.length;
  return (
    <section className="flex min-w-0 flex-col gap-3">
      <SideTitle glyph="◇" note={`${drafts.length} waiting · ${mine.length} in all`}>
        Plans
      </SideTitle>
      {error && <p className="font-mono text-[11px] text-bad">Could not load plans: {error}</p>}
      {plans && mine.length === 0 && (
        <p className="rounded-[10px] border border-dashed border-line-3 px-3.5 py-4 text-[12.5px] leading-[1.55] text-text-2">
          No plans yet. When the planner submits one, it shows here as a draft: read each task&apos;s prompt, edit it if needed, then approve to create the tasks.
        </p>
      )}
      {drafts.map((p) => (
        <PlanCard key={p.id} plan={p} project={project} open />
      ))}
      {shown.map((p) => (
        <PlanCard key={p.id} plan={p} project={project} open={fresh(p)} />
      ))}
      {(hidden > 0 || showOld) && decided.length > 0 && (
        <button onClick={() => setShowOld(!showOld)} className="self-start font-mono text-[11px] text-muted hover:text-text">
          {showOld ? "show fewer" : `show ${hidden} older or superseded`}
        </button>
      )}
    </section>
  );
}

function PlanCard({ plan, project, open: initiallyOpen }: { plan: Plan; project: string | null; open: boolean }) {
  const { tasks, orchestrator, projects, toast, now, go, setProject, setScope } = useApp();
  const paused = projects.find((p) => p.project.slug === project)?.project.settings.paused;
  const [open, setOpen] = useState(initiallyOpen);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const draft = plan.status === "draft";
  const ordered = planOrder(plan.tasks);

  const openTask = (id: number) => {
    setProject(project);
    setScope("project");
    go("command", `tasks/${id}`);
  };

  const approve = async () => {
    const n = plan.tasks.length;
    const notes = [!orchestrator?.running && "The orchestrator is paused right now.", paused && "This project is paused right now."].filter(Boolean).join(" ");
    const text = `Approve plan #${plan.id} “${plan.title}”?\n\nThis creates ${n} task${n === 1 ? "" : "s"}; the orchestrator runs them when it is running and the project isn't paused.${notes ? `\n\n${notes}` : ""}`;
    if (!confirm(text)) return;
    setBusy(true);
    try {
      const res = await plansApi.approve(plan.id);
      setPlan(res.plan);
      const ids = res.tasks.map((t) => t.id);
      toast(`Plan #${plan.id} approved · ${ids.length} tasks created${ids.length ? ` (#${Math.min(...ids)}–#${Math.max(...ids)})` : ""}`);
    } catch (e) {
      toast(err(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  const reject = async () => {
    const reason = prompt(`Reject plan #${plan.id}? Say why, so the planner can revise it (optional):`, "");
    if (reason === null) return;
    setBusy(true);
    try {
      setPlan(await plansApi.reject(plan.id, reason.trim() || undefined));
      toast(`Plan #${plan.id} rejected`, "warn");
    } catch (e) {
      toast(err(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  const save = async (changed: PlanTaskInput) => {
    const list = plan.tasks.map((t) => (t.key === changed.key ? changed : toInput(t)));
    try {
      setPlan(await plansApi.edit(plan.id, { tasks: list }));
      setEditing(null);
      toast(`Plan #${plan.id} · ${changed.key} saved`);
    } catch (e) {
      toast(err(e), "bad");
    }
  };

  return (
    <article className={clsx("flex min-w-0 flex-col rounded-[10px] border", draft ? "border-accent/45 bg-accent/[0.035]" : "border-line bg-surface", plan.status === "superseded" && "opacity-70")}>
      <button onClick={() => setOpen(!open)} className="flex min-w-0 cursor-pointer items-start gap-2.5 px-3.5 py-3 text-left text-text" aria-expanded={open}>
        {open ? <ChevronDown className="mt-0.5 size-3.5 flex-none text-muted" /> : <ChevronRight className="mt-0.5 size-3.5 flex-none text-muted" />}
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
            <span className="font-mono text-[12px] text-muted">#{plan.id}</span>
            <span className="min-w-0 text-[14px] leading-[1.25] font-semibold break-words">{plan.title}</span>
          </span>
          <span className="font-mono text-[10.5px] leading-[1.3] text-muted">
            rev {plan.revision}
            {plan.replaces != null ? ` · revises #${plan.replaces}` : ""} · {plan.tasks.length} task{plan.tasks.length === 1 ? "" : "s"} · {rel(plan.created_at, now)}
            {plan.decided_at && !draft ? ` · ${plan.status} ${rel(plan.decided_at, now)}` : ""}
          </span>
        </span>
        <Pill tone={STATUS_TONE[plan.status]}>{draft ? "waiting for you" : plan.status}</Pill>
      </button>
      {open && (
        <div className="flex min-w-0 flex-col gap-3 border-t border-line px-3.5 pt-3 pb-3.5">
          {plan.summary && <p className="text-[12.5px] leading-[1.55] whitespace-pre-wrap text-text-2">{plan.summary}</p>}
          {plan.reason && <p className="font-mono text-[11.5px] leading-[1.45] text-bad">rejected: {plan.reason}</p>}
          <ol className="flex min-w-0 flex-col">
            {ordered.map(({ task: t, depth }) => {
              const real = t.task_id != null ? tasks.find((x) => x.id === t.task_id) : undefined;
              const isOpen = expanded === t.key;
              return (
                <li key={t.key} className="flex min-w-0 flex-col border-t border-line first:border-t-0">
                  <div className="flex min-w-0 items-start gap-2 py-2" style={{ paddingLeft: Math.min(depth, 4) * 14 }}>
                    <button onClick={() => setExpanded(isOpen ? null : t.key)} className="flex min-w-0 flex-1 cursor-pointer items-start gap-2 text-left text-text" aria-expanded={isOpen}>
                      <span className={clsx("mt-px flex-none rounded border px-1 font-mono text-[10px] leading-[16px]", depth ? "border-line-2 text-nav" : "border-accent/40 text-accent-soft")}>{t.key}</span>
                      <span className="flex min-w-0 flex-col gap-0.5">
                        <span className="text-[12.5px] leading-[1.35] font-medium break-words">{t.title}</span>
                        <span className="font-mono text-[10px] leading-[1.35] text-muted">
                          {t.depends_on.length ? `after ${t.depends_on.join(", ")}` : "no dependencies"}
                          {t.provider ? ` · ${t.provider}${t.model ? ` ${t.model}` : ""}` : ""}
                        </span>
                      </span>
                    </button>
                    {t.task_id != null && (
                      <button onClick={() => openTask(t.task_id as number)} className="flex flex-none cursor-pointer items-center gap-1.5 font-mono text-[11px] text-accent-soft hover:underline" title="Open in the command centre">
                        #{t.task_id}
                        {real && <Pill tone={TASK_TONE[real.status]} small>{real.status}</Pill>}
                      </button>
                    )}
                  </div>
                  {isOpen && editing !== t.key && (
                    <div className="mb-2.5 flex min-w-0 flex-col gap-2" style={{ marginLeft: Math.min(depth, 4) * 14 }}>
                      <pre className="max-h-72 overflow-auto rounded-lg border border-line bg-inset p-2.5 font-mono text-[11.5px] leading-[1.5] whitespace-pre-wrap text-text-2">{t.prompt}</pre>
                      {draft && (
                        <Btn size="sm" className="self-start" onClick={() => setEditing(t.key)}>
                          Edit task
                        </Btn>
                      )}
                    </div>
                  )}
                  {isOpen && editing === t.key && <TaskEditor task={t} others={plan.tasks.filter((x) => x.key !== t.key).map((x) => x.key)} onCancel={() => setEditing(null)} onSave={save} />}
                </li>
              );
            })}
          </ol>
          {draft ? (
            <div className="flex flex-wrap items-center gap-2 border-t border-line pt-3">
              <Btn kind="primary" disabled={busy} onClick={() => void approve()}>
                Approve · create {plan.tasks.length} task{plan.tasks.length === 1 ? "" : "s"}
              </Btn>
              <Btn kind="danger" disabled={busy} onClick={() => void reject()}>
                Reject
              </Btn>
              <span className="min-w-0 font-mono text-[10.5px] leading-[1.4] text-muted">click a task to read its prompt; ask the planner in the chat for bigger changes</span>
            </div>
          ) : (
            plan.status === "approved" && (
              <div className="flex flex-wrap items-center gap-2 border-t border-line pt-3 font-mono text-[11px] text-muted">
                <span>created</span>
                {plan.tasks
                  .filter((t) => t.task_id != null)
                  .map((t) => (
                    <button key={t.key} onClick={() => openTask(t.task_id as number)} className="cursor-pointer text-accent-soft hover:underline">
                      #{t.task_id}
                    </button>
                  ))}
                <span>· in the command centre</span>
              </div>
            )
          )}
        </div>
      )}
    </article>
  );
}

const field = "w-full min-w-0 rounded-[7px] border border-line-2 bg-inset px-2.5 text-text outline-none placeholder:text-muted/80 focus:border-accent";

function TaskEditor({ task, others, onCancel, onSave }: { task: PlanTask; others: string[]; onCancel: () => void; onSave: (t: PlanTaskInput) => Promise<void> }) {
  const [title, setTitle] = useState(task.title);
  const [prompt, setPrompt] = useState(task.prompt);
  const [deps, setDeps] = useState<string[]>(task.depends_on);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    await onSave({ ...toInput(task), title, prompt, depends_on: others.filter((k) => deps.includes(k)) });
    setBusy(false);
  };
  return (
    <form
      className="mb-3 flex min-w-0 flex-col gap-2.5 rounded-lg border border-accent/35 p-2.5"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <input value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Task title" className={clsx(field, "h-8 text-[12.5px] font-medium")} />
      <textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} rows={10} aria-label="Task prompt" className={clsx(field, "resize-y py-2 font-mono text-[11.5px] leading-[1.5]")} />
      {others.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="font-mono text-[10px] tracking-[0.08em] text-muted uppercase">after</span>
          {others.map((k) => (
            <Chip key={k} on={deps.includes(k)} className="h-6 px-2 text-[10.5px]" onClick={() => setDeps(deps.includes(k) ? deps.filter((d) => d !== k) : [...deps, k])}>
              {k}
            </Chip>
          ))}
        </div>
      )}
      <div className="flex gap-2">
        <Btn type="submit" size="sm" kind="primary" disabled={busy || !title.trim() || !prompt.trim()}>
          {busy ? "Saving…" : "Save"}
        </Btn>
        <Btn type="button" size="sm" onClick={onCancel}>
          Cancel
        </Btn>
      </div>
    </form>
  );
}
