"use client";

/** Configure › Hooks (backend hooks.rs): commands MAP runs around a task, grouped by event. Each row: enable switch,
 * editable command, blocking toggle (where the event can block), timeout, source, Test (runs it now in the project
 * folder) and delete. map.toml's `[orchestrator] post_done` commands show read only. A machine hook edited in project
 * scope becomes this project's override (same id). Drafts until Save. */

import { useEffect, useState } from "react";

import { hooksApi, type ConfigOp } from "@/lib/api";
import type { HookEvent } from "@/lib/generated/HookEvent";
import type { HookItem } from "@/lib/generated/HookItem";
import type { HookRun } from "@/lib/generated/HookRun";
import type { HooksOverview } from "@/lib/generated/HooksOverview";

import { AskAgent } from "./ask-agent";
import { Note, SourceBadge, Switch, TabHead, btnCls, inputCls, same, useConfigure } from "./ui";

let seq = 0;
const fmtTimeout = (s: number) => (s >= 60 && s % 60 === 0 ? `${s / 60}m` : `${s}s`);
const parseTimeout = (t: string) => {
  const m = /^(\d+)\s*(s|m)?$/.exec(t.trim());
  return m ? Number(m[1]) * (m[2] === "m" ? 60 : 1) : null;
};

export function HooksTab() {
  const { scope, slug, setPending, flash } = useConfigure();
  const [data, setData] = useState<HooksOverview | null>(null);
  const [hooks, setHooks] = useState<HookItem[] | null>(null);
  const [runs, setRuns] = useState<Record<string, HookRun | "running">>({});
  const [newEvent, setNewEvent] = useState<HookEvent>("post_done");
  const [newCmd, setNewCmd] = useState("");
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    hooksApi.list(slug).then(
      (d) => {
        if (!live) return;
        setData(d);
        setHooks(d.hooks);
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [slug, tick]);

  const mine = (h: HookItem) => !h.read_only && (scope === "machine" ? h.source === "machine" : h.source !== "machine");
  const sig = (hs: HookItem[] | null) => JSON.stringify((hs ?? []).filter(mine).map(({ id, event, command, blocking, timeout_secs, enabled }) => ({ id, event, command, blocking, timeout_secs, enabled })));
  const dirty = !!hooks && !!data && sig(hooks) !== sig(data.hooks);
  const count = dirty ? Math.max(1, (hooks ?? []).filter((h) => mine(h) && !same(h, data?.hooks.find((x) => x.id === h.id))).length + Math.max(0, (data?.hooks.filter(mine).length ?? 0) - (hooks ?? []).filter(mine).length)) : 0;

  useEffect(() => {
    setPending(
      "hooks",
      dirty
        ? {
            count,
            files: [scope === "machine" ? "machine settings (hooks)" : "project settings (hooks)"],
            save: async () => {
              const d = await hooksApi.save(
                slug,
                (hooks ?? []).filter(mine).map(({ id, event, command, blocking, timeout_secs, enabled }) => ({ id: id.startsWith("new:") ? "" : id, event, command, blocking, timeout_secs, enabled })),
              );
              setData(d);
              setHooks(d.hooks);
            },
            discard: () => setTick((t) => t + 1),
          }
        : null,
    );
  }, [sig(hooks), dirty, slug]); // eslint-disable-line react-hooks/exhaustive-deps

  const upd = (id: string, p: Partial<HookItem>) =>
    setHooks((hs) => (hs ?? []).map((h) => (h.id === id ? { ...h, ...p, source: scope === "project" && h.source === "machine" ? "override" : h.source } : h)));
  const add = (event: HookEvent, command: string, blocking?: boolean, timeout = 60) => {
    const canBlock = data?.events.find((e) => e.event === event)?.can_block ?? false;
    setHooks((hs) => [...(hs ?? []), { id: `new:${++seq}`, event, command, blocking: canBlock && (blocking ?? event !== "on_fail"), timeout_secs: timeout, enabled: true, source: scope === "machine" ? "machine" : "project", read_only: false }]);
  };
  const test = async (h: HookItem) => {
    setRuns((r) => ({ ...r, [h.id]: "running" }));
    try {
      const r = await hooksApi.test(slug, h.command, h.timeout_secs);
      setRuns((x) => ({ ...x, [h.id]: r }));
    } catch (e) {
      setRuns((x) => {
        const n = { ...x };
        delete n[h.id];
        return n;
      });
      flash((e as Error).message, "bad");
    }
  };
  const applyOps = (ops: ConfigOp[]) => {
    let n = 0;
    for (const o of ops) {
      const event = o.target.replace(/^hook:/, "") as HookEvent;
      if (!data?.events.some((e) => e.event === event)) continue;
      const v = (o.value ?? {}) as { command?: string; blocking?: boolean; timeout_secs?: number; enabled?: boolean; set?: Partial<HookItem> };
      const target = (hooks ?? []).find((h) => h.event === event && h.command === v.command && !h.read_only);
      if (o.op === "add" && v.command) {
        add(event, v.command, v.blocking, v.timeout_secs ?? 60);
        n++;
      } else if (o.op === "change" && target && v.set) {
        upd(target.id, v.set);
        n++;
      } else if (o.op === "remove" && target && mine(target)) {
        setHooks((hs) => (hs ?? []).filter((h) => h.id !== target.id));
        n++;
      }
    }
    return n;
  };

  return (
    <>
      <TabHead
        title="Hooks"
        desc="Commands MAP runs around a task. A blocking hook that fails stops the task and waits for a person."
        savesTo={scope === "machine" ? "machine settings in the platform database (hooks) · map.toml [orchestrator] post_done (read only)" : "project settings (hooks) · post_done = [orchestrator] post_done"}
      />
      <AskAgent tab="hooks" context={{ hooks: (hooks ?? []).map(({ id, event, command, blocking, timeout_secs, enabled, source }) => ({ id, event, command, blocking, timeout_secs, enabled, source })), events: data?.events }} onApply={applyOps} />
      {error && <Note tone="bad">Could not read the hooks: {error}</Note>}
      {!data && !error && <Note>Reading the hooks…</Note>}
      {data && hooks && (
        <div className="flex flex-col gap-4">
          {data.events.map((ev) => {
            const rows = hooks.filter((h) => h.event === ev.event);
            return (
              <div key={ev.event} className="flex flex-col gap-2">
                <div className="flex flex-wrap items-baseline gap-2.5">
                  <span className="font-mono text-[13px] leading-none font-semibold text-warn">⚑ {ev.event}</span>
                  <span className="text-[11.5px] leading-[1.3] text-muted">{ev.description}</span>
                </div>
                {rows.map((h) => {
                  const ro = h.read_only || (scope === "machine" && h.source !== "machine");
                  const run = runs[h.id];
                  return (
                    <div key={h.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-[9px] border border-line bg-surface px-3 py-2.5" style={{ opacity: h.enabled ? 1 : 0.55 }}>
                      <Switch on={h.enabled} disabled={ro} title="Enable / disable" onClick={() => upd(h.id, { enabled: !h.enabled })} />
                      <input value={h.command} readOnly={ro} onChange={(e) => upd(h.id, { command: e.target.value })} className={`${inputCls} h-[30px] flex-[1_1_260px]`} />
                      <button
                        disabled={ro || !ev.can_block}
                        onClick={() => upd(h.id, { blocking: !h.blocking })}
                        title={ev.can_block ? undefined : "this event never stops a task"}
                        className="h-[26px] cursor-pointer rounded-[5px] border px-2 font-mono text-[10.5px] font-medium whitespace-nowrap disabled:cursor-default"
                        style={{ borderColor: h.blocking ? "color-mix(in srgb, var(--warn) 40%, transparent)" : "var(--line-3)", color: h.blocking ? "var(--warn)" : "var(--text-2)" }}
                      >
                        {h.blocking ? "blocking" : "non-blocking"}
                      </button>
                      <label className="flex items-center gap-1 font-mono text-[10.5px] whitespace-nowrap text-muted">
                        timeout
                        <input
                          defaultValue={fmtTimeout(h.timeout_secs)}
                          readOnly={ro}
                          onBlur={(e) => {
                            const t = parseTimeout(e.target.value);
                            if (t && t >= 1 && t <= 3600) upd(h.id, { timeout_secs: t });
                            else e.target.value = fmtTimeout(h.timeout_secs);
                          }}
                          className="h-6 w-12 rounded border border-line-2 bg-inset px-1 text-center font-mono text-[10.5px] text-text outline-none"
                        />
                      </label>
                      <SourceBadge source={h.read_only ? "read_only" : h.source} />
                      <button onClick={() => test(h)} disabled={run === "running" || !h.command.trim()} className={`${btnCls} h-[26px] px-2.5 text-[11px]`}>
                        {run === "running" ? "Running…" : "Test"}
                      </button>
                      {!ro && mine(h) ? (
                        <button onClick={() => setHooks(hooks.filter((x) => x.id !== h.id))} title="Delete" className="size-[26px] cursor-pointer rounded-[5px] text-muted hover:text-bad">
                          ×
                        </button>
                      ) : (
                        <span className="w-[26px]" />
                      )}
                      {run && run !== "running" && (
                        <span className="basis-full font-mono text-[11px] leading-[1.4] whitespace-pre-wrap" style={{ color: run.exit_code === 0 && !run.timed_out ? "#7ee2a8" : "var(--bad)" }}>
                          {run.timed_out ? "timed out" : `exit ${run.exit_code ?? "?"}`} · {(run.duration_ms / 1000).toFixed(1)}s{run.output_tail ? `\n${run.output_tail}` : ""}
                        </span>
                      )}
                    </div>
                  );
                })}
                {!rows.length && <span className="rounded-[9px] border border-dashed border-line-2 px-3 py-2.5 font-mono text-[11.5px] leading-none text-muted/80">no hooks</span>}
              </div>
            );
          })}
          <div className="flex flex-wrap gap-2 rounded-[10px] border border-dashed border-line-3 p-3">
            <select value={newEvent} onChange={(e) => setNewEvent(e.target.value as HookEvent)} className={`${inputCls} font-medium`}>
              {data.events.map((e) => (
                <option key={e.event}>{e.event}</option>
              ))}
            </select>
            <input value={newCmd} onChange={(e) => setNewCmd(e.target.value)} placeholder="command, e.g. bun run lint" className={`${inputCls} flex-[1_1_260px]`} />
            <button
              onClick={() => {
                if (!newCmd.trim()) {
                  flash("Type a command first", "warn");
                  return;
                }
                add(newEvent, newCmd.trim());
                setNewCmd("");
              }}
              className={`${btnCls} h-8 text-text`}
            >
              + Add hook
            </button>
          </div>
        </div>
      )}
    </>
  );
}
