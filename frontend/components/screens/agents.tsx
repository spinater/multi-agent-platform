"use client";

import { Plus, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { AgentRow } from "@/lib/generated/AgentRow";
import type { AgentScope } from "@/lib/generated/AgentScope";
import type { AgentsOverview } from "@/lib/generated/AgentsOverview";
import type { PlannerInstructions } from "@/lib/generated/PlannerInstructions";
import { plansApi } from "@/lib/use-plans";
import { providerName } from "@/lib/use-usage";

import { AgentEditor } from "../agents/agent-editor";
import { AgentMatrix } from "../agents/agent-matrix";
import { agentArg, AGENT_PROVIDERS, parseAgentArg, PROVIDER_FORMAT, PROVIDER_LABEL } from "../agents/shared";
import { useApp } from "../app-context";
import { Main, Modal, PageHeader, SectionTitle } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";
import { Button, Empty } from "../ui";

/** Agent files change outside MAP (an editor, a CLI); read them again this often. */
const REFRESH_MS = 30_000;

type Loaded = { project: string | null; data: AgentsOverview } | { project: string | null; error: string };

/** Where each CLI looks, for the empty state: the folders the scanner read, in `[agents]` order. */
function Folders({ data }: { data: AgentsOverview }) {
  return (
    <div className="grid gap-2.5 sm:grid-cols-3">
      {AGENT_PROVIDERS.map((p) => (
        <div key={p} className="flex min-w-0 flex-col gap-1.5 rounded-xl border border-line bg-surface px-4 py-3">
          <span className="flex items-center gap-2 font-mono text-[12px] font-semibold text-text">
            <span className="size-1.5 rounded-full" style={{ background: providerColor(p) }} />
            {PROVIDER_LABEL[p]}
            <span className="font-normal text-muted">{PROVIDER_FORMAT[p]}</span>
          </span>
          {data.dirs
            .filter((d) => d.provider === p)
            .map((d) => (
              <span key={`${d.scope}:${d.path}`} className="font-mono text-[11px] leading-[1.5] break-all text-text-2">
                <span className="text-muted">{d.scope === "project" ? "project " : "user "}</span>
                {d.path}
                {!d.exists && <span className="text-muted"> · not there yet</span>}
              </span>
            ))}
        </div>
      ))}
    </div>
  );
}

/** Agents MAP itself runs in every project (read-only here; `[templates] planner_agent` in map.toml replaces the
 * instructions). */
function BuiltinAgents() {
  const { go } = useApp();
  const [text, setText] = useState<PlannerInstructions | { error: string } | null>(null);
  const [open, setOpen] = useState(false);
  const read = () => {
    setOpen(true);
    plansApi
      .instructions()
      .then(setText)
      .catch((e: Error) => setText({ error: e.message }));
  };
  return (
    <section className="flex flex-col gap-3">
      <SectionTitle glyph="◆" note="run by MAP · read-only">
        Built in
      </SectionTitle>
      <div className="flex flex-col rounded-xl border border-line bg-surface">
        <div className="flex flex-wrap items-start gap-x-4 gap-y-2.5 px-4 py-3">
          <span className="flex min-w-0 flex-1 basis-[280px] flex-col gap-1">
            <span className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-[13px] font-semibold text-text">planner</span>
              <span className="inline-flex h-[18px] items-center rounded border border-accent/35 bg-accent/10 px-1.5 font-mono text-[9px] font-medium tracking-[0.1em] whitespace-nowrap text-accent uppercase">
                built-in · every project
              </span>
            </span>
            <span className="text-[12px] leading-[1.45] text-nav">
              Requirements analyst and planner you talk to: it reads the project, asks what changes the plan, and submits draft plans of tasks that wait for your approval. Never writes code.
              Session <span className="font-mono">&lt;prefix&gt;plan-&lt;slug&gt;</span>, any provider.
            </span>
          </span>
          <div className="flex flex-wrap gap-2">
            <Button onClick={read}>Instructions</Button>
            <Button onClick={() => go("command", "requirements")}>Talk to it →</Button>
          </div>
        </div>
      </div>
      {open && (
        <Modal onClose={() => setOpen(false)} label="Planner instructions" width={860}>
          <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-3.5">
            <span className="flex min-w-0 flex-col gap-1">
              <span className="font-mono text-[11px] font-semibold tracking-[0.14em] uppercase">Planner instructions</span>
              <span className="truncate font-mono text-[10.5px] text-muted">
                {text && "source" in text ? (text.source === "built-in" ? "built in · backend/templates/planner-agent.md · replace with [templates] planner_agent" : text.source) : "…"}
              </span>
            </span>
            <button onClick={() => setOpen(false)} className="size-[26px] flex-none cursor-pointer rounded-md border border-line-2 text-nav" aria-label="Close">
              ×
            </button>
          </div>
          <div className="p-[18px]">
            {text && "error" in text ? (
              <p className="text-sm text-bad">{text.error}</p>
            ) : (
              <pre className="max-h-[65vh] overflow-auto rounded-lg border border-line bg-inset p-3 font-mono text-[11.5px] leading-[1.55] whitespace-pre-wrap text-text-2">{text?.content ?? "Loading…"}</pre>
            )}
          </div>
        </Modal>
      )}
    </section>
  );
}

/** Custom sub-agents for the current project and user-wide, and which of Claude Code, Codex and agy has each one.
 * `#agents/<scope>/<name>` opens one in the editor, `#agents/new` a blank one. */
export function AgentsScreen() {
  const { project, current, server, route, go } = useApp();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [tick, setTick] = useState(0);

  const refresh = useCallback(() => setTick((t) => t + 1), []);
  useEffect(() => {
    let live = true;
    api
      .agents(project)
      .then((data) => live && setLoaded({ project, data }))
      .catch((e: Error) => live && setLoaded({ project, error: e.message }));
    const timer = window.setInterval(refresh, REFRESH_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [project, tick, refresh]);

  const projectName = current?.project.name ?? (project === null ? workspaceName(server?.workspace_dir) : project);
  const ours = loaded && loaded.project === project ? loaded : null;
  const data = ours && "data" in ours ? ours.data : null;
  const rows = data?.rows ?? [];
  const target = parseAgentArg(route.arg);
  const close = useCallback(() => go("agents"), [go]);
  const open = (r: AgentRow) => go("agents", agentArg(r.scope, r.name));
  const saved = (scope: AgentScope, name: string) => {
    refresh();
    go("agents", agentArg(scope, name));
  };
  const deleted = () => {
    refresh();
    go("agents");
  };
  const full = (p: (typeof AGENT_PROVIDERS)[number]) => rows.filter((r) => r.files.some((f) => f.provider === p && !f.error)).length;

  return (
    <>
      <TopBar />
      <Main className="gap-[26px]">
        <PageHeader
          kicker={`Project · ${projectName}`}
          title="Agents"
          lead={`Custom sub-agents for this project and for you, written in each CLI's own format: Claude Code and agy read Markdown, Codex reads TOML.${data?.readers.length ? ` ${data.readers.map((r) => providerName(r.provider, true)).join(" and ")} load the Claude Code files of a project.` : ""}`}
          actions={
            <>
              <div className="mr-2 flex flex-wrap items-center gap-x-[22px] gap-y-1.5 font-mono text-[11px] leading-[1.4] text-muted">
                <span className="whitespace-nowrap">{rows.length} agents</span>
                {AGENT_PROVIDERS.map((p) => (
                  <span key={p} className="whitespace-nowrap">
                    {full(p)} {PROVIDER_LABEL[p]}
                  </span>
                ))}
                <button onClick={refresh} className="flex items-center gap-1.5 whitespace-nowrap hover:text-text" title="Scan again">
                  <RefreshCw className="size-3" />
                  rescan
                </button>
              </div>
              <Button variant="primary" onClick={() => go("agents", "new")}>
                <Plus className="size-3.5" />
                New agent
              </Button>
            </>
          }
        />

        <BuiltinAgents />

        {ours && "error" in ours && <p className="rounded-lg border border-bad/40 bg-bad/10 px-3 py-2 text-sm text-bad">Could not scan agents: {ours.error}</p>}
        {!ours && <Empty>Scanning agent folders…</Empty>}

        {data && (
          <>
            <section className="flex flex-col gap-3">
              <SectionTitle glyph="▦" note="✓ has a file · ! file has a problem · — none">
                Agent × CLI
              </SectionTitle>
              {rows.length ? (
                <AgentMatrix rows={rows} readers={data.readers} selected={target && target !== "new" ? agentArg(target.scope, target.name) : null} onSelect={open} />
              ) : (
                <div className="flex flex-col gap-2 rounded-xl border border-line bg-surface px-5 py-6 text-sm leading-[1.6] text-text-2">
                  <p className="text-text">No agents yet in this project or your home folder.</p>
                  <p>
                    An agent is a file: a name, a description the main agent reads to decide when to delegate, and a system prompt. <b className="font-semibold text-text">New agent</b> writes one file per
                    CLI you pick into the folders below (project agents inside the project&apos;s folder, user agents in your home folder); each CLI then finds it by name.
                  </p>
                </div>
              )}
            </section>

            {data.errors.length > 0 && (
              <section className="flex flex-col gap-3">
                <SectionTitle glyph="!" note="these name no agent, so no CLI can load them">
                  Unreadable files
                </SectionTitle>
                <div className="flex flex-col rounded-xl border border-bad/30 bg-bad/[.04]">
                  {data.errors.map((e) => (
                    <div key={e.path} className="grid grid-cols-[86px_minmax(0,1fr)] gap-3 border-b border-line px-4 py-2 font-mono text-[11.5px] leading-[1.5] last:border-b-0 sm:grid-cols-[86px_minmax(0,1fr)_minmax(0,1fr)]">
                      <span style={{ color: providerColor(e.provider) }}>{PROVIDER_LABEL[e.provider]}</span>
                      <span className="break-all text-text-2">{e.path}</span>
                      <span className="text-bad max-sm:col-start-2">{e.error}</span>
                    </div>
                  ))}
                </div>
              </section>
            )}

            <section className="flex flex-col gap-3">
              <SectionTitle glyph="▤" note="from [agents] in map.toml">
                Where the files go
              </SectionTitle>
              <Folders data={data} />
              <p className="font-mono text-[10.5px] leading-[1.6] text-muted">
                Capabilities map onto each CLI&apos;s tools; agy only gets tool names from a fixed list. Codex has no tool list: no shell switches its shell tool off, and read-only adds an
                instruction. Effort is dropped for agy. The editor shows what each format loses before you save.
              </p>
            </section>
          </>
        )}
      </Main>
      {target && <AgentEditor target={target} onClose={close} onSaved={saved} onDeleted={deleted} />}
    </>
  );
}
