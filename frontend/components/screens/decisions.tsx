"use client";

import { useMemo, useState } from "react";

import { api, decisionsApi } from "@/lib/api";
import type { DecisionNode } from "@/lib/generated/DecisionNode";
import type { Task } from "@/lib/generated/Task";

import { useApp } from "../app-context";
import { DecisionGraph, type Selection, type Token } from "../decisions/graph";
import { CHIPS, ORDER, attention, edgeKey, nodeOf, nowChecks, providerOf } from "../decisions/model";
import { NodeChips, NodePanel, TaskPanel, TaskRows, type Row } from "../decisions/panels";
import { waitText } from "../parallel";
import { useDecisions, useTrace } from "../decisions/use-decisions";
import { Main, PageHeader } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";
import { Empty } from "../ui";

/** The orchestrator's decision model, animated live (design: Decisions.dc.html). Data: `decision.*` events. */
export function DecisionsScreen() {
  const app = useApp();
  const { tasks, sessions, orchestrator, server, inProject, current, now, toast, go } = app;
  const { model, kinds, counts, latest, pulses, error } = useDecisions();
  // `#decisions/<task id>` opens with that task selected.
  const [sel, setSel] = useState<Selection | null>(() => (app.route.arg && /^\d+$/.test(app.route.arg) ? { type: "task", id: Number(app.route.arg) } : null));
  const [filter, setFilter] = useState<DecisionNode | null>(null);
  const [busy, setBusy] = useState(false);

  const projectName = current?.project.name ?? workspaceName(server?.workspace_dir);
  const running = orchestrator?.running ?? false;

  // Every task of this project with its place in the graph.
  const placed = useMemo(() => {
    const out: (Row & { assigned: boolean; attention: string | null })[] = [];
    for (const task of tasks) {
      if (!inProject(task.project)) continue;
      const last = latest.get(task.id);
      const node = nodeOf(task, last, kinds);
      if (!node) continue;
      const p = providerOf(task, sessions);
      const s = sessions.find((x) => x.name === task.session);
      const sub =
        node === "watch"
          ? `${task.session ?? ""} · ${(s?.state ?? "working").replace("_", " ")}`
          : node === "ready"
            ? (waitText(last) ?? (task.depends_on.length
              ? `waiting on ${task.depends_on.filter((d) => tasks.find((t) => t.id === d)?.status !== "done").map((d) => `#${d}`).join(", ") || "a worker slot"}`
              : "no dependencies · next tick"))
            : node === "pick"
              ? `${task.provider ? `pinned ${task.provider}` : "provider_order"} · not available`
              : node === "blocked"
                ? (task.result ?? "dependency failed")
                : node === "failed"
                  ? `attempts ${task.attempts}/${model?.max_attempts ?? "?"}`
                  : `${task.session ?? ""}${p.name ? ` · ${p.name}` : ""}`;
      out.push({ task, node, provider: p.name, color: p.color, sub, assigned: p.assigned, attention: attention(task, node, sessions, last) });
    }
    return out;
  }, [tasks, sessions, latest, kinds, inProject, model]);

  const nodeOfTask = (t: Task) => nodeOf(t, latest.get(t.id), kinds);
  const label = (n: DecisionNode) => model?.nodes.find((x) => x.id === n)?.label ?? n;
  const rank = (n: DecisionNode) => ORDER.indexOf(n);
  const rows = placed.filter((r) => !filter || r.node === filter).sort((a, b) => rank(b.node) - rank(a.node) || a.task.id - b.task.id);

  // Default selection: a task the orchestrator is watching, else the first task, else the Ready node.
  const fallback: Selection = (() => {
    const t = placed.find((r) => r.node === "watch") ?? placed[0];
    return t ? { type: "task", id: t.task.id } : { type: "node", id: "ready" };
  })();
  const selection: Selection = sel?.type === "task" && !placed.some((r) => r.task.id === sel.id) ? fallback : (sel ?? fallback);
  const selRow = selection.type === "task" ? placed.find((r) => r.task.id === selection.id) : undefined;
  const trace = useTrace(selRow?.task.id ?? null);

  const path = useMemo(() => {
    const keys = new Set<string>();
    for (const e of trace) {
      const k = kinds.get(e.kind);
      if (k && k.from !== k.to) keys.add(edgeKey(k.from, k.to));
    }
    return keys;
  }, [trace, kinds]);

  const tokens: Token[] = placed.map((r) => ({ id: r.task.id, node: r.node, color: r.color, assigned: r.assigned, attention: !!r.attention }));
  const chips = CHIPS.map((n) => ({ node: n, label: label(n), n: placed.filter((r) => r.node === n).length }));

  const toggle = async () => {
    setBusy(true);
    try {
      await api.setOrchestrator(!running);
      toast(running ? "Orchestrator paused" : "Orchestrator running");
    } catch (e) {
      toast((e as Error).message, "bad");
    } finally {
      setBusy(false);
    }
  };

  const step = async () => {
    setBusy(true);
    try {
      const r = await decisionsApi.step();
      const mine = r.decisions.filter((e) => placed.some((p) => p.task.id === e.task_id));
      toast(mine.length ? `Stepped one tick: ${mine.length} decision${mine.length === 1 ? "" : "s"}` : "Stepped one tick: no decision to make", mine.length ? "ok" : "warn");
    } catch (e) {
      toast((e as Error).message, "bad");
    } finally {
      setBusy(false);
    }
  };

  const tick = server?.watcher_interval_secs ?? model?.tick_secs ?? 5;
  const live = running ? "var(--ok)" : "var(--idle)";

  return (
    <>
      <TopBar />
      <Main>
        <PageHeader
          kicker={`Project · ${projectName} · orchestrator.rs`}
          title="Decision model"
          lead="Every task passes through the same checks, from readiness to the end decision. Each one shows where its task is now, what the orchestrator checked, and which way it went."
          actions={
            <>
              <span className="flex items-center gap-[7px] font-mono text-[10.5px] font-medium tracking-[0.1em] whitespace-nowrap uppercase" style={{ color: live }}>
                <span className={running ? "size-[7px] animate-pulse rounded-full" : "size-[7px] rounded-full"} style={{ background: live }} />
                {running ? `live · tick ${tick}s` : "paused"}
              </span>
              <button
                onClick={toggle}
                disabled={busy || !orchestrator}
                className="h-8 rounded-[7px] border border-line-3 px-[13px] text-xs font-semibold whitespace-nowrap text-text hover:border-muted disabled:opacity-50"
              >
                {running ? "❚❚ Pause" : "▶ Run"}
              </button>
              <button
                onClick={step}
                disabled={busy || running}
                title={running ? "Pause the orchestrator to step one decision at a time" : "Run one orchestrator tick"}
                className="h-8 rounded-[7px] border border-accent bg-accent px-[13px] text-xs font-semibold whitespace-nowrap text-accent-text hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
              >
                Step one decision →
              </button>
            </>
          }
        />

        <NodeChips
          chips={chips}
          filter={filter}
          onPick={(n) => {
            setFilter(filter === n ? null : n);
            setSel({ type: "node", id: n });
          }}
        />

        {error && <p className="rounded-[10px] border border-bad/40 bg-bad/10 px-4 py-3 text-[13px] text-bad">Could not load the decision model: {error}</p>}

        {!placed.length && (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-accent/35 bg-accent/5 px-4 py-3.5">
            <span className="text-[13px] leading-normal text-text-2">No tasks for {projectName} yet. Tasks you add in its command centre show up here with their decisions.</span>
            <button onClick={() => go("command")} className="flex h-8 items-center rounded-[7px] bg-accent px-[13px] text-xs font-semibold whitespace-nowrap text-accent-text">
              Open command centre →
            </button>
          </div>
        )}

        <div className="overflow-x-auto rounded-[14px] border border-line bg-inset bg-[radial-gradient(var(--line)_1px,transparent_1px)] bg-size-[22px_22px]">
          {model ? (
            <DecisionGraph
              model={model}
              counts={counts}
              tokens={tokens}
              pulses={pulses}
              selection={selection}
              path={selRow ? path : new Set()}
              onSelect={setSel}
              renderHover={(n) => {
                const point = model.nodes.find((x) => x.id === n)!;
                const here = placed.filter((r) => r.node === n);
                return (
                  <div className="flex flex-col gap-1.5">
                    <span className="font-mono text-[10px] tracking-[0.14em] text-muted uppercase">{point.source}</span>
                    <span className="text-[12.5px] font-semibold">{point.question}</span>
                    <span className="font-mono text-[11px] leading-snug text-text-2">{point.rule}</span>
                    <span className="font-mono text-[10.5px] text-muted">
                      {here.length ? `here: ${here.map((r) => `#${r.task.id}`).join(" ")}` : "no tasks here"} · click for details
                    </span>
                  </div>
                );
              }}
            />
          ) : (
            <div className="flex h-[470px] items-center justify-center">
              <Empty>{error ? "Decision model unavailable." : "Loading the decision model…"}</Empty>
            </div>
          )}
        </div>

        <div className="flex flex-wrap items-start gap-5">
          <TaskRows
            rows={rows}
            label={filter ? `only ${label(filter)} · click again to clear` : `${placed.length} tasks`}
            selected={selRow?.task.id ?? null}
            nodeLabel={label}
            onSelect={(id) => setSel({ type: "task", id })}
          />
          {model &&
            (selRow ? (
              <TaskPanel
                task={selRow.task}
                node={selRow.node}
                provider={selRow.provider}
                color={selRow.color}
                model={model}
                kinds={kinds}
                trace={trace}
                attempt={Math.max(1, selRow.task.attempts)}
                checks={nowChecks({ task: selRow.task, node: selRow.node, trace, tasks, sessions, model, active: orchestrator?.active ?? 0, now, nodeOfTask })}
                onSelect={setSel}
              />
            ) : (
              <NodePanel node={selection.type === "node" ? selection.id : "ready"} model={model} counts={counts} rows={placed.filter((r) => r.node === (selection.type === "node" ? selection.id : "ready"))} onSelect={setSel} />
            ))}
        </div>
      </Main>
    </>
  );
}
