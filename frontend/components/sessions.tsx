"use client";

import clsx from "clsx";
import { Plus, X } from "lucide-react";
import { useState } from "react";

import { api } from "@/lib/api";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Session } from "@/lib/generated/Session";

import { Badge, Button, Empty, Input, Label, Panel, Select, SessionBadge, Textarea, relative } from "./ui";

export function SessionList({
  sessions,
  selected,
  onSelect,
  now,
  onCreate,
}: {
  sessions: Session[];
  selected: string | null;
  onSelect: (name: string) => void;
  now: number;
  onCreate: () => void;
}) {
  return (
    <Panel
      title={`Sessions (${sessions.filter((s) => s.state !== "exited").length} live)`}
      actions={
        <Button size="sm" variant="primary" onClick={onCreate}>
          <Plus className="size-3.5" /> New session
        </Button>
      }
    >
      {sessions.length === 0 ? (
        <Empty>No sessions yet. Start one, or add a task and run the orchestrator.</Empty>
      ) : (
        <ul className="grid gap-2 p-3 sm:grid-cols-2">
          {sessions.map((s) => (
            <li key={s.name}>
              <button
                onClick={() => onSelect(s.name)}
                className={clsx(
                  "flex w-full flex-col gap-1.5 rounded-md border bg-panel-2 p-3 text-left transition-colors",
                  selected === s.name ? "border-accent" : "border-line hover:border-muted",
                  s.state === "exited" && "opacity-60",
                )}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="truncate font-mono text-[13px] font-semibold">{s.name}</span>
                  <SessionBadge state={s.state} />
                </div>
                <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
                  <span>
                    {s.provider} · {s.model}
                  </span>
                  <Badge>{s.role}</Badge>
                  {s.task_id !== null && <Badge tone="accent">task #{s.task_id}</Badge>}
                  {s.attached && <Badge tone="info">attached</Badge>}
                  {s.wakeups > 0 && <span>{s.wakeups} wake-up{s.wakeups === 1 ? "" : "s"}</span>}
                </div>
                {s.state === "rate_limited" && s.rate_limit_reset_at && (
                  <p className="text-xs text-warn">Wakes {relative(s.rate_limit_reset_at, now)}</p>
                )}
                {s.detail && s.state !== "rate_limited" && <p className="line-clamp-2 text-xs text-muted">{s.detail}</p>}
                {s.last_line && <p className="truncate font-mono text-[11px] text-muted">› {s.last_line}</p>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

export function NewSessionForm({ providers, onDone, onClose }: { providers: ProviderInfo[]; onDone: (name: string) => void; onClose: () => void }) {
  const usable = providers.filter((p) => p.available);
  const [provider, setProvider] = useState(usable[0]?.name ?? providers[0]?.name ?? "");
  const info = providers.find((p) => p.name === provider);
  const [model, setModel] = useState(info?.default_model ?? "");
  const [name, setName] = useState("");
  const [prompt, setPrompt] = useState("");
  const [supervisor, setSupervisor] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const submit = async () => {
    setBusy(true);
    setError("");
    try {
      const s = await api.createSession({ provider, model, name: name || undefined, prompt: prompt || undefined, supervisor });
      onDone(s.name);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel
      title="New session"
      actions={
        <Button size="sm" onClick={onClose} aria-label="Close">
          <X className="size-3.5" />
        </Button>
      }
    >
      <div className="grid gap-3 p-4 sm:grid-cols-2">
        <Label text="Provider">
          <Select
            value={provider}
            onChange={(e) => {
              setProvider(e.target.value);
              setModel(providers.find((p) => p.name === e.target.value)?.default_model ?? "");
            }}
          >
            {providers.map((p) => (
              <option key={p.name} value={p.name} disabled={!p.available}>
                {p.name}
                {p.available ? "" : " (not installed)"}
              </option>
            ))}
          </Select>
        </Label>
        <Label text="Model">
          <Select value={model} onChange={(e) => setModel(e.target.value)}>
            {info?.models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </Select>
        </Label>
        <Label text="Name (optional)">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="reviewer" />
        </Label>
        <label className="flex items-end gap-2 pb-2 text-sm">
          <input type="checkbox" checked={supervisor} onChange={(e) => setSupervisor(e.target.checked)} className="size-4 accent-[var(--accent)]" />
          Supervisor role
        </label>
        <div className="sm:col-span-2">
          <Label text="Initial prompt (optional)">
            <Textarea rows={4} value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="Review the open diff and list risks…" />
          </Label>
        </div>
        {error && <p className="text-sm text-bad sm:col-span-2">{error}</p>}
        <div className="flex justify-end gap-2 sm:col-span-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={submit} disabled={busy || !provider}>
            {busy ? "Starting…" : "Start session"}
          </Button>
        </div>
      </div>
    </Panel>
  );
}
