"use client";

/** A project adds its own lanes: a role (manager, developer or another word), a name (default `<role>-<slug>`), and
 * either an existing tmux session to watch (picked from the server's live list; MAP only reads it until actions are
 * turned on) or a new MAP session (a persistent lane on MAP's own tmux server). Also the project's default slots. */

import clsx from "clsx";
import { Plus, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { lanesApi, managerApi } from "@/lib/api";
import type { LaneInfo } from "@/lib/generated/LaneInfo";
import type { TmuxSessionInfo } from "@/lib/generated/TmuxSessionInfo";

import { useApp } from "../app-context";
import { Lbl, RadioCard, Seg, inputCls } from "../configure/ui";
import { Btn, errText } from "../project-ui";

/** The standard pair every project has a slot for. */
export const STANDARD_ROLES = ["manager", "developer"] as const;

const WATCH_NAME = /^[a-z0-9._-]{1,48}$/;
const MAP_NAME = /^[A-Za-z0-9][A-Za-z0-9-]{0,23}$/;
const ROLE = /^[a-z][a-z0-9-]{0,23}$/;

type Mode = "watch" | "map";

export function AddLane({ slug, role: initialRole = "developer", onDone, onCancel }: { slug: string; role?: string; onDone: () => void; onCancel?: () => void }) {
  const { providers, toast } = useApp();
  const standard = (STANDARD_ROLES as readonly string[]).includes(initialRole);
  const [roleKind, setRoleKind] = useState<string>(standard ? initialRole : "other");
  const [other, setOther] = useState(standard ? "" : initialRole);
  const role = (roleKind === "other" ? other : roleKind).trim().toLowerCase();
  // The name follows the role until it is edited.
  const [customName, setCustomName] = useState<string | null>(null);
  const name = customName ?? `${role || "role"}-${slug}`;
  const [mode, setMode] = useState<Mode>("watch");
  const [socket, setSocket] = useState("");
  const [sessions, setSessions] = useState<TmuxSessionInfo[] | null>(null);
  const [listError, setListError] = useState("");
  const [session, setSession] = useState("");
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const usable = providers.filter((p) => p.available && p.signed_in !== false);
  const models = providers.find((p) => p.name === provider)?.models ?? [];

  const list = useCallback(() => {
    setListError("");
    lanesApi.tmuxSessions(socket.trim()).then(
      (s) => setSessions(s),
      (e) => {
        setSessions([]);
        setListError(errText(e));
      },
    );
  }, [socket]);

  useEffect(() => {
    if (mode !== "watch") return;
    const t = window.setTimeout(list, 250);
    return () => window.clearTimeout(t);
  }, [mode, list]);

  const nameOk = mode === "watch" ? WATCH_NAME.test(name.trim()) : MAP_NAME.test(name.trim());
  const ready = ROLE.test(role) && nameOk && (mode === "map" || session !== "");

  const submit = async () => {
    setBusy(true);
    try {
      if (mode === "watch") {
        const l = await lanesApi.watch(slug, { name: name.trim(), role, session, socket: socket.trim() || null });
        toast(`Lane ${l.name} added · watching ${session} (read only until actions are on)`);
      } else {
        const r = await managerApi.openLane(slug, { name: name.trim(), role, provider: provider || null, model: model || null, prompt: prompt.trim() || null, clear_when: null, status_prompt: null });
        toast(`Lane ${r.name} started on ${r.provider}`);
      }
      onDone();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-3.5 rounded-[10px] border border-line bg-inset p-3.5">
      <div className="flex flex-wrap items-end gap-4">
        <label className="flex flex-col gap-1.5">
          <Lbl>Role</Lbl>
          <div className="flex flex-wrap items-center gap-2">
            <Seg value={roleKind} options={["manager", "developer", "other"]} onChange={setRoleKind} size="md" />
            {roleKind === "other" && (
              <input value={other} onChange={(e) => setOther(e.target.value)} placeholder="e.g. reviewer" aria-label="Other role" className={clsx(inputCls, "w-[140px]", other && !ROLE.test(role) && "border-bad")} />
            )}
          </div>
        </label>
        <label className="flex min-w-0 flex-1 flex-col gap-1.5">
          <Lbl>Name</Lbl>
          <input
            value={name}
            onChange={(e) => setCustomName(e.target.value)}
            aria-label="Lane name"
            className={clsx(inputCls, "w-full max-w-[320px]", name && !nameOk && "border-bad")}
          />
        </label>
      </div>

      <div className="grid gap-2 sm:grid-cols-2">
        <RadioCard on={mode === "watch"} onClick={() => setMode("watch")} title="Watch an existing session" desc="A tmux session that already runs (yours). MAP reads it and reports; it never creates, kills or renames it." />
        <RadioCard on={mode === "map"} onClick={() => setMode("map")} title="Start a new MAP session" desc="An agent session MAP starts in the project folder on its own tmux server, kept as a persistent lane." />
      </div>

      {mode === "watch" ? (
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap items-end gap-2">
            <label className="flex flex-col gap-1.5">
              <Lbl>tmux socket</Lbl>
              <input value={socket} onChange={(e) => setSocket(e.target.value)} placeholder="default server" aria-label="tmux socket" className={clsx(inputCls, "w-[180px]")} />
            </label>
            <Btn size="sm" onClick={list} aria-label="Reload the session list">
              <RefreshCw className="size-3" />
              Reload
            </Btn>
          </div>
          <Lbl>Session</Lbl>
          {listError && <p className="font-mono text-[11.5px] text-bad">{listError}</p>}
          {sessions === null ? (
            <p className="font-mono text-[11.5px] text-muted">listing…</p>
          ) : sessions.length === 0 ? (
            <p className="font-mono text-[11.5px] text-muted">No sessions on this server (MAP&apos;s own are not listed).</p>
          ) : (
            <ul className="flex max-h-[220px] min-w-0 flex-col gap-1 overflow-y-auto">
              {sessions.map((s) => {
                const taken = s.lane !== null;
                const on = session === s.name;
                return (
                  <li key={s.name}>
                    <button
                      type="button"
                      disabled={taken}
                      onClick={() => setSession(s.name)}
                      aria-pressed={on}
                      className={clsx(
                        "flex w-full min-w-0 items-center gap-2 rounded-md border px-2.5 py-1.5 text-left font-mono text-[12px] disabled:cursor-default disabled:opacity-55",
                        on ? "border-accent text-text" : "border-line-2 text-text-2 hover:text-text",
                      )}
                    >
                      <span className="min-w-0 flex-1 truncate">{s.name}</span>
                      {s.attached && <span className="text-[10.5px] text-info">attached</span>}
                      {taken && <span className="text-[10.5px] text-muted">watched by {s.lane}</span>}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          <p className="font-mono text-[10.5px] leading-snug text-muted">Starts read only (actions off): turn actions on in the lane card to let MAP type into it when idle and nobody is attached.</p>
        </div>
      ) : (
        <div className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap gap-2">
            <select
              value={provider}
              onChange={(e) => {
                setProvider(e.target.value);
                setModel("");
              }}
              aria-label="Provider"
              className={inputCls}
            >
              <option value="">default provider</option>
              {usable.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
            <select value={model} onChange={(e) => setModel(e.target.value)} aria-label="Model" className={inputCls} disabled={!provider}>
              <option value="">default model</option>
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </div>
          <input value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="first prompt (optional)" aria-label="First prompt" className={clsx(inputCls, "w-full")} />
          <p className="font-mono text-[10.5px] leading-snug text-muted">A MAP session name is 1-24 letters, digits or -; its tmux session is named after the project and this name.</p>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Btn tone="primary" size="sm" disabled={busy || !ready} onClick={() => void submit()}>
          {busy ? "Adding…" : mode === "watch" ? "Watch session" : "Start session"}
        </Btn>
        {onCancel && (
          <Btn size="sm" tone="quiet" onClick={onCancel}>
            Cancel
          </Btn>
        )}
      </div>
    </div>
  );
}

/** The project's header on the Lanes screen: its name, "+ Add lane", the open form, and a slot for each standard role
 * no lane has yet (with "Set up"). */
export function ProjectLanesHead({ slug, title, lanes, onDone }: { slug: string; title: string; lanes: LaneInfo[]; onDone: () => void }) {
  const [adding, setAdding] = useState<string | null>(null);
  const missing = STANDARD_ROLES.filter((r) => !lanes.some((l) => l.role === r));
  const done = () => {
    setAdding(null);
    onDone();
  };
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-line pb-2">
        <h2 className="flex items-baseline gap-2 text-[15px] font-semibold text-text">
          {title}
          <span className="font-mono text-[11px] font-normal text-muted">
            {slug} · {lanes.length} lane{lanes.length === 1 ? "" : "s"}
          </span>
        </h2>
        <Btn size="sm" onClick={() => setAdding(adding === null ? "developer" : null)} aria-expanded={adding !== null}>
          <Plus className="size-3" />
          Add lane
        </Btn>
      </div>
      {adding !== null && <AddLane key={adding} slug={slug} role={adding} onDone={done} onCancel={() => setAdding(null)} />}
      {missing.length > 0 && adding === null && (
        <div className="grid min-w-0 gap-2 sm:grid-cols-2">
          {missing.map((r) => (
            <div key={r} className="flex min-w-0 items-center justify-between gap-2 rounded-[10px] border border-dashed border-line-3 px-3.5 py-3">
              <span className="flex min-w-0 flex-col gap-0.5">
                <span className="font-mono text-[10px] tracking-[0.12em] text-muted uppercase">{r}</span>
                <span className="truncate font-mono text-[12.5px] text-text-2">
                  {r}-{slug} · not set up
                </span>
              </span>
              <Btn size="sm" tone="primary" onClick={() => setAdding(r)}>
                Set up
              </Btn>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
