"use client";

import clsx from "clsx";
import { Trash2, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { AgentDef } from "@/lib/generated/AgentDef";
import type { AgentDetail } from "@/lib/generated/AgentDetail";
import type { AgentNote } from "@/lib/generated/AgentNote";
import type { AgentPlan } from "@/lib/generated/AgentPlan";
import type { AgentProvider } from "@/lib/generated/AgentProvider";
import type { AgentScope } from "@/lib/generated/AgentScope";
import type { Capability } from "@/lib/generated/Capability";
import type { Effort } from "@/lib/generated/Effort";

import { useApp } from "../app-context";
import { Button, Input, Label, Select, Textarea } from "../ui";
import { ScopeBadge } from "./agent-matrix";
import { AGENT_PROVIDERS, blankDef, CAPABILITIES, CLAUDE_MODELS, EFFORTS, GEMINI_MODELS, PROVIDER_FORMAT, PROVIDER_LABEL, runCommand } from "./shared";

/** The preview asks the backend to render after typing pauses this long. */
const PREVIEW_MS = 250;

export type EditorTarget = { scope: AgentScope; name: string } | "new";

function Check({ checked, disabled, onChange, children }: { checked: boolean; disabled?: boolean; onChange: (on: boolean) => void; children: ReactNode }) {
  return (
    <label className={clsx("flex cursor-pointer items-start gap-2 text-[12.5px] leading-[1.4] text-text", disabled && "cursor-not-allowed opacity-50")}>
      <input type="checkbox" className="mt-[2px] size-3.5 flex-none" style={{ accentColor: "var(--accent)" }} checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span className="min-w-0">{children}</span>
    </label>
  );
}

function Notes({ notes, title }: { notes: AgentNote[]; title: string }) {
  if (!notes.length) return null;
  return (
    <div className="flex flex-col gap-1.5">
      <span className="label">{title}</span>
      <ul className="flex flex-col gap-1">
        {notes.map((n, i) => (
          <li key={i} className="flex items-baseline gap-2 font-mono text-[11px] leading-[1.5] text-text-2">
            <span className="size-1.5 flex-none translate-y-[-1px] rounded-full" style={{ background: providerColor(n.provider) }} />
            <span>
              <span className="text-muted">{PROVIDER_LABEL[n.provider]} · </span>
              {n.text}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Preview({ plan, stale }: { plan: AgentPlan | null; stale: boolean }) {
  const [tab, setTab] = useState<AgentProvider | null>(null);
  if (!plan) return <p className="font-mono text-[11px] text-muted">rendering…</p>;
  const change = plan.changes.find((c) => c.provider === tab) ?? plan.changes[0];
  if (!change) return <p className="font-mono text-[11px] text-muted">Pick a CLI to see its file.</p>;
  return (
    <div className={clsx("flex min-w-0 flex-col gap-2 transition-opacity", stale && "opacity-60")}>
      <div className="flex flex-wrap gap-1.5">
        {plan.changes.map((c) => {
          const on = c === change;
          const action = c.content === null ? "remove" : c.exists ? "replace" : "new";
          return (
            <button
              key={`${c.provider}:${c.path}`}
              onClick={() => setTab(c.provider)}
              aria-pressed={on}
              className="flex h-7 items-center gap-1.5 rounded-md border px-2.5 font-mono text-[11px] text-text"
              style={{ borderColor: on ? providerColor(c.provider) : "var(--line-2)" }}
            >
              <span className="size-1.5 rounded-full" style={{ background: providerColor(c.provider) }} />
              {PROVIDER_LABEL[c.provider]}
              <span className={clsx(action === "remove" ? "text-bad" : action === "replace" ? "text-warn" : "text-ok")}>{action}</span>
            </button>
          );
        })}
      </div>
      <span className="font-mono text-[11px] break-all text-muted">{change.path}</span>
      {change.content === null ? (
        <p className="rounded-lg border border-bad/40 bg-bad/10 px-3 py-2 text-[12px] text-bad">Saving removes this file: {PROVIDER_LABEL[change.provider]} is no longer a target.</p>
      ) : (
        <pre className="max-h-[52vh] overflow-auto rounded-lg border border-line bg-inset px-3 py-2.5 font-mono text-[11.5px] leading-[1.6] break-words whitespace-pre-wrap text-text-2">{change.content}</pre>
      )}
    </div>
  );
}

function Form({ target, detail, onSaved, onDeleted }: { target: EditorTarget; detail: AgentDetail | null; onSaved: (scope: AgentScope, name: string) => void; onDeleted: () => void }) {
  const { project, toast } = useApp();
  const isNew = target === "new";
  const [def, setDef] = useState<AgentDef>(() => detail?.def ?? blankDef());
  const [scope, setScope] = useState<AgentScope>(isNew ? "project" : target.scope);
  const [plan, setPlan] = useState<{ key: string; plan: AgentPlan } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const key = JSON.stringify([project, scope, def]);
  useEffect(() => {
    let live = true;
    const t = window.setTimeout(() => {
      api
        .saveAgent(project, scope, def, true)
        .then((p) => live && setPlan({ key, plan: p }))
        .catch((e: Error) => live && setPlan({ key, plan: { changes: [], notes: [], error: e.message } }));
    }, PREVIEW_MS);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [project, scope, def, key]);

  const set = (patch: Partial<AgentDef>) => setDef((d) => ({ ...d, ...patch }));
  const setModel = (p: AgentProvider, v: string) => setDef((d) => ({ ...d, models: { ...d.models, [p]: v.trim() ? v : null } }));
  const toggleCap = (c: Capability, on: boolean) => set({ tools: on ? [...(def.tools ?? []), c] : (def.tools ?? []).filter((x) => x !== c) });
  const toggleTarget = (p: AgentProvider, on: boolean) => set({ targets: on ? AGENT_PROVIDERS.filter((x) => x === p || def.targets.includes(x)) : def.targets.filter((x) => x !== p) });
  const targeted = (p: AgentProvider) => def.targets.includes(p);

  const current = plan?.plan ?? null;
  // Notes from reading the files (dropped tools, unknown effort) that the live preview does not repeat.
  const readNotes = (detail?.notes ?? []).filter((n) => !current?.notes.some((m) => m.provider === n.provider && m.text === n.text));
  const removes = current?.changes.filter((c) => c.content === null).length ?? 0;

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const done = await api.saveAgent(project, scope, def);
      const written = done.changes.filter((c) => c.content !== null).length;
      toast(`Agent ${def.name} saved: ${written} file${written === 1 ? "" : "s"} written${removes ? `, ${removes} removed` : ""}`);
      onSaved(scope, def.name);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (isNew || !confirm(`Delete agent ${target.name}? Its ${detail?.files.length ?? 0} file(s) are removed for every CLI.`)) return;
    setBusy(true);
    try {
      const done = await api.deleteAgent(project, target.scope, target.name);
      toast(`Agent ${target.name} deleted: ${done.removed.join(", ")}`);
      onDeleted();
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  };

  return (
    <>
      <div className="grid min-h-0 flex-1 gap-6 overflow-y-auto px-5 py-5 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
        <div className="flex min-w-0 flex-col gap-4">
          <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto]">
            <Label text="Name">
              <Input value={def.name} onChange={(e) => set({ name: e.target.value })} disabled={!isNew} placeholder="code-reviewer" spellCheck={false} autoFocus={isNew} className="font-mono" />
            </Label>
            <fieldset className="flex flex-col gap-1">
              <legend className="mb-1 text-[11px] font-medium tracking-wide text-muted uppercase">Scope</legend>
              <div className="flex h-8 items-center gap-3">
                {(["project", "user"] as const).map((s) => (
                  <label key={s} className={clsx("flex items-center gap-1.5 text-[12.5px] text-text", !isNew && "opacity-60")}>
                    <input type="radio" name="agent-scope" style={{ accentColor: "var(--accent)" }} checked={scope === s} disabled={!isNew} onChange={() => setScope(s)} />
                    {s === "project" ? "This project" : "User-wide"}
                  </label>
                ))}
              </div>
            </fieldset>
          </div>
          <p className="-mt-2 font-mono text-[10.5px] leading-[1.5] text-muted">
            a-z, 0-9, - and _. The CLIs find agents by this name{isNew ? "" : "; to rename, create a new agent and delete this one"}.
          </p>
          <Label text="Description">
            <Textarea value={def.description} onChange={(e) => set({ description: e.target.value })} rows={2} placeholder="Reviews a diff for bugs and risky changes. Use after code changes." />
          </Label>
          <Label text="Instructions">
            <Textarea value={def.instructions} onChange={(e) => set({ instructions: e.target.value })} rows={12} placeholder="You are a senior reviewer…" />
          </Label>

          <fieldset className="flex flex-col gap-2">
            <legend className="label mb-2">Capabilities</legend>
            <Check checked={def.tools === null} onChange={(on) => set({ tools: on ? null : ["read", "search"] })}>
              All tools <span className="text-muted">(no tools key: whatever the CLI has, MCP included)</span>
            </Check>
            <div className="grid gap-2 pl-5 sm:grid-cols-2">
              {CAPABILITIES.map((c) => (
                <Check key={c.id} checked={def.tools === null || def.tools.includes(c.id)} disabled={def.tools === null} onChange={(on) => toggleCap(c.id, on)}>
                  {c.label}
                  <span className="block font-mono text-[10px] text-muted">{c.note}</span>
                </Check>
              ))}
            </div>
          </fieldset>

          <div className="grid gap-3 sm:grid-cols-3">
            <Label text="Claude model">
              <Input value={def.models.claude ?? ""} onChange={(e) => setModel("claude", e.target.value)} list="agent-claude-models" placeholder="inherit" disabled={!targeted("claude")} className="font-mono" />
              <datalist id="agent-claude-models">
                {CLAUDE_MODELS.map((m) => (
                  <option key={m} value={m} />
                ))}
              </datalist>
            </Label>
            <Label text="Codex model">
              <Input value={def.models.codex ?? ""} onChange={(e) => setModel("codex", e.target.value)} placeholder="Codex default" disabled={!targeted("codex")} className="font-mono" />
            </Label>
            <Label text="agy model">
              <Select value={def.models.gemini ?? ""} onChange={(e) => setModel("gemini", e.target.value)} disabled={!targeted("gemini")}>
                <option value="">default (no model key)</option>
                {GEMINI_MODELS.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            </Label>
          </div>
          <Label text="Effort">
            <Select value={def.effort ?? ""} onChange={(e) => set({ effort: (e.target.value || null) as Effort | null })} className="sm:max-w-[220px]">
              <option value="">default</option>
              {EFFORTS.map((e) => (
                <option key={e} value={e}>
                  {e}
                </option>
              ))}
            </Select>
          </Label>

          <fieldset className="flex flex-col gap-2">
            <legend className="label mb-2">Write for</legend>
            <div className="flex flex-wrap gap-x-5 gap-y-2">
              {AGENT_PROVIDERS.map((p) => (
                <Check key={p} checked={targeted(p)} onChange={(on) => toggleTarget(p, on)}>
                  <span className="inline-flex items-center gap-1.5">
                    <span className="size-1.5 rounded-full" style={{ background: providerColor(p) }} />
                    {PROVIDER_LABEL[p]} <span className="text-muted">{PROVIDER_FORMAT[p]}</span>
                  </span>
                </Check>
              ))}
            </div>
          </fieldset>

          {detail && (
            <div className="flex flex-col gap-1.5">
              <span className="label">Files now · read from {PROVIDER_LABEL[detail.source]}</span>
              {detail.files.map((f) => (
                <div key={`${f.provider}:${f.path}`} className="grid grid-cols-[86px_minmax(0,1fr)] gap-2 font-mono text-[11px] leading-[1.5]">
                  <span style={{ color: providerColor(f.provider) }}>{PROVIDER_LABEL[f.provider]}</span>
                  <span className="break-all text-text-2">
                    {f.path}
                    {f.error && <span className="block text-bad">! {f.error}</span>}
                  </span>
                </div>
              ))}
              <p className="font-mono text-[10.5px] leading-[1.5] text-muted">
                Keys MAP does not edit (permissionMode, mcpServers, sandbox_mode…) are kept when it rewrites a file.{" "}
                {AGENT_PROVIDERS.map((p) => runCommand(p, detail.def.name))
                  .filter(Boolean)
                  .join(" · ")}{" "}
                runs it as the main agent.
              </p>
            </div>
          )}
        </div>

        <div className="flex min-w-0 flex-col gap-4 lg:sticky lg:top-0 lg:self-start">
          <span className="label">Files to write</span>
          <Preview plan={current} stale={plan?.key !== key} />
          <Notes notes={current?.notes ?? []} title="What each format loses" />
          <Notes notes={readNotes} title="Read from the current files" />
        </div>
      </div>

      <footer className="flex flex-wrap items-center gap-2 border-t border-line px-5 py-3.5">
        {(error ?? current?.error) && <p className="mr-auto min-w-0 flex-1 text-xs break-words text-bad">{error ?? current?.error}</p>}
        <div className="ml-auto flex gap-2">
          {!isNew && (
            <Button variant="danger" onClick={() => void remove()} disabled={busy}>
              <Trash2 className="size-3.5" />
              Delete
            </Button>
          )}
          <Button variant="primary" onClick={() => void save()} disabled={busy || !current || !!current.error || plan?.key !== key}>
            {busy ? "Saving…" : removes ? `Save · remove ${removes} file${removes === 1 ? "" : "s"}` : "Save"}
          </Button>
        </div>
      </footer>
    </>
  );
}

type Loaded = { key: string; detail: AgentDetail } | { key: string; error: string };

/** Right-hand drawer: the editor with a live preview of every file a save writes or removes. */
export function AgentEditor({ target, onClose, onSaved, onDeleted }: { target: EditorTarget; onClose: () => void; onSaved: (scope: AgentScope, name: string) => void; onDeleted: () => void }) {
  const { project } = useApp();
  // Primitives only: the parent re-renders every second with a new `target` object.
  const [scope, name] = target === "new" ? [null, null] : [target.scope, target.name];
  const key = scope === null ? "new" : `${project ?? ""}:${scope}/${name}`;
  const [loaded, setLoaded] = useState<Loaded | null>(null);

  useEffect(() => {
    if (scope === null || name === null) return;
    let live = true;
    api
      .agent(project, scope, name)
      .then((detail) => live && setLoaded({ key, detail }))
      .catch((e: Error) => live && setLoaded({ key, error: e.message }));
    return () => {
      live = false;
    };
  }, [project, scope, name, key]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const ours = loaded?.key === key ? loaded : null;
  let body: ReactNode;
  if (target === "new") body = <Form key="new" target={target} detail={null} onSaved={onSaved} onDeleted={onDeleted} />;
  else if (!ours) body = <p className="label p-6">loading…</p>;
  else if ("error" in ours) body = <p className="m-5 rounded-lg border border-bad/40 bg-bad/10 px-3 py-2 text-sm text-bad">Could not read {target.name}: {ours.error}</p>;
  else body = <Form key={key} target={target} detail={ours.detail} onSaved={onSaved} onDeleted={onDeleted} />;

  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black/60" onClick={onClose}>
      <div
        role="dialog"
        aria-label={target === "new" ? "New agent" : `Agent ${target.name}`}
        className="flex h-full w-full max-w-[1120px] flex-col border-l border-line-2 bg-menu shadow-[0_20px_50px_rgba(0,0,0,.6)]"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-start gap-3 border-b border-line px-5 pt-4 pb-3.5">
          <div className="flex min-w-0 flex-1 flex-col gap-1.5">
            <span className="label">{target === "new" ? "New agent" : "Agent"}</span>
            <span className="flex min-w-0 items-center gap-2">
              <span className="truncate font-mono text-lg leading-[1.2] font-semibold text-accent-soft">{target === "new" ? "untitled" : target.name}</span>
              {target !== "new" && <ScopeBadge scope={target.scope} />}
            </span>
          </div>
          <button onClick={onClose} className="flex size-8 items-center justify-center rounded-md text-muted hover:bg-white/5 hover:text-text" aria-label="Close">
            <X className="size-4" />
          </button>
        </header>
        {body}
      </div>
    </div>
  );
}
