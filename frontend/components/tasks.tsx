"use client";

import clsx from "clsx";
import { useState } from "react";

import { api } from "@/lib/api";
import type { PipelineInfo } from "@/lib/generated/PipelineInfo";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Task } from "@/lib/generated/Task";

import { Btn, Chip, Pill, TASK_TONE } from "./command-ui";
import { Modal } from "./shell/page";

const FINISHED = ["done", "failed", "cancelled"];

/** True when a pending task waits on a dependency that failed or was cancelled. */
export function blocked(t: Task, tasks: Task[]): boolean {
  return t.status === "pending" && t.depends_on.some((d) => {
    const x = tasks.find((y) => y.id === d);
    return !!x && (x.status === "failed" || x.status === "cancelled");
  });
}

/** The task queue of the left column (design: `taskRows`). */
export function TaskRows({ tasks, label, selected, onSelect, projectOf }: { tasks: Task[]; label: (t: Task) => string; selected: string | null; onSelect: (id: string) => void; projectOf?: (t: Task) => string }) {
  if (tasks.length === 0) return <p className="font-mono text-[11px] text-muted">No tasks yet. Add one; with the orchestrator running it starts once its dependencies are done.</p>;
  return (
    <div className="flex flex-col gap-1.5">
      {tasks.map((t) => {
        const on = selected === `t:${t.id}`;
        const isBlocked = blocked(t, tasks);
        return (
          <button
            key={t.id}
            onClick={() => onSelect(`t:${t.id}`)}
            className={clsx(
              "grid cursor-pointer grid-cols-[minmax(30px,auto)_minmax(0,1fr)_auto] items-center gap-2 rounded-lg border px-2.5 py-[9px] text-left text-text",
              on ? "border-accent/45 bg-accent/[0.08]" : "border-line bg-surface hover:border-line-2",
              t.status === "cancelled" && "opacity-50",
            )}
          >
            <span className="font-mono text-[10.5px] leading-none text-muted">{label(t)}</span>
            <span className="flex min-w-0 flex-col gap-1">
              <span className="text-[12.5px] leading-[1.25] font-medium text-pretty">{t.title}</span>
              <span className="font-mono text-[10px] leading-[1.2] text-muted">
                {projectOf ? `${projectOf(t)} · ` : ""}
                {t.depends_on.length ? `after ${t.depends_on.map((d) => `#${d}`).join(", ")}` : "no deps"}
                {t.provider ? ` · ${t.provider}` : ""}
                {t.attempts > 1 ? ` · try ${t.attempts}` : ""}
              </span>
            </span>
            <Pill tone={isBlocked ? "bad" : TASK_TONE[t.status]}>{isBlocked ? "blocked" : t.status}</Pill>
          </button>
        );
      })}
    </div>
  );
}

/** Retry, cancel and delete, as the detail card's buttons. Returns `null` while nothing applies. */
export function taskActions(t: Task, act: (what: string, run: () => Promise<unknown>) => void) {
  const out: { label: string; kind: "primary" | "ghost" | "danger"; run: () => void }[] = [];
  if (t.status === "failed" || t.status === "cancelled") out.push({ label: "Retry", kind: "primary", run: () => act(`Task #${t.id} queued for retry`, () => api.taskAction(t.id, "retry")) });
  if (!FINISHED.includes(t.status))
    out.push({ label: "Cancel", kind: "danger", run: () => confirm(`Cancel task #${t.id}?`) && act(`Task #${t.id} cancelled`, () => api.taskAction(t.id, "cancel")) });
  if (FINISHED.includes(t.status) || t.status === "pending")
    out.push({ label: "Delete", kind: "danger", run: () => confirm(`Delete task #${t.id}?`) && act(`Task #${t.id} deleted`, () => api.deleteTask(t.id)) });
  return out;
}

const fieldLabel = "flex flex-col gap-1.5 font-mono text-[11px] leading-none font-medium tracking-[0.08em] text-muted uppercase";
const field = "rounded-[7px] border border-line-2 bg-inset text-text outline-none placeholder:text-muted/80 focus:border-accent font-normal tracking-normal normal-case";

/** New task (design: the "New task · #N" modal). The task runs in the current project. */
export function NewTaskModal({
  tasks,
  providers,
  pipeline,
  project,
  projectName,
  onClose,
  onCreated,
  nextId,
}: {
  tasks: Task[];
  providers: ProviderInfo[];
  pipeline: PipelineInfo | null;
  project: string | null;
  projectName: string;
  onClose: () => void;
  onCreated: (t: Task) => void;
  /** Task ids are global, so the next one comes from every project. */
  nextId: number;
}) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [deps, setDeps] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const info = providers.find((p) => p.name === provider);
  const ok = title.trim() && prompt.trim() && !busy;

  const pick = (name: string) => {
    setProvider(name);
    setModel("");
  };

  const submit = async () => {
    if (!ok) return;
    setBusy(true);
    setError("");
    try {
      const t = await api.createTask({ title: title.trim(), prompt: prompt.trim(), depends_on: deps, provider: provider || undefined, model: model || undefined, project: project ?? undefined });
      onCreated(t);
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  };

  const verify = pipeline?.verify.length ?? 0;
  return (
    <Modal onClose={onClose} label="New task" width={580}>
      <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-4">
        <span className="flex items-center gap-2 font-mono text-[11px] font-semibold tracking-[0.14em] uppercase">
          <span className="text-accent">◇</span>New task · #{nextId}
          <span className="font-normal tracking-[0.06em] text-muted normal-case">in {projectName}</span>
        </span>
        <button onClick={onClose} className="size-[26px] cursor-pointer rounded-md border border-line-2 text-nav" aria-label="Close">
          ×
        </button>
      </div>
      <div className="flex flex-col gap-4 p-[18px]">
        <label className={fieldLabel}>
          Title
          <input autoFocus value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Add rate limiting to the login endpoint" className={clsx(field, "h-[38px] px-3 font-sans text-[13.5px]")} />
        </label>
        <label className={fieldLabel}>
          Prompt for the worker
          <textarea
            rows={6}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="What to build, which files matter, and how to prove it works."
            className={clsx(field, "resize-y px-3 py-2.5 font-mono text-[12.5px] leading-[1.6]")}
          />
        </label>
        <div className="flex flex-col gap-2">
          <span className={fieldLabel}>Provider</span>
          <div className="flex flex-wrap gap-1.5">
            <Chip on={provider === ""} onClick={() => pick("")}>
              any · failover
            </Chip>
            {providers
              .filter((p) => p.available)
              .map((p) => (
                <Chip key={p.name} on={provider === p.name} onClick={() => pick(p.name)}>
                  {p.name}
                </Chip>
              ))}
          </div>
        </div>
        {info && (
          <div className="flex flex-col gap-2">
            <span className={fieldLabel}>Model</span>
            <div className="flex flex-wrap gap-1.5">
              <Chip on={model === ""} onClick={() => setModel("")}>
                default · {info.default_model}
              </Chip>
              {info.models.map((m) => (
                <Chip key={m} on={model === m} onClick={() => setModel(m)}>
                  {m}
                </Chip>
              ))}
            </div>
          </div>
        )}
        <div className="flex flex-col gap-2">
          <span className={fieldLabel}>Runs after</span>
          <div className="flex max-h-[120px] flex-wrap gap-1.5 overflow-auto">
            {tasks.filter((t) => t.status !== "cancelled").length === 0 && <span className="font-mono text-[11px] text-muted">no other tasks in this project</span>}
            {tasks
              .filter((t) => t.status !== "cancelled")
              .map((t) => (
                <Chip key={t.id} on={deps.includes(t.id)} className="h-[26px] px-[9px] text-[11px] font-normal" onClick={() => setDeps(deps.includes(t.id) ? deps.filter((d) => d !== t.id) : [...deps, t.id])}>
                  #{t.id} {t.title.length > 44 ? `${t.title.slice(0, 44)}…` : t.title}
                </Chip>
              ))}
          </div>
        </div>
        <p className="font-mono text-[11px] leading-[1.6] text-pretty text-muted">
          The orchestrator appends the $MAP_RESULT_FILE protocol{verify ? ` and the ${verify} verify command${verify === 1 ? "" : "s"}` : ""} to this prompt
          {pipeline?.audit_enabled ? `, then an audit by another agent (${pipeline.audit_chain.join(" → ")}) must return VERDICT: PASS` : ""}.
        </p>
        {error && <p className="text-[12px] text-bad">{error}</p>}
      </div>
      <div className="flex justify-end gap-2 border-t border-line px-[18px] py-3.5">
        <Btn size="lg" onClick={onClose}>
          Cancel
        </Btn>
        <Btn size="lg" kind="primary" disabled={!ok} onClick={submit}>
          {busy ? "Adding…" : "Add task"}
        </Btn>
      </div>
    </Modal>
  );
}
