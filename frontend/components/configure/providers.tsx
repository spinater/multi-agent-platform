"use client";

/** Configure › Providers: the agent CLIs on this machine (binary, found, sign-in) and their order. Machine scope
 * shows `[orchestrator] provider_order` from map.toml (read only); project scope edits the project's own list
 * (`settings.providers`: order, model, max parallel sessions), empty meaning the machine order. MAP never signs a
 * CLI out: that is done in the CLI itself. */

import { useEffect, useState } from "react";

import { api, gitApi } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { PipelineInfo } from "@/lib/generated/PipelineInfo";
import type { ProjectProvider } from "@/lib/generated/ProjectProvider";
import type { SettingsInfo } from "@/lib/generated/SettingsInfo";

import { useApp } from "../app-context";
import { useProjectSettings } from "./project-draft";
import { SourceBadge, Stepper, TabHead, useConfigure } from "./ui";

export function ProvidersTab() {
  const { scope, slug } = useConfigure();
  const { providers, openAuth } = useApp();
  const [pipe, setPipe] = useState<PipelineInfo | null>(null);
  const [settings, setSettings] = useState<SettingsInfo | null>(null);
  const { value, patch } = useProjectSettings("providers", ["providers"], ["project settings (providers)"]);

  useEffect(() => {
    api.pipeline().then(setPipe, () => {});
    gitApi.settings().then(setSettings, () => {});
  }, []);

  const machine = scope === "machine" || !slug || !value;
  const machineOrder = pipe?.provider_order ?? providers.map((p) => p.name);
  const own = value?.providers ?? [];
  // The project's list, or the machine order to start from.
  const rows: ProjectProvider[] = machine
    ? machineOrder.map((name) => ({ name, model: null, max_parallel: null }))
    : own.length
      ? own
      : machineOrder.map((name) => ({ name, model: null, max_parallel: null }));
  const rest = providers.filter((p) => !rows.some((r) => r.name === p.name));
  const set = (next: ProjectProvider[]) => patch({ providers: next });
  const move = (i: number, d: number) => {
    const j = i + d;
    if (j < 0 || j >= rows.length) return;
    const n = [...rows];
    [n[i], n[j]] = [n[j], n[i]];
    set(n);
  };

  return (
    <>
      <TabHead
        title="Providers"
        desc="The agent CLIs on this machine, their sign-in state and order."
        savesTo={machine ? "map.toml [orchestrator] provider_order, [providers.*] (read only here)" : "project settings (providers)"}
      />
      <div className="flex flex-col gap-2">
        {rows.map((r, i) => {
          const info = providers.find((p) => p.name === r.name);
          const bin = settings?.agents.find((a) => a.provider === r.name);
          const found = !!info?.available;
          const signed = info?.signed_in;
          const auth = !found ? "binary not found" : signed === true ? "signed in" : signed === false ? "not signed in" : "sign-in unknown";
          const authColor = !found ? "var(--muted)" : signed === true ? "var(--ok)" : signed === false ? "var(--bad)" : "var(--muted)";
          const src = machine ? "machine" : own.length ? "project" : "machine";
          return (
            <div key={r.name} className="flex flex-wrap items-center gap-x-4 gap-y-2.5 rounded-[10px] border border-line bg-surface px-3.5 py-[13px]">
              {!machine && (
                <span className="flex flex-col gap-0.5">
                  <button onClick={() => move(i, -1)} className="h-4 w-[22px] cursor-pointer text-[10px] text-muted hover:text-text" aria-label="Up">
                    ▲
                  </button>
                  <button onClick={() => move(i, 1)} className="h-4 w-[22px] cursor-pointer text-[10px] text-muted hover:text-text" aria-label="Down">
                    ▼
                  </button>
                </span>
              )}
              <span className="font-mono text-[11px] text-muted/80">{i + 1}</span>
              <span className="flex min-w-0 flex-[1_1_200px] flex-col gap-1.5">
                <span className="flex items-center gap-2 text-[14px] leading-none font-semibold">
                  <span className="size-2 rounded-full" style={{ background: providerColor(r.name) }} />
                  {r.name}
                </span>
                <span className="truncate font-mono text-[11px] leading-[1.2] text-muted">{bin ? (bin.path ?? bin.binary) : "…"}</span>
              </span>
              <span className="font-mono text-[11px] font-medium" style={{ color: found ? "var(--ok)" : "var(--bad)" }}>
                {found ? "● found" : "○ not found"}
              </span>
              <span className="font-mono text-[11px] font-medium" style={{ color: authColor }}>
                {auth}
              </span>
              {machine ? (
                <span className="font-mono text-[11px] text-text-2">model {info?.default_model ?? "default"}</span>
              ) : (
                <span className="flex items-center gap-2 font-mono text-[11px] text-text-2">
                  max
                  <Stepper
                    value={r.max_parallel ?? "∞"}
                    width={32}
                    onMinus={() => set(rows.map((x, j) => (j === i ? { ...x, max_parallel: Math.max(1, (x.max_parallel ?? 2) - 1) } : x)))}
                    onPlus={() => set(rows.map((x, j) => (j === i ? { ...x, max_parallel: x.max_parallel == null ? null : x.max_parallel + 1 } : x)))}
                  />
                  parallel
                </span>
              )}
              <SourceBadge source={src} />
              {!found ? (
                <a href="https://github.com/spinater/multi-agent-platform#agent-clis" target="_blank" rel="noreferrer" className="flex h-[30px] items-center rounded-[7px] border border-line-3 px-3 text-[12px] font-semibold text-nav">
                  Install guide
                </a>
              ) : signed !== true ? (
                <button onClick={() => openAuth(r.name)} className="h-[30px] cursor-pointer rounded-[7px] border border-accent bg-accent px-3 text-[12px] font-semibold whitespace-nowrap text-accent-text">
                  Login
                </button>
              ) : (
                <span className="font-mono text-[10.5px] text-muted" title="MAP never signs a CLI out; run the CLI's own logout if you need to">
                  sign out in the CLI
                </span>
              )}
              {!machine && own.length > 0 && (
                <button onClick={() => set(rows.filter((_, j) => j !== i))} className="size-[26px] cursor-pointer text-muted hover:text-bad" aria-label="Remove from this project">
                  ×
                </button>
              )}
            </div>
          );
        })}
        {!machine && rest.length > 0 && (
          <div className="flex flex-wrap items-center gap-2 font-mono text-[11px] text-muted">
            add:
            {rest.map((p) => (
              <button key={p.name} onClick={() => set([...rows, { name: p.name, model: null, max_parallel: null }])} className="h-6 cursor-pointer rounded-[5px] border border-line-2 px-2 text-text-2 hover:text-text">
                + {p.name}
              </button>
            ))}
          </div>
        )}
        {!machine && own.length > 0 && (
          <button onClick={() => set([])} className="h-7 cursor-pointer self-start rounded-md border px-2.5 text-[11.5px] font-medium text-warn" style={{ borderColor: "color-mix(in srgb, var(--warn) 40%, transparent)" }}>
            Use machine value
          </button>
        )}
        <span className="font-mono text-[10.5px] leading-normal text-muted">order = provider_order: the orchestrator tries providers top to bottom when a task names none</span>
      </div>
    </>
  );
}
