"use client";

/** Configure › Verify & audit: how a task is proven done when its workflow does not say otherwise. Project scope edits
 * the project's verify commands, audit switch and auditor, and its Reflex gates; machine scope shows map.toml's
 * `[orchestrator]` values (read only). */

import { useEffect, useState } from "react";

import { api, type ConfigOp } from "@/lib/api";
import type { PipelineInfo } from "@/lib/generated/PipelineInfo";
import type { ReflexGates } from "@/lib/generated/ReflexGates";

import { useApp } from "../app-context";
import { useDefaults } from "../project-ui";
import { useProjectSettings } from "./project-draft";
import { AskAgent } from "./ask-agent";
import { Box, Lbl, Note, SourceBadge, Switch, TabHead, inputCls, useConfigure } from "./ui";

const GATES: [keyof ReflexGates, string, string][] = [
  [
    "intake",
    "intake",
    "Before any LLM sees a pending task: hold a near-copy of a done task (duplicate), or one that is unclear or should be planned first; otherwise pass it on as light or heavy. Only a confident Reflex holds a task; you can clarify it or start it anyway.",
  ],
  ["audit", "needs_audit", "Ask Reflex before the LLM audit. A confident “no” accepts the task on verification alone."],
  ["tier", "tier", "Ask Reflex for light, standard or heavy for a task with no pinned model, and use [orchestrator.tiers]."],
];

export function VerifyTab() {
  const { scope, slug } = useConfigure();
  const { providers } = useApp();
  const defaults = useDefaults();
  const [pipe, setPipe] = useState<PipelineInfo | null>(null);
  const { value, patch } = useProjectSettings("verify", ["verify", "audit", "reflex_gates"], ["project settings (verify, audit, reflex_gates)"]);

  useEffect(() => {
    api.pipeline().then(setPipe, () => {});
  }, []);

  const machine = scope === "machine" || !slug;
  const cmds = machine ? (pipe?.verify ?? []) : (value?.verify ?? defaults?.verify ?? []);
  const vSource = machine || value?.verify == null ? "machine" : "project";
  const audit = machine ? { enabled: pipe?.audit_enabled ?? false, provider: pipe?.audit_chain[0] ?? null } : (value?.audit ?? { enabled: defaults?.audit_enabled ?? false, provider: null });
  const aSource = machine || value?.audit == null ? "machine" : "project";
  const setCmds = (c: string[]) => patch({ verify: c });
  const gates = value?.reflex_gates ?? { intake: false, audit: false, tier: false };
  const applyOps = (ops: ConfigOp[]) => {
    let n = 0;
    let c = [...cmds];
    let a = { ...audit };
    for (const o of ops) {
      const v = (o.value ?? {}) as { command?: string; index?: number; from?: string; to?: string; value?: unknown };
      if (o.target === "verify.cmds") {
        if (o.op === "add" && v.command) c.splice(Math.min(v.index ?? c.length, c.length), 0, v.command);
        else if (o.op === "remove" && v.command && c.includes(v.command)) c = c.filter((x) => x !== v.command);
        else if (o.op === "change" && v.from && v.to && c.includes(v.from)) c = c.map((x) => (x === v.from ? v.to! : x));
        else continue;
      } else if (o.target === "audit.enabled" && typeof v.value === "boolean") a = { ...a, enabled: v.value };
      else if (o.target === "audit.auditor" && typeof v.value === "string") a = { ...a, provider: v.value };
      else continue;
      n++;
    }
    if (n) patch({ verify: c, audit: a });
    return n;
  };

  return (
    <>
      <TabHead title="Verify & audit" desc="How a task is proven done when its workflow does not say otherwise." savesTo={machine ? "map.toml [orchestrator.verify], [orchestrator.audit] (read only here)" : "project settings in the platform database (verify, audit, reflex_gates)"} />
      {!machine && <AskAgent tab="verify" context={{ verify: cmds, audit, gates, max_attempts: pipe?.max_attempts }} onApply={applyOps} />}
      {machine && <Note>Machine values come from map.toml and are read only here. Switch to Project to give a project its own commands.</Note>}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fit,minmax(min(340px,100%),1fr))]">
        <Box>
          <span className="flex items-center gap-2 text-[14px] leading-none font-semibold">
            <span className="text-ok">✓</span>Verify
            <span className="ml-auto">
              <SourceBadge source={vSource === "machine" && machine ? "read_only" : vSource} label={vSource === "machine" ? "machine" : undefined} />
            </span>
          </span>
          <span className="text-[12px] leading-normal text-text-2">Commands run in order after an agent finishes. All must exit 0.</span>
          {cmds.map((c, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <span className="w-[18px] font-mono text-[11px] text-muted/80">{i + 1}</span>
              <input value={c} readOnly={machine} onChange={(e) => setCmds(cmds.map((x, j) => (j === i ? e.target.value : x)))} className={`${inputCls} flex-1`} />
              {!machine && (
                <>
                  <button
                    onClick={() => {
                      if (!i) return;
                      const n = [...cmds];
                      [n[i - 1], n[i]] = [n[i], n[i - 1]];
                      setCmds(n);
                    }}
                    className="size-[26px] cursor-pointer rounded-[5px] text-muted hover:text-text"
                    aria-label="Move up"
                  >
                    ↑
                  </button>
                  <button onClick={() => setCmds(cmds.filter((_, j) => j !== i))} className="size-[26px] cursor-pointer rounded-[5px] text-muted hover:text-bad" aria-label="Remove">
                    ×
                  </button>
                </>
              )}
            </div>
          ))}
          {!cmds.length && <span className="font-mono text-[11px] text-muted">no verify commands: a task passes once its agent reports done</span>}
          {!machine && (
            <div className="flex flex-wrap gap-2">
              <button onClick={() => setCmds([...cmds, ""])} className="h-7 cursor-pointer self-start rounded-md border border-dashed border-line-3 px-2.5 text-[11.5px] font-medium text-nav">
                + Add command
              </button>
              {value?.verify != null && (
                <button onClick={() => patch({ verify: null })} className="h-7 cursor-pointer rounded-md border px-2.5 text-[11.5px] font-medium text-warn" style={{ borderColor: "color-mix(in srgb, var(--warn) 40%, transparent)" }}>
                  Use machine value
                </button>
              )}
            </div>
          )}
          <div className="flex flex-wrap items-center gap-2.5">
            <Lbl>max attempts</Lbl>
            <span className="font-mono text-[13px] font-semibold">{pipe?.max_attempts ?? "…"}</span>
            <SourceBadge source="read_only" />
          </div>
        </Box>
        <Box className={audit.enabled ? "" : "opacity-60"}>
          <div className="flex items-center gap-2">
            <span className="text-info">◎</span>
            <span className="text-[14px] leading-none font-semibold">Audit</span>
            <SourceBadge source={aSource === "machine" && machine ? "read_only" : aSource} label={aSource === "machine" ? "machine" : undefined} />
            <span className="ml-auto">
              <Switch on={audit.enabled} disabled={machine} onClick={() => patch({ audit: { ...audit, enabled: !audit.enabled } })} />
            </span>
          </div>
          <span className="text-[12px] leading-normal text-text-2">A second agent reads the diff after verify passes. A fail sends the task back with its reasons.</span>
          <label className="flex flex-col gap-1.5">
            <Lbl>auditor</Lbl>
            <select
              value={audit.provider ?? ""}
              disabled={machine}
              onChange={(e) => patch({ audit: { ...audit, provider: e.target.value || null } })}
              className={`${inputCls} font-medium`}
            >
              <option value="">first of the audit chain ({(pipe?.audit_chain ?? []).join(" → ") || "none"})</option>
              {providers.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                  {p.available ? "" : " (not installed)"}
                </option>
              ))}
            </select>
          </label>
          <label className="flex flex-col gap-1.5">
            <Lbl>pass when output contains</Lbl>
            <input value="VERDICT: PASS" readOnly className={inputCls} />
          </label>
          <span className="font-mono text-[10.5px] leading-[1.4] text-muted">workflows can replace these per step (Workflows tab)</span>
        </Box>
        <Box>
          <span className="flex items-center gap-2 text-[14px] leading-none font-semibold">
            <span className="text-ok">⚡</span>Reflex gates
          </span>
          <span className="text-[12px] leading-normal text-text-2">
            Decisions asked of this project&apos;s Reflex first, never of an LLM. When Reflex is off, abstains or is below the threshold, the orchestrator does what it did before.
          </span>
          {machine && <span className="font-mono text-[11px] text-muted">per project: switch to Project to turn gates on</span>}
          {GATES.map(([k, name, desc]) => (
            <div key={k} className="flex items-start gap-2.5">
              <span className="mt-px">
                <Switch on={gates[k]} disabled={machine} onClick={() => patch({ reflex_gates: { ...gates, [k]: !gates[k] } })} />
              </span>
              <span className="flex flex-col gap-1">
                <span className="font-mono text-[12.5px] leading-[1.2] font-semibold">{name}</span>
                <span className="text-[11.5px] leading-[1.45] text-text-2">{desc}</span>
              </span>
            </div>
          ))}
        </Box>
      </div>
    </>
  );
}
