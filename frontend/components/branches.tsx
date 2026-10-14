"use client";

/** A project's Branches settings (`settings.branches`): shared folder or a branch and worktree per task, and, while
 * worktrees are on, what running N tasks side by side would cost this machine (`POST /worktree-estimate`). */

import clsx from "clsx";
import { Minus, Plus, X } from "lucide-react";
import { useEffect, useState } from "react";

import { api } from "@/lib/api";
import type { BranchMode } from "@/lib/generated/BranchMode";
import type { LandMode } from "@/lib/generated/LandMode";
import type { Level } from "@/lib/generated/Level";
import type { ProjectBranches } from "@/lib/generated/ProjectBranches";
import type { ProjectDefaults } from "@/lib/generated/ProjectDefaults";
import type { WorktreeEstimate } from "@/lib/generated/WorktreeEstimate";

import { LEVEL_TONE, TONE_TEXT } from "./command-ui";
import { Btn, Field, Inset, Segmented, TextInput, errText } from "./project-ui";

const MODES: [BranchMode, string, string][] = [
  ["shared", "Shared folder", "Every task works in the project folder itself, one at a time per file; nothing to merge."],
  ["worktree", "Branch + worktree per task", "Each task gets branch map/task-<id> in its own git worktree, so several run at once; MAP lands each when it passes."],
];

const LANDS: [LandMode, string, string][] = [
  ["merge", "Merge", "Rebase onto the base branch and fast-forward it; a conflict goes back to the agent."],
  ["pull_request", "Pull request", "Push the branch and open a pull request (gh) or merge request (glab)."],
  ["keep", "Keep", "Leave the branch for you to review; the task waits in review."],
];

const label = "font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase";

/** `bytes` as 1 decimal KB/MB/GB (base 1024), whole numbers from 10 up. */
export function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "unknown";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 || v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export function BranchesEditor({ value, onChange, slug, defaults }: { value: ProjectBranches; onChange: (b: ProjectBranches) => void; slug?: string; defaults: ProjectDefaults | null }) {
  const set = (patch: Partial<ProjectBranches>) => onChange({ ...value, ...patch });
  const worktree = value.mode === "worktree";
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-col gap-2" role="radiogroup" aria-label="Where tasks run">
        {MODES.map(([mode, title, sub]) => (
          <button
            key={mode}
            type="button"
            role="radio"
            aria-checked={value.mode === mode}
            onClick={() => set({ mode })}
            className={clsx("flex items-start gap-2.5 rounded-lg border px-3 py-2.5 text-left text-text", value.mode === mode ? "border-accent/60 bg-accent/[0.06]" : "border-line hover:border-line-2")}
          >
            <span className={clsx("mt-0.5 flex size-3.5 shrink-0 items-center justify-center rounded-full border", value.mode === mode ? "border-accent" : "border-white/25")}>
              {value.mode === mode && <span className="size-1.5 rounded-full bg-accent" />}
            </span>
            <span className="flex flex-col gap-1">
              <span className="text-[13px] leading-tight font-medium">{title}</span>
              <span className="font-mono text-[11px] leading-snug text-muted">{sub}</span>
            </span>
          </button>
        ))}
      </div>
      {worktree && (
        <>
          <Field label="Base branch" hint="Tasks start from it and land into it. Empty: the branch checked out in the project folder.">
            <TextInput value={value.base ?? ""} onChange={(e) => set({ base: e.target.value })} placeholder="the branch checked out" />
          </Field>
          <div className="flex flex-col gap-2">
            <span className={label}>when a task passes</span>
            <Segmented value={value.land} options={LANDS.map(([v, t]) => [v, t])} onChange={(land) => set({ land })} />
            <p className="font-mono text-[11px] leading-snug text-muted">{LANDS.find(([v]) => v === value.land)?.[2]}</p>
          </div>
          <ListEditor title="setup" hint="Run in each new worktree before the agent starts." placeholder="bun install" items={value.setup} onChange={(setup) => set({ setup })} />
          <ListEditor title="copy" hint="Untracked files or folders copied from the project folder into each worktree; never committed." placeholder=".env" items={value.copy} onChange={(copy) => set({ copy })} />
          {slug ? (
            <EstimatePanel slug={slug} copy={value.copy} setup={value.setup} defaults={defaults} />
          ) : (
            <p className="font-mono text-[11px] text-muted">Once the project exists, its settings show what running several tasks at once costs this machine.</p>
          )}
        </>
      )}
    </div>
  );
}

/** A list of strings: remove with ×, add with Enter. */
function ListEditor({ title, hint, placeholder, items, onChange }: { title: string; hint: string; placeholder: string; items: string[]; onChange: (items: string[]) => void }) {
  const [draft, setDraft] = useState("");
  const add = () => {
    const v = draft.trim();
    if (v && !items.includes(v)) onChange([...items, v]);
    setDraft("");
  };
  return (
    <div className="flex flex-col gap-2">
      <span className={label}>{title}</span>
      {items.map((c, i) => (
        <div key={`${i}-${c}`} className="flex h-[34px] items-center gap-2 rounded-[7px] border border-line bg-inset pr-1.5 pl-3">
          <span className="min-w-0 flex-1 truncate font-mono text-[12.5px]">{c}</span>
          <button type="button" onClick={() => onChange(items.filter((_, j) => j !== i))} className="flex size-6 items-center justify-center text-muted hover:text-text" aria-label={`Remove ${c}`}>
            <X className="size-3.5" />
          </button>
        </div>
      ))}
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            add();
          }
        }}
        onBlur={add}
        placeholder={`${placeholder}, Enter to add`}
        aria-label={`Add to ${title}`}
        className="h-[34px] rounded-[7px] border border-dashed border-line-3 bg-inset px-3 font-mono text-[12.5px] outline-none placeholder:text-muted/70 focus:border-accent"
      />
      <span className="font-mono text-[11px] leading-snug text-muted">{hint}</span>
    </div>
  );
}

const SEVERITY: Record<Level, number> = { error: 0, warning: 1, info: 2, success: 3 };

/** Used against available as a bar: red when over. `avail: null` is unknown to MAP. */
function UsageBar({ title, used, avail, of, fmt = fmtBytes }: { title: string; used: number; avail: number | null; of?: string; fmt?: (n: number) => string }) {
  const over = avail !== null && used > avail;
  const pct = avail ? Math.min(100, (used / avail) * 100) : 0;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex justify-between gap-2 font-mono text-[11px]">
        <span className="text-text-2">{title}</span>
        <span className={over ? "text-bad" : "text-nav"}>
          {fmt(used)} of {avail === null ? "unknown" : fmt(avail)} {of}
          {over ? " · over" : ""}
        </span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-[3px] bg-white/5">
        <div className={clsx("h-full rounded-[3px]", over ? "bg-bad" : pct > 80 ? "bg-warn" : "bg-ok")} style={{ width: `${avail === null ? 0 : Math.max(2, pct)}%` }} />
      </div>
    </div>
  );
}

/** What N tasks side by side would cost. Refetches (debounced) when N or the form's unsaved copy/setup lists change. */
function EstimatePanel({ slug, copy, setup, defaults }: { slug: string; copy: string[]; setup: string[]; defaults: ProjectDefaults | null }) {
  // `null` until the owner picks N: the server answers for `orchestrator.max_parallel`.
  const [parallel, setParallel] = useState<number | null>(null);
  const [result, setResult] = useState<{ key: string; est: WorktreeEstimate | null; error: string } | null>(null);
  const key = JSON.stringify({ parallel, copy, setup });

  useEffect(() => {
    let stale = false;
    const body = JSON.parse(key) as { parallel: number | null; copy: string[]; setup: string[] };
    const t = window.setTimeout(() => {
      api.worktreeEstimate(slug, { parallel: body.parallel ?? undefined, copy: body.copy, setup: body.setup }).then(
        (est) => !stale && setResult({ key, est, error: "" }),
        (e) => !stale && setResult((r) => ({ key, est: r?.est ?? null, error: errText(e) })),
      );
    }, 400);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [slug, key]);

  const loading = result?.key !== key;
  const est = result?.est ?? null;
  const n = parallel ?? est?.parallel ?? defaults?.max_parallel ?? 1;
  const max = Math.max(est?.machine.cpus ?? 0, 8);
  const pick = (v: number) => setParallel(Math.min(max, Math.max(1, v)));
  const m = est?.machine;
  const memAvail = m ? (m.memory_available ?? m.memory_total) : null;
  const globalMax = defaults?.max_parallel;

  return (
    <Inset className="gap-3.5 font-sans text-[12.5px]">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className={label}>cost of running tasks side by side</span>
        <span className="font-mono text-[10.5px] text-muted">{loading ? "estimating…" : "rough estimate"}</span>
      </div>

      {m && (
        <p className="font-mono text-[11px] leading-relaxed text-text-2">
          This machine: {m.cpus} CPU threads · {fmtBytes(m.memory_total)} RAM ({m.memory_available === null ? "free unknown" : `${fmtBytes(m.memory_available)} free`}) ·{" "}
          {m.disk_free === null ? "free disk unknown" : `${fmtBytes(m.disk_free)} free disk`} at {m.disk_path}
        </p>
      )}

      <div className="flex flex-col gap-2">
        <span className="font-mono text-[11px] text-text-2">Tasks at once</span>
        <div className="flex items-center gap-2">
          <button type="button" className="flex size-7 items-center justify-center rounded border border-line-2 text-nav disabled:opacity-40" disabled={n <= 1} onClick={() => pick(n - 1)} aria-label="Fewer tasks at once">
            <Minus className="size-3" />
          </button>
          <input type="range" min={1} max={max} value={n} onChange={(e) => pick(Number(e.target.value))} className="min-w-0 flex-1 accent-[var(--accent)]" aria-label="Tasks at once" />
          <button type="button" className="flex size-7 items-center justify-center rounded border border-line-2 text-nav disabled:opacity-40" disabled={n >= max} onClick={() => pick(n + 1)} aria-label="More tasks at once">
            <Plus className="size-3" />
          </button>
          <span className="min-w-8 text-center font-mono text-[15px] font-semibold">{n}</span>
        </div>
      </div>

      {result?.error && <p className="text-[12px] text-bad">Could not estimate: {result.error}</p>}
      {!est && !result?.error && <p className="font-mono text-[11px] text-muted">Measuring the project folder…</p>}

      {est && (
        <div className={clsx("flex flex-col gap-3.5 transition-opacity", loading && "opacity-60")}>
          <div className="flex flex-col gap-1.5">
            <span className="font-mono text-[11px] text-text-2">Each task needs</span>
            <div className="grid gap-x-4 gap-y-1 font-mono text-[11px] leading-snug sm:grid-cols-2">
              <div className="flex flex-col gap-1">
                <span className="text-text">disk {fmtBytes(est.per_task_disk)}</span>
                <span className="text-muted">
                  checkout {fmtBytes(est.checkout_bytes)} ({est.tracked_files} files)
                </span>
                {est.extra.map((x) => (
                  <span key={x.path} className="text-muted">
                    {x.path} {fmtBytes(x.bytes)} · {x.copied ? "copied" : "rebuilt by setup"}
                  </span>
                ))}
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-text">memory {fmtBytes(est.per_task_memory)}</span>
                {est.agents.map((a) => (
                  <span key={a.provider} className="text-muted">
                    {a.provider} CLI {fmtBytes(a.bytes)} · {a.measured ? `measured from ${a.measured} running` : "assumed"}
                  </span>
                ))}
                <span className="text-muted">
                  build {fmtBytes(est.build_memory)} · {est.build_note}
                </span>
              </div>
            </div>
          </div>

          <div className="flex flex-col gap-2.5">
            <span className="font-mono text-[11px] text-text-2">With {est.parallel} at once</span>
            <UsageBar title="memory" used={est.total_memory} avail={memAvail} of={m?.memory_available === null ? "total" : "free"} />
            <UsageBar title="disk" used={est.total_disk} avail={m?.disk_free ?? null} of="free" />
            <UsageBar title="CPU at peak" used={est.peak_threads} avail={m?.cpus ?? null} of="threads" fmt={String} />
          </div>

          <div className="flex flex-wrap items-center gap-2 font-mono text-[11px]">
            <span className={est.parallel > est.recommended_parallel ? "text-warn" : "text-ok"}>
              Recommended: up to {est.recommended_parallel} at once
            </span>
            <span className="text-muted">· limited by {est.limited_by}</span>
            {est.recommended_parallel !== n && (
              <Btn size="sm" tone="quiet" onClick={() => pick(est.recommended_parallel)}>
                Use {est.recommended_parallel}
              </Btn>
            )}
          </div>

          <p className="font-mono text-[11px] leading-snug text-text-2">
            {est.open_tasks === 0
              ? "No open tasks in this project right now."
              : `Its ${est.open_tasks} open task${est.open_tasks === 1 ? "" : "s"}: ${est.waves_serial} round${est.waves_serial === 1 ? "" : "s"} one at a time → ${est.waves_parallel} with ${est.parallel} at once${est.waves_parallel < est.waves_serial ? ` (about ${(est.waves_serial / Math.max(1, est.waves_parallel)).toFixed(1)}× faster)` : " (dependencies keep them in order)"}.`}
          </p>

          {est.drawbacks.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="font-mono text-[11px] text-text-2">What it trades for the speed</span>
              {[...est.drawbacks]
                .sort((a, b) => SEVERITY[a.level] - SEVERITY[b.level])
                .map((d, i) => (
                  <div key={i} className={clsx("flex flex-col gap-0.5 border-l-2 pl-2.5", { error: "border-bad", warning: "border-warn", info: "border-info", success: "border-ok" }[d.level])}>
                    <span className={clsx("text-[12.5px] font-medium", TONE_TEXT[LEVEL_TONE[d.level]])}>{d.title}</span>
                    <span className="font-mono text-[11px] leading-snug text-muted">{d.detail}</span>
                  </div>
                ))}
            </div>
          )}
        </div>
      )}

      <p className={clsx("font-mono text-[11px] leading-snug", globalMax !== undefined && n > globalMax ? "text-warn" : "text-muted")}>
        MAP runs at most {globalMax ?? "?"} task{globalMax === 1 ? "" : "s"} at once across all projects ([orchestrator] max_parallel in map.toml); this setting does not change that.
        {globalMax !== undefined && n > globalMax ? ` To run ${n} at once, set [orchestrator] max_parallel = ${n} in map.toml, then restart MAP.` : ""}
      </p>
    </Inset>
  );
}
