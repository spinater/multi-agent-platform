"use client";

/** The intake gate (backend intake.rs): before any LLM sees a task, the project's intake gate asks the gates Reflex
 * whether it is ready, too unclear, big enough to plan first, or a near-copy of a done task. A held task stays
 * `pending` and waits for the owner: start it anyway, clarify it (the note goes into its prompt) or cancel it. */

import clsx from "clsx";
import { useState } from "react";

import { api } from "@/lib/api";
import type { Task } from "@/lib/generated/Task";
import { VERDICT_LABEL, heldReason, isHeld } from "@/lib/intake";

import { useApp } from "./app-context";
import { Btn, rel } from "./command-ui";

export { heldReason, isHeld };

/** "Waiting for you · Unclear", for a held task's row; `null` when it is not held. */
export function HeldBadge({ task, className }: { task: Task; className?: string }) {
  if (!isHeld(task)) return null;
  return (
    <span
      title={task.intake?.reason}
      className={clsx("inline-flex items-center self-start rounded border border-accent/40 bg-accent/10 px-1.5 py-px font-mono text-[10px] leading-[1.3] font-semibold text-accent", className)}
    >
      Waiting for you · {heldReason(task)}
    </span>
  );
}

/** Opens the planner (Requirements) of `project` in the command centre. */
export function useOpenPlanner() {
  const { setProject, setScope, go } = useApp();
  return (project: string | null) => {
    setProject(project);
    setScope("project");
    go("command", "requirements");
  };
}

/** The intake verdict in a task's detail card, and the owner's choices for a held task. `actions: false` shows it
 * read-only (all-projects mode). */
export function IntakePanel({ task: t, act, actions = true, onOpenTask }: { task: Task; act: (what: string, run: () => Promise<unknown>) => void; actions?: boolean; onOpenTask: (id: number) => void }) {
  const { now } = useApp();
  const openPlanner = useOpenPlanner();
  const [note, setNote] = useState<string | null>(null);
  const i = t.intake;
  if (!i) return null;
  const held = isHeld(t);
  const row = "grid grid-cols-[92px_minmax(0,1fr)] items-baseline gap-3";
  const key = "font-mono text-[10px] leading-[1.5] tracking-[0.08em] text-muted uppercase";
  const verdict = i.verdict === "duplicate" ? heldReason(t) : VERDICT_LABEL[i.verdict];
  return (
    <div className="flex flex-col gap-[7px] px-4 pb-3">
      <div className={row}>
        <span className={key}>Intake</span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <span className={clsx("text-[12.5px] leading-snug", held ? "font-semibold text-accent" : "text-text/90")}>
            {verdict}
            {i.released && <span className="font-normal text-muted"> · you started it anyway</span>}
            {i.tier && <span className="font-normal text-muted"> · tier {i.tier}</span>}
          </span>
          {i.reason && <span className="font-mono text-[11px] leading-snug break-words text-muted">{i.reason}</span>}
          {i.at && <span className="font-mono text-[10.5px] text-muted">{rel(i.at, now)}</span>}
        </span>
      </div>
      {actions && held && (
        <div className="flex flex-col gap-2 rounded-lg border border-accent/35 bg-accent/[0.06] p-2.5">
          <span className="text-[12px] leading-snug text-text-2">
            {i.verdict === "unclear"
              ? "Reflex finds it too unclear to start. Say what it should do, or start it as it is."
              : i.verdict === "plan_first"
                ? "Reflex thinks it is big enough to plan first. Open the planner, or start it as it is."
                : "It looks like a task that is already done. Check that one, or start it anyway."}
          </span>
          <div className="flex flex-wrap gap-1.5">
            <Btn kind="primary" onClick={() => act(`Task #${t.id} started anyway`, () => api.taskAction(t.id, "start_anyway"))}>
              Start anyway
            </Btn>
            <Btn onClick={() => setNote(note === null ? "" : null)}>Clarify</Btn>
            {i.verdict === "plan_first" && <Btn onClick={() => openPlanner(t.project ?? null)}>Open the planner</Btn>}
            {i.verdict === "duplicate" && i.similar !== null && <Btn onClick={() => onOpenTask(i.similar!)}>Open #{i.similar}</Btn>}
          </div>
          {note !== null && (
            <div className="flex flex-col gap-1.5">
              <textarea
                autoFocus
                rows={3}
                value={note}
                onChange={(e) => setNote(e.target.value)}
                placeholder="What the task should do. It is added to the prompt, then intake looks again."
                aria-label="Clarify the task"
                className="resize-y rounded-[7px] border border-line-2 bg-inset px-2.5 py-2 font-mono text-[12px] leading-[1.5] text-text outline-none placeholder:text-muted/80 focus:border-accent"
              />
              <div className="flex gap-1.5">
                <Btn
                  kind="primary"
                  disabled={!note.trim()}
                  onClick={() => {
                    act(`Task #${t.id} clarified · intake looks again`, () => api.taskAction(t.id, "clarify", note.trim()));
                    setNote(null);
                  }}
                >
                  Send
                </Btn>
                <Btn onClick={() => setNote(null)}>Cancel</Btn>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** "Runs alone: changes shared files (Reflex)", beside the parallel scope. */
export function SharedFilesLine({ task }: { task: Task }) {
  if (!task.intake?.shared_files) return null;
  return (
    <div className="grid grid-cols-[92px_minmax(0,1fr)] items-baseline gap-3 px-4 pb-3">
      <span className="font-mono text-[10px] leading-[1.5] tracking-[0.08em] text-muted uppercase">Parallel</span>
      <span className="text-[12.5px] leading-snug text-warn">Runs alone: changes shared files (Reflex)</span>
    </div>
  );
}
