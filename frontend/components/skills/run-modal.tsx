"use client";

import { useState } from "react";

import { api } from "@/lib/api";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Skill } from "@/lib/generated/Skill";
import type { Task } from "@/lib/generated/Task";

import { Modal } from "../shell/page";
import { Button, Input, Label, Select, Textarea } from "../ui";

export function defaultPrompt(skill: Skill): string {
  const how = skill.home === "claude" ? `Load the /${skill.invocation} skill (Skill tool) and follow it.` : `Use the ${skill.name} skill (${skill.path}) and follow it.`;
  return `${how}\n\n${skill.description}`.trim();
}

/** Lets the person edit the task before it is queued: title, prompt, and which provider runs it. */
export function RunSkillModal({
  skill,
  project,
  projectName,
  providers,
  onClose,
  onCreated,
}: {
  skill: Skill;
  project: string | null;
  projectName: string;
  providers: ProviderInfo[];
  onClose: () => void;
  onCreated: (task: Task) => void;
}) {
  const loaders = skill.loads_in.map((l) => l.provider);
  const [title, setTitle] = useState(`Run /${skill.invocation}`);
  const [prompt, setPrompt] = useState(() => defaultPrompt(skill));
  // Only the provider that loads it natively is sure to have the skill.
  const [provider, setProvider] = useState(skill.home);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const task = await api.createTask({ title: title.trim(), prompt, depends_on: [], provider: provider || undefined, project: project ?? undefined });
      onCreated(task);
    } catch (e) {
      setError((e as Error).message);
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onClose} label={`Run /${skill.invocation} as a task`} width={560}>
      <form
        className="flex flex-col gap-3.5 p-5"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <div>
          <p className="label">New task · {projectName}</p>
          <h2 className="mt-1.5 font-mono text-base font-semibold text-accent-soft">/{skill.invocation}</h2>
        </div>
        <Label text="Title">
          <Input value={title} onChange={(e) => setTitle(e.target.value)} required />
        </Label>
        <Label text="Prompt">
          <Textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} rows={7} required />
        </Label>
        <Label text="Provider">
          <Select value={provider} onChange={(e) => setProvider(e.target.value)}>
            <option value="">any (provider_order)</option>
            {providers.map((p) => (
              <option key={p.name} value={p.name}>
                {p.name}
                {loaders.includes(p.name) ? ` · loads it ${skill.loads_in.find((l) => l.provider === p.name)?.how}` : " · does not load it"}
                {p.available ? "" : " · not installed"}
              </option>
            ))}
          </Select>
        </Label>
        <p className="font-mono text-[10.5px] leading-[1.5] text-muted">The orchestrator adds its protocol footer (result file, verify commands) when the task starts.</p>
        {error && <p className="text-xs text-bad">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={busy || !title.trim() || !prompt.trim()}>
            {busy ? "Queuing…" : "Queue task"}
          </Button>
        </div>
      </form>
    </Modal>
  );
}
