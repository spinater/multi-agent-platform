"use client";

/** A project's manager (backend manager.rs): an always-on agent session the owner talks to about the project, here,
 * in its terminal or from Telegram (`/ask <slug> …`). It reads and changes the project through MAP's API with its
 * own token, which opens this project only. Below it, the "Open lane" form for persistent lanes in this folder. */

import { Bot, Plus } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { managerApi } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { LaneInfo } from "@/lib/generated/LaneInfo";
import type { ManagerContextInfo } from "@/lib/generated/ManagerContextInfo";
import type { ManagerInfo } from "@/lib/generated/ManagerInfo";
import { attachCommand } from "@/lib/tmux";

import { useApp } from "./app-context";
import { Btn, Chip, Pill, SideTitle, type Tone } from "./command-ui";
import { ConsoleView } from "./console";
import { TextInput, Toggle, errText } from "./project-ui";

const STATE_TONE: Record<string, Tone> = { off: "idle", starting: "info", running: "ok", restarting: "warn", stopped: "bad", failed: "bad" };

export function ManagerPanel({ slug }: { slug: string }) {
  const { sessions, providers, server, toast, go } = useApp();
  const [info, setInfo] = useState<ManagerInfo | null>(null);
  const [lanes, setLanes] = useState<LaneInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [pick, setPick] = useState<{ provider: string; model: string } | null>(null);
  const [ctx, setCtx] = useState<ManagerContextInfo | null>(null);

  const name = info?.session_name ?? `${server?.session_prefix ?? "map-"}mgr-${slug}`;
  const session = sessions.find((s) => s.name === name);
  const live = !!session && session.state !== "exited";
  // The session's state comes over the WebSocket; re-read the manager's own state when it changes.
  const sessionState = session?.state ?? "none";

  const load = useCallback(() => {
    managerApi.get(slug).then(setInfo, () => {});
    managerApi.lanes(slug).then(setLanes, () => {});
    managerApi.context(slug).then(setCtx, () => {});
  }, [slug]);
  useEffect(() => load(), [load, sessionState]);

  const usable = providers.filter((p) => p.available);
  const settings = info?.settings;
  const provider = pick?.provider ?? settings?.provider ?? info?.default_provider ?? "";
  const models = providers.find((p) => p.name === provider)?.models ?? [];
  const model = pick?.model ?? settings?.model ?? "";
  const dirty = pick !== null && (pick.provider !== (settings?.provider ?? info?.default_provider ?? "") || pick.model !== (settings?.model ?? ""));

  const save = async (enabled: boolean, force = false) => {
    setBusy(true);
    try {
      const next = await managerApi.set(slug, { enabled, provider: provider || null, model: model || null, force });
      setInfo(next);
      setPick(null);
      toast(enabled ? `Manager on · ${next.state}` : "Manager off");
    } catch (e) {
      const msg = errText(e);
      if (!enabled && !force && /attached|typed/.test(msg) && window.confirm(`${msg}\n\nTurn it off anyway?`)) {
        setBusy(false);
        return save(false, true);
      }
      toast(msg, "bad");
    } finally {
      setBusy(false);
    }
  };

  const startNow = async () => {
    setBusy(true);
    try {
      await managerApi.start(slug);
      toast("Manager started");
      load();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  const compact = async () => {
    setBusy(true);
    try {
      setCtx(await managerApi.compact(slug));
      toast("Manager compressing its conversation");
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  const state = info?.state ?? "off";
  return (
    <div className="grid min-w-0 gap-6 xl:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
      <section className="flex min-w-0 flex-col gap-3">
        <SideTitle glyph={<Bot className="size-3" />} note={<Pill tone={STATE_TONE[state] ?? "idle"}>{state}</Pill>}>
          Manager
        </SideTitle>
        <p className="text-[12.5px] leading-[1.55] text-pretty text-text-2">
          An always-on agent for this project. Ask it how things stand, or tell it to create, cancel or retry tasks, approve plans, pause the project or open a lane, here or from Telegram
          with <span className="font-mono text-text">/ask {slug} …</span>. It changes things only when you ask, and its token opens this project only.
        </p>
        <div className="flex flex-wrap items-center gap-x-5 gap-y-2.5">
          <label className="flex items-center gap-2">
            <Toggle on={!!settings?.enabled} disabled={busy || !info} label="Manager on" onClick={() => void save(!settings?.enabled)} />
            <span className="text-[12.5px] font-semibold text-text">{settings?.enabled ? "On · kept running" : "Off"}</span>
          </label>
          {info?.resumes && <span className="font-mono text-[10.5px] text-muted">restarts resume its conversation</span>}
          {live && ctx && (
            <span className="flex items-center gap-2 font-mono text-[10.5px] text-muted" title={ctx.blocked ?? `Compressed by itself past ${ctx.limit.toLocaleString()} tokens`}>
              context {ctx.tokens == null ? "unknown" : `${Math.round(ctx.tokens / 1000)}k`} / {Math.round(ctx.limit / 1000)}k tokens
              <Btn size="sm" disabled={busy || !!ctx.blocked} onClick={() => void compact()}>
                Compress
              </Btn>
            </span>
          )}
          {(state === "stopped" || state === "failed" || state === "restarting") && (
            <Btn size="sm" kind="primary" disabled={busy} onClick={() => void startNow()}>
              Start now
            </Btn>
          )}
        </div>
        {info?.warning && <p className="rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 font-mono text-[11.5px] leading-normal text-warn">{info.warning}</p>}
        <div className="flex flex-col gap-2">
          <span className="font-mono text-[10.5px] font-medium tracking-[0.08em] text-muted uppercase">Provider{info?.settings.provider ? "" : " · default"}</span>
          <div className="flex flex-wrap gap-1.5">
            {usable.map((p) => (
              <Chip key={p.name} on={provider === p.name} onClick={() => setPick({ provider: p.name, model: "" })}>
                <span className="size-1.5 rounded-full" style={{ background: providerColor(p.name) }} />
                {p.name}
                {p.signed_in === false ? " · signed out" : ""}
              </Chip>
            ))}
            {models.length > 0 && (
              <select
                value={model}
                onChange={(e) => setPick({ provider, model: e.target.value })}
                aria-label="Model"
                className="h-7 rounded-md border border-line-2 bg-inset px-2 font-mono text-[11.5px] text-text"
              >
                <option value="">default model</option>
                {models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            )}
            {dirty && (
              <Btn size="sm" kind="primary" disabled={busy} onClick={() => void save(!!settings?.enabled)}>
                Save
              </Btn>
            )}
          </div>
        </div>
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
              <span className="hidden sm:inline">{attachCommand(server?.tmux_socket, session.name)}</span>
            </div>
            <ConsoleView session={session} readOnly={false} toast={toast} className="h-[min(58vh,540px)] min-h-[300px]" />
          </>
        ) : (
          session && <ConsoleView session={session} readOnly toast={toast} className="h-[180px]" />
        )}
      </section>
      <section className="flex min-w-0 flex-col gap-3">
        <SideTitle glyph={<Plus className="size-3" />} note={`${lanes.length} lane${lanes.length === 1 ? "" : "s"}`}>
          Lanes
        </SideTitle>
        <OpenLaneForm slug={slug} onDone={load} />
        {lanes.length > 0 && (
          <ul className="flex min-w-0 flex-col gap-1.5">
            {lanes.map((l) => (
              <li key={l.name} className="flex min-w-0 items-center gap-2 font-mono text-[11.5px]">
                <button type="button" className="min-w-0 truncate text-link hover:underline" onClick={() => go("lanes", l.name)}>
                  {l.label}
                </button>
                <span className="text-muted">{l.role ?? l.owned ?? "watched"}</span>
                <span className={l.stopped ? "text-bad" : "text-text-2"}>{l.stopped ? "stopped" : (l.sessions[0]?.state.replace("_", " ") ?? "not checked yet")}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

function OpenLaneForm({ slug, onDone }: { slug: string; onDone: () => void }) {
  const { providers, toast } = useApp();
  const [name, setName] = useState("");
  const [provider, setProvider] = useState("");
  const [prompt, setPrompt] = useState("");
  const [statusPrompt, setStatusPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const usable = providers.filter((p) => p.available && p.signed_in !== false);

  const open = async () => {
    setBusy(true);
    try {
      const r = await managerApi.openLane(slug, {
        name: name.trim(),
        provider: provider || null,
        model: null,
        prompt: prompt.trim() || null,
        clear_when: null,
        status_prompt: statusPrompt.trim() || null,
        role: null,
      });
      toast(`Lane ${r.lane} opened on ${r.provider}`);
      setName("");
      setPrompt("");
      setStatusPrompt("");
      onDone();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2.5 rounded-[10px] border border-line bg-inset p-3.5">
      <p className="text-[12px] leading-normal text-text-2">
        A persistent agent session in this folder that MAP watches like a lane (rate-limit wake, stall alerts, Telegram). It survives MAP restarts; if its session ends it shows as stopped
        with a Start button.
      </p>
      <div className="flex flex-wrap gap-2">
        <TextInput value={name} onChange={(e) => setName(e.target.value)} placeholder="name (letters, digits, -)" aria-label="Lane name" className="max-w-[220px]" />
        <select value={provider} onChange={(e) => setProvider(e.target.value)} aria-label="Provider" className="h-[38px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[12px] text-text">
          <option value="">default provider</option>
          {usable.map((p) => (
            <option key={p.name} value={p.name}>
              {p.name}
            </option>
          ))}
        </select>
      </div>
      <TextInput value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="first prompt (optional)" aria-label="First prompt" />
      <TextInput value={statusPrompt} onChange={(e) => setStatusPrompt(e.target.value)} placeholder="status prompt when idle (optional)" aria-label="Status prompt" />
      <Btn kind="primary" className="self-start" disabled={busy || !/^[A-Za-z0-9][A-Za-z0-9-]{0,23}$/.test(name.trim())} onClick={() => void open()}>
        {busy ? "Opening…" : "Open lane"}
      </Btn>
    </div>
  );
}
