"use client";

/** Parallel tasks: the machine-wide "tasks at once" stepper of the command centre (`POST /api/orchestrator
 * {max_parallel}`, live, no restart), a project's Parallel settings (`settings.parallel`), and what the parallel
 * analyst decided for a task (`task.scope`) or why a pending task still waits (`decision.ready.*`). */

import clsx from "clsx";
import { Minus, Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { api } from "@/lib/api";
import type { BranchMode } from "@/lib/generated/BranchMode";
import type { Event } from "@/lib/generated/Event";
import type { OrchestratorStatus } from "@/lib/generated/OrchestratorStatus";
import type { ProjectParallel } from "@/lib/generated/ProjectParallel";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { TaskScope } from "@/lib/generated/TaskScope";

import { useApp } from "./app-context";
import { rel } from "./command-ui";
import { Btn, Check, Field, Inset, TextInput, errText } from "./project-ui";

// ---- machine-wide limit (command centre)

/** Highest value the stepper offers; the backend has the final say. */
const STEPPER_MAX = 64;

/** "Tasks at once: N" with − / +, saved live (debounced). Shows map.toml's value and a reset when they differ. */
export function ParallelStepper({ orchestrator }: { orchestrator: OrchestratorStatus | null }) {
  const { toast } = useApp();
  // The value just picked, until the server's (pushed over the WebSocket) moves off `base`.
  const [pick, setPick] = useState<{ value: number; base: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  if (!orchestrator) return null;
  const live = orchestrator.max_parallel;
  const config = orchestrator.config_max_parallel;
  const n = pick && pick.base === live ? pick.value : live;

  const send = (value: number, label: string) => {
    setBusy(true);
    api.updateOrchestrator({ max_parallel: value }).then(
      (s) => {
        setBusy(false);
        // Unchanged (or clamped): drop the pick so the server's value shows.
        if (s.max_parallel === live) setPick(null);
        toast(`${label} · ${s.max_parallel} task${s.max_parallel === 1 ? "" : "s"} at once, from the next tick`);
      },
      (e) => {
        setBusy(false);
        setPick(null);
        toast(errText(e), "bad");
      },
    );
  };
  const step = (d: number) => {
    const value = Math.min(STEPPER_MAX, Math.max(1, n + d));
    if (value === n) return;
    setPick({ value, base: live });
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => send(value, "Tasks at once saved"), 350);
  };
  const reset = () => {
    window.clearTimeout(timer.current);
    setPick({ value: config, base: live });
    send(0, "Reset to map.toml");
  };

  const btn = "flex size-6 cursor-pointer items-center justify-center rounded-[5px] border border-line-2 text-nav hover:text-text disabled:cursor-not-allowed disabled:opacity-40";
  return (
    <div
      className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5 rounded-[10px] border border-line px-3 py-2"
      title={`Tasks at once across all projects. A change applies at once (the next orchestrator tick), no restart, and is kept across restarts; running tasks are never stopped. map.toml's [orchestrator] max_parallel is ${config}.`}
    >
      <span className="font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">Tasks at once</span>
      <span className="flex items-center gap-1.5">
        <button type="button" className={btn} disabled={n <= 1} onClick={() => step(-1)} aria-label="Fewer tasks at once">
          <Minus className="size-3" />
        </button>
        <span className={clsx("min-w-7 text-center font-mono text-[15px] font-semibold", pick && pick.base === live && "text-accent")}>{n}</span>
        <button type="button" className={btn} disabled={n >= STEPPER_MAX} onClick={() => step(1)} aria-label="More tasks at once">
          <Plus className="size-3" />
        </button>
      </span>
      <span className="flex w-full items-center justify-between gap-2 font-mono text-[10.5px] text-muted">
        <span>
          {orchestrator.active} active · all projects{busy ? " · saving…" : ""}
        </span>
        {live !== config && (
          <span className="flex items-center gap-1.5">
            map.toml: {config}
            <button type="button" onClick={reset} disabled={busy} className="cursor-pointer text-accent-soft hover:underline disabled:opacity-45">
              reset
            </button>
          </span>
        )}
      </span>
    </div>
  );
}

// ---- project settings

/** The Parallel box: the project's limit, and the analyst that clears tasks to share one folder. `mode` is the form's
 * (possibly unsaved) branch mode, so the explanation follows the Branches box. */
export function ParallelEditor({
  value,
  onChange,
  mode,
  slug,
  providers,
  defaultProvider,
  dirty,
}: {
  value: ProjectParallel;
  onChange: (p: ProjectParallel) => void;
  mode: BranchMode;
  slug: string;
  providers: ProviderInfo[];
  /** Who runs the analyst with no provider picked: the project's audit provider, else the first available. */
  defaultProvider: string | null;
  /** Unsaved settings: the analyst runs with the saved ones. */
  dirty: boolean;
}) {
  const { orchestrator, go, toast } = useApp();
  const [draft, setDraft] = useState<string | null>(null);
  const [run, setRun] = useState<{ busy: boolean; text: string; tone: "ok" | "warn" | "bad" } | null>(null);
  const set = (patch: Partial<ProjectParallel>) => onChange({ ...value, ...patch });
  const machine = orchestrator?.max_parallel;
  const cap = value.max === null ? machine : machine === undefined ? value.max : Math.min(value.max, machine);
  const capText = cap === undefined ? "the machine-wide limit" : String(cap);
  const shared = mode === "shared";

  const text = draft ?? (value.max === null ? "" : String(value.max));
  const commit = (raw: string) => {
    setDraft(null);
    const v = raw.trim();
    if (v === "") return set({ max: null });
    const n = Math.floor(Number(v));
    if (Number.isFinite(n) && n >= 1) set({ max: n });
  };

  const analyze = () => {
    setRun({ busy: true, text: "starting the analyst…", tone: "ok" });
    api.analyzeParallel(slug).then(
      (r) => {
        const t = r.started ? `Analyst started${r.reason ? ` · ${r.reason}` : ""}` : `Not started · ${r.reason || "nothing to analyse"}`;
        setRun({ busy: false, text: t, tone: r.started ? "ok" : "warn" });
        toast(t, r.started ? "ok" : "warn");
      },
      (e) => setRun({ busy: false, text: errText(e), tone: "bad" }),
    );
  };

  return (
    <div className="flex flex-col gap-4">
      <Field
        label="Most tasks at once in this project"
        hint={
          <>
            Empty: only the machine-wide limit ({machine ?? "?"} at once across all projects, changed live in the{" "}
            <button type="button" onClick={() => go("command")} className="cursor-pointer text-accent-soft hover:underline">
              Command centre
            </button>
            ). Applies at the next tick once saved, no restart.
          </>
        }
      >
        <TextInput
          inputMode="numeric"
          value={text}
          onChange={(e) => setDraft(e.target.value.replace(/[^0-9]/g, ""))}
          onBlur={(e) => commit(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && commit(e.currentTarget.value)}
          placeholder={`machine-wide limit (${machine ?? "?"})`}
          className="max-w-[220px]"
        />
      </Field>

      {shared ? (
        <>
          <Check
            on={value.analyst}
            onClick={() => set({ analyst: !value.analyst })}
            label="Parallel analyst"
            sub={
              value.analyst
                ? `One folder and branch: a second task runs beside another only when the analyst agent has cleared both as touching different files (up to ${capText} at once). It runs by itself when tasks wait for clearance.`
                : "One folder and branch: tasks run one at a time. Turn the analyst on to let tasks it clears as touching different files run side by side."
            }
          />
          {value.analyst && (
            <>
              <Field label="Analyst provider" hint="Runs headless, reads the waiting tasks and the code, and declares the paths each one touches.">
                <select
                  value={value.analyst_provider ?? ""}
                  onChange={(e) => set({ analyst_provider: e.target.value || null })}
                  className="h-[34px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[12.5px] tracking-normal text-text normal-case"
                >
                  <option value="">default · audit provider, else the first available{defaultProvider ? ` (${defaultProvider})` : ""}</option>
                  {providers.map((p) => (
                    <option key={p.name} value={p.name} disabled={!p.available}>
                      {p.name} (headless){p.available ? "" : " · not installed"}
                    </option>
                  ))}
                </select>
              </Field>
              <div className="flex flex-wrap items-center gap-2">
                <Btn size="sm" onClick={analyze} disabled={run?.busy}>
                  {run?.busy ? "Starting…" : "Run analyst now"}
                </Btn>
                {dirty && <span className="font-mono text-[11px] text-warn">uses the saved settings; save first</span>}
                {run && !run.busy && <span className={clsx("font-mono text-[11px]", { ok: "text-ok", warn: "text-warn", bad: "text-bad" }[run.tone])}>{run.text}</span>}
              </div>
            </>
          )}
        </>
      ) : (
        <Inset className="text-text-2">Each task has its own branch; up to {capText} run at once, no analyst needed.</Inset>
      )}
    </div>
  );
}

// ---- tasks

/** The orchestrator's waits before starting a task (all `ready → ready`). */
export const READY_WAITS = ["decision.ready.project_full", "decision.ready.shared_busy", "decision.ready.awaiting_analyst", "decision.ready.cleared"];

/** A short line for a pending task whose newest decision is a parallel wait, else `null`. "#n" comes from the
 * message ("… beside #12"). */
export function waitText(e: Event | undefined): string | null {
  if (!e || !READY_WAITS.includes(e.kind)) return null;
  const other = /#(\d+)/.exec(e.message)?.[0];
  switch (e.kind) {
    case "decision.ready.project_full":
      return "waiting: project at its parallel limit";
    case "decision.ready.shared_busy":
      return `waiting: shared folder busy${other ? ` with ${other}` : ""}`;
    case "decision.ready.awaiting_analyst":
      return "waiting: for the parallel analyst";
    default:
      return `cleared to run beside ${other ?? "a running task"}`;
  }
}

/** "independent" / "not independent", with the paths as the tooltip. */
export function ScopeChip({ scope }: { scope: TaskScope }) {
  return (
    <span
      title={`${scope.paths.join(", ") || "no paths"}${scope.reason ? ` · ${scope.reason}` : ""}`}
      className={clsx(
        "inline-flex items-center self-start rounded border px-1.5 py-px font-mono text-[10px] leading-[1.3]",
        scope.independent ? "border-ok/35 bg-ok/10 text-ok" : "border-warn/35 bg-warn/10 text-warn",
      )}
    >
      {scope.independent ? "independent" : "not independent"}
    </span>
  );
}

/** The analyst's verdict in the task card: chip, paths (first 3, then "+n more"), reason, who and when. */
export function ScopePanel({ scope, now }: { scope: TaskScope; now: number }) {
  const [open, setOpen] = useState(false);
  const paths = open ? scope.paths : scope.paths.slice(0, 3);
  const row = "grid grid-cols-[92px_minmax(0,1fr)] items-baseline gap-3";
  const key = "font-mono text-[10px] leading-[1.5] tracking-[0.08em] text-muted uppercase";
  return (
    <div className="flex flex-col gap-[7px] px-4 pb-3">
      <div className={row}>
        <span className={key}>Scope</span>
        <ScopeChip scope={scope} />
      </div>
      <div className={row}>
        <span className={key}>Paths</span>
        <span className="flex min-w-0 flex-col gap-0.5 font-mono text-[11.5px] text-text/90">
          {paths.length === 0 && <span className="text-muted">none declared</span>}
          {paths.map((p) => (
            <span key={p} className="break-all">
              {p === "*" ? "* (the whole repo)" : p}
            </span>
          ))}
          {scope.paths.length > 3 && (
            <button type="button" onClick={() => setOpen(!open)} className="cursor-pointer self-start text-[11px] text-accent-soft hover:underline">
              {open ? "show fewer" : `+${scope.paths.length - 3} more`}
            </button>
          )}
        </span>
      </div>
      {scope.reason && (
        <div className={row}>
          <span className={key}>Reason</span>
          <span className="text-[12.5px] leading-snug text-text/90">{scope.reason}</span>
        </div>
      )}
      <div className={row}>
        <span className={key}>By</span>
        <span className="font-mono text-[11.5px] text-text-2">
          {scope.by.startsWith("analyst:") ? `analyst · ${scope.by.slice(8)}` : `agent · ${scope.by}`} · {rel(scope.at, now)}
        </span>
      </div>
    </div>
  );
}
