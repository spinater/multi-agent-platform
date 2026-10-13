"use client";

import { Plus, RotateCcw, Square, Trash2, X } from "lucide-react";
import { useState } from "react";

import { api } from "@/lib/api";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Task } from "@/lib/generated/Task";

import { Button, Empty, Input, Label, Panel, Select, TaskBadge, Textarea } from "./ui";

export function TaskBoard({ tasks, onOpenSession, onCreate }: { tasks: Task[]; onOpenSession: (name: string) => void; onCreate: () => void }) {
  const [error, setError] = useState("");
  const [open, setOpen] = useState<number | null>(null);

  const act = async (action: () => Promise<unknown>) => {
    setError("");
    try {
      await action();
    } catch (err) {
      setError((err as Error).message);
    }
  };

  const counts = tasks.reduce<Record<string, number>>((acc, t) => ({ ...acc, [t.status]: (acc[t.status] ?? 0) + 1 }), {});
  const summary = ["running", "pending", "done", "failed"].filter((s) => counts[s]).map((s) => `${counts[s]} ${s}`).join(" · ");

  return (
    <Panel
      title={`Tasks${summary ? ` · ${summary}` : ""}`}
      actions={
        <Button size="sm" variant="primary" onClick={onCreate}>
          <Plus className="size-3.5" /> New task
        </Button>
      }
    >
      {error && <p className="px-4 pt-3 text-sm text-bad">{error}</p>}
      {tasks.length === 0 ? (
        <Empty>No tasks. Add one; with the orchestrator running, it starts as soon as its dependencies are done.</Empty>
      ) : (
        <ul className="divide-y divide-line">
          {tasks.map((t) => (
            <li key={t.id} className="px-4 py-2.5">
              <div className="flex items-center gap-3">
                <span className="w-9 shrink-0 font-mono text-xs text-muted">#{t.id}</span>
                <button className="min-w-0 flex-1 truncate text-left text-sm font-medium hover:underline" onClick={() => setOpen(open === t.id ? null : t.id)}>
                  {t.title}
                </button>
                {t.depends_on.length > 0 && <span className="hidden text-xs text-muted sm:inline">after {t.depends_on.map((d) => `#${d}`).join(", ")}</span>}
                {t.attempts > 1 && <span className="text-xs text-muted">try {t.attempts}</span>}
                <TaskBadge status={t.status} />
                <div className="flex gap-1">
                  {t.session && (
                    <Button size="sm" onClick={() => onOpenSession(t.session!)} title="Open the worker session">
                      view
                    </Button>
                  )}
                  {(t.status === "failed" || t.status === "cancelled") && (
                    <Button size="sm" onClick={() => act(() => api.taskAction(t.id, "retry"))} title="Retry">
                      <RotateCcw className="size-3.5" />
                    </Button>
                  )}
                  {!["done", "failed", "cancelled"].includes(t.status) && (
                    <Button size="sm" variant="danger" onClick={() => confirm(`Cancel task #${t.id}?`) && act(() => api.taskAction(t.id, "cancel"))} title="Cancel">
                      <Square className="size-3.5" />
                    </Button>
                  )}
                  {["done", "failed", "cancelled", "pending"].includes(t.status) && (
                    <Button size="sm" variant="danger" onClick={() => confirm(`Delete task #${t.id}?`) && act(() => api.deleteTask(t.id))} title="Delete">
                      <Trash2 className="size-3.5" />
                    </Button>
                  )}
                </div>
              </div>
              {t.result && open !== t.id && <p className="mt-1 truncate pl-12 text-xs text-muted">{t.result.split("\n")[0]}</p>}
              {open === t.id && (
                <div className="mt-2 grid gap-2 pl-12 text-xs">
                  <div>
                    <p className="mb-1 font-medium text-muted">Prompt</p>
                    <pre className="max-h-48 overflow-auto rounded bg-panel-2 p-2 font-mono whitespace-pre-wrap">{t.prompt}</pre>
                  </div>
                  {t.result && (
                    <div>
                      <p className="mb-1 font-medium text-muted">Result</p>
                      <pre className="max-h-64 overflow-auto rounded bg-panel-2 p-2 font-mono whitespace-pre-wrap">{t.result}</pre>
                    </div>
                  )}
                  <p className="text-muted">
                    {t.provider ? `Pinned to ${t.provider}${t.model ? ` (${t.model})` : ""}` : "Any provider"} · {t.attempts} attempt(s)
                  </p>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

export function NewTaskForm({ tasks, providers, onClose }: { tasks: Task[]; providers: ProviderInfo[]; onClose: () => void }) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [deps, setDeps] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const info = providers.find((p) => p.name === provider);
  const open = tasks.filter((t) => t.status !== "cancelled");

  const submit = async () => {
    setBusy(true);
    setError("");
    try {
      await api.createTask({ title, prompt, depends_on: deps, provider: provider || undefined, model: model || undefined });
      onClose();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel
      title="New task"
      actions={
        <Button size="sm" onClick={onClose} aria-label="Close">
          <X className="size-3.5" />
        </Button>
      }
    >
      <div className="grid gap-3 p-4 sm:grid-cols-2">
        <div className="sm:col-span-2">
          <Label text="Title">
            <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Add rate limiting to the login endpoint" />
          </Label>
        </div>
        <div className="sm:col-span-2">
          <Label text="Prompt for the worker">
            <Textarea rows={7} value={prompt} onChange={(e) => setPrompt(e.target.value)} placeholder="What to build, which files matter, and how to prove it works." />
          </Label>
        </div>
        <Label text="Provider">
          <Select
            value={provider}
            onChange={(e) => {
              setProvider(e.target.value);
              setModel("");
            }}
          >
            <option value="">Any (orchestrator picks, with failover)</option>
            {providers.map((p) => (
              <option key={p.name} value={p.name}>
                {p.name}
              </option>
            ))}
          </Select>
        </Label>
        <Label text="Model">
          <Select value={model} onChange={(e) => setModel(e.target.value)} disabled={!info}>
            <option value="">{info ? `Default (${info.default_model})` : "Provider default"}</option>
            {info?.models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </Select>
        </Label>
        {open.length > 0 && (
          <div className="sm:col-span-2">
            <p className="mb-1 text-xs font-medium text-muted">Runs after</p>
            <div className="flex max-h-28 flex-wrap gap-1.5 overflow-auto">
              {open.map((t) => (
                <label key={t.id} className="flex items-center gap-1.5 rounded border border-line bg-panel-2 px-2 py-1 text-xs">
                  <input
                    type="checkbox"
                    checked={deps.includes(t.id)}
                    onChange={(e) => setDeps(e.target.checked ? [...deps, t.id] : deps.filter((d) => d !== t.id))}
                    className="accent-[var(--accent)]"
                  />
                  #{t.id} {t.title.slice(0, 40)}
                </label>
              ))}
            </div>
          </div>
        )}
        {error && <p className="text-sm text-bad sm:col-span-2">{error}</p>}
        <div className="flex justify-end gap-2 sm:col-span-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={submit} disabled={busy || !title.trim() || !prompt.trim()}>
            {busy ? "Adding…" : "Add task"}
          </Button>
        </div>
      </div>
    </Panel>
  );
}
