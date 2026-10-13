"use client";

import clsx from "clsx";
import { useState } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Session } from "@/lib/generated/Session";

import type { ProviderState } from "./command-data";
import { Btn, Chip, Pill, SESSION_TONE, rel } from "./command-ui";
import { Modal } from "./shell/page";

const ORDER: Record<string, number> = { error: 0, needs_auth: 0, waiting_input: 0, stalled: 1, rate_limited: 1, working: 2, starting: 2, idle: 3, exited: 4 };

/** Sessions of the right column, the ones needing you first (design: `sessionRows`). */
export function SessionRows({
  sessions,
  selected,
  onSelect,
  now,
  color = (s) => providerColor(s.provider),
  prefix,
}: {
  sessions: Session[];
  selected: string | null;
  onSelect: (id: string) => void;
  now: number;
  color?: (s: Session) => string;
  /** All projects: the project name before the meta line. */
  prefix?: (s: Session) => string;
}) {
  if (sessions.length === 0) return <p className="font-mono text-[11px] text-muted">No sessions. Start one, or add a task and run the orchestrator.</p>;
  const sorted = [...sessions].sort((a, b) => (ORDER[a.state] ?? 5) - (ORDER[b.state] ?? 5) || b.last_activity_at.localeCompare(a.last_activity_at));
  return (
    <div className="flex flex-col">
      {sorted.map((s) => (
        <button
          key={s.name}
          onClick={() => onSelect(`s:${s.name}`)}
          className={clsx(
            "-mx-2.5 flex cursor-pointer flex-col gap-1 rounded-[7px] px-2.5 py-[9px] text-left text-text",
            selected === `s:${s.name}` ? "bg-accent/[0.08]" : "hover:bg-white/[0.03]",
            s.state === "exited" && "opacity-50",
          )}
        >
          <span className="flex w-full items-center gap-2">
            <span className="size-[7px] flex-none rounded-full" style={{ background: color(s) }} />
            <span className="min-w-0 flex-1 truncate font-mono text-[12.5px] leading-[1.2] font-medium">{s.name}</span>
            <Pill tone={SESSION_TONE[s.state]}>{s.state.replace("_", " ")}</Pill>
          </span>
          <span className="pl-[15px] font-mono text-[10.5px] leading-[1.3] text-muted">
            {prefix ? `${prefix(s)} · ` : ""}
            {s.provider} · {s.model} · {s.role}
            {s.task_id != null ? ` · #${s.task_id}` : ""}
            {s.attached ? " · attached" : ""}
          </span>
          {(s.last_line || s.state === "rate_limited") && (
            <span className="max-w-full truncate pl-[15px] font-mono text-[10.5px] leading-[1.3] text-text-2">
              {s.state === "rate_limited" && s.rate_limit_reset_at ? `wakes ${rel(s.rate_limit_reset_at, now)}` : `› ${s.last_line}`}
            </span>
          )}
        </button>
      ))}
    </div>
  );
}

const fieldLabel = "flex flex-col gap-1.5 font-mono text-[11px] leading-none font-medium tracking-[0.08em] text-muted uppercase";
const field = "rounded-[7px] border border-line-2 bg-inset text-text outline-none placeholder:text-muted/80 focus:border-accent font-normal tracking-normal normal-case";

/** New session (design: the "New session · <name>" modal). The session runs in the current project. */
export function NewSessionModal({
  providers,
  stateOf,
  prefix,
  initialProvider,
  project,
  projectName,
  onClose,
  onCreated,
  onSignIn,
}: {
  providers: ProviderInfo[];
  stateOf: (p: ProviderInfo) => ProviderState;
  prefix: string;
  initialProvider?: string;
  project: string | null;
  projectName: string;
  onClose: () => void;
  onCreated: (s: Session) => void;
  onSignIn: (provider: string) => void;
}) {
  const usable = providers.filter((p) => p.available);
  const [picked, setProvider] = useState(initialProvider ?? "");
  // Until one is picked: the first provider that is installed and not signed out (providers may still be loading).
  const provider = picked || (usable.find((p) => !stateOf(p).signIn)?.name ?? usable[0]?.name ?? "");
  const [model, setModel] = useState("");
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [supervisor, setSupervisor] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const info = providers.find((p) => p.name === provider);
  const clean = name.trim().replace(/\s+/g, "-");
  const preview = clean ? (clean.startsWith(prefix) ? clean : `${prefix}${clean}`) : `${prefix}${provider || "…"}-HHMMSS`;

  const submit = async () => {
    if (!provider || busy) return;
    setBusy(true);
    setError("");
    try {
      const s = await api.createSession({ provider, model: model || undefined, name: clean || undefined, prompt: prompt.trim() || undefined, supervisor, project: project ?? undefined });
      onCreated(s);
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose} label="New session" width={540}>
      <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-4">
        <span className="flex min-w-0 items-center gap-2 font-mono text-[11px] font-semibold tracking-[0.14em] uppercase">
          <span className="text-accent">○</span>
          <span className="truncate">New session · {preview}</span>
        </span>
        <button onClick={onClose} className="size-[26px] flex-none cursor-pointer rounded-md border border-line-2 text-nav" aria-label="Close">
          ×
        </button>
      </div>
      <div className="flex flex-col gap-4 p-[18px]">
        <div className="flex flex-col gap-2">
          <span className={fieldLabel}>Provider</span>
          <div className="flex flex-wrap gap-1.5">
            {usable.map((p) => {
              const st = stateOf(p);
              return (
                <Chip
                  key={p.name}
                  on={provider === p.name}
                  onClick={() => {
                    if (st.signIn) onSignIn(p.name);
                    setProvider(p.name);
                    setModel("");
                  }}
                >
                  <span className="size-1.5 rounded-full" style={{ background: providerColor(p.name) }} />
                  {p.name}
                  {st.signIn ? " · sign in" : ""}
                </Chip>
              );
            })}
            {usable.length === 0 && <span className="font-mono text-[11px] text-muted">No provider CLI was found on this machine (see `map check`).</span>}
          </div>
        </div>
        {info && (
          <div className="flex flex-col gap-2">
            <span className={fieldLabel}>Model</span>
            <div className="flex flex-wrap gap-1.5">
              <Chip on={model === ""} onClick={() => setModel("")}>
                default · {info.default_model}
              </Chip>
              {info.models
                .filter((m) => m !== info.default_model)
                .map((m) => (
                  <Chip key={m} on={model === m} onClick={() => setModel(m)}>
                    {m}
                  </Chip>
                ))}
            </div>
          </div>
        )}
        <label className={fieldLabel}>
          Name (optional)
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="reviewer" className={clsx(field, "h-[38px] px-3 font-mono text-[13px]")} />
        </label>
        <button type="button" onClick={() => setSupervisor(!supervisor)} className="flex cursor-pointer items-start gap-2.5 text-left text-text">
          <span
            className={clsx(
              "mt-px size-4 flex-none rounded border text-center text-[11px] leading-4 font-bold text-accent-text",
              supervisor ? "border-accent bg-accent" : "border-line-3 bg-transparent",
            )}
          >
            {supervisor ? "✓" : ""}
          </span>
          <span className="flex flex-col gap-[3px]">
            <span className="text-[13px] leading-[1.2] font-medium">Supervisor role</span>
            <span className="font-mono text-[11px] leading-[1.4] text-muted">a long-lived planning agent that creates and reviews tasks through the API</span>
          </span>
        </button>
        <label className={fieldLabel}>
          Initial prompt (optional)
          <textarea
            rows={4}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder="Review the open diff and list risks…"
            className={clsx(field, "resize-y px-3 py-2.5 font-mono text-[12.5px] leading-[1.6]")}
          />
        </label>
        <p className="font-mono text-[11px] text-muted">Runs in {projectName}.</p>
        {error && <p className="text-[12px] text-bad">{error}</p>}
      </div>
      <div className="flex justify-end gap-2 border-t border-line px-[18px] py-3.5">
        <Btn size="lg" onClick={onClose}>
          Cancel
        </Btn>
        <Btn size="lg" kind="primary" disabled={!provider || busy} onClick={submit}>
          {busy ? "Starting…" : "Start session"}
        </Btn>
      </div>
    </Modal>
  );
}
