"use client";

import clsx from "clsx";
import { Check, Copy, Play } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { Skill } from "@/lib/generated/Skill";
import type { Task } from "@/lib/generated/Task";

import { TaskBadge } from "../ui";
import { ago, loadNote } from "./shared";

type FileState = { id: string; lines: string[]; truncated: boolean } | { id: string; error: string };

/** Colours SKILL.md like the design: frontmatter dim, headings bright, prose secondary. */
function lineClass(line: string, i: number, fmEnd: number): string {
  if (i <= fmEnd) return line.trim() === "---" ? "text-muted/70" : "text-nav";
  return line.startsWith("#") ? "text-text" : "text-text-2";
}

function SkillMarkdown({ project, skill }: { project: string | null; skill: Skill }) {
  const [file, setFile] = useState<FileState | null>(null);
  useEffect(() => {
    let live = true;
    api
      .skillFile(project, skill.id)
      .then((f) => live && setFile({ id: skill.id, lines: f.content.split("\n"), truncated: f.truncated }))
      .catch((e: Error) => live && setFile({ id: skill.id, error: e.message }));
    return () => {
      live = false;
    };
  }, [project, skill.id]);

  const current = file?.id === skill.id ? file : null;
  let body;
  if (!current) body = <span className="text-muted">loading…</span>;
  else if ("error" in current) body = <span className="text-bad">{current.error}</span>;
  else {
    const fmEnd = current.lines[0]?.trim() === "---" ? current.lines.findIndex((l, i) => i > 0 && l.trim() === "---") : -1;
    body = (
      <>
        {current.lines.map((line, i) => (
          <span key={i} className={clsx("break-words whitespace-pre-wrap", lineClass(line, i, fmEnd))}>
            {line || " "}
          </span>
        ))}
        {current.truncated && <span className="text-warn">… cut at 256 KB</span>}
      </>
    );
  }
  return <div className="flex max-h-[220px] flex-col overflow-auto rounded-lg border border-line bg-inset px-3 py-2.5 font-mono text-[11.5px] leading-[1.6]">{body}</div>;
}

function Row({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[96px_minmax(0,1fr)] items-baseline gap-3">
      <span className="font-mono text-[10px] leading-[1.5] tracking-[0.08em] text-muted uppercase">{k}</span>
      <span className="font-mono text-[12.5px] leading-[1.45] break-words text-text">{children}</span>
    </div>
  );
}

export function SkillDetail({
  skill,
  project,
  tasks,
  now,
  busy,
  onToggle,
  onRun,
}: {
  skill: Skill;
  project: string | null;
  tasks: Task[];
  now: number;
  busy: boolean;
  onToggle: () => void;
  onRun: () => void;
}) {
  const [copied, setCopied] = useState<string | null>(null);
  const used = skill.usage.task_ids.map((id) => ({ id, task: tasks.find((t) => t.id === id) ?? null }));
  const kicker =
    skill.source === "plugin" ? `plugin · ${skill.plugin}` : skill.home === "codex" ? `${skill.source} skill · codex` : skill.invocation.startsWith("anthropic-skills:") ? "user skill · synced from claude.ai" : `${skill.source} skill`;
  const u = skill.usage;
  const last = !skill.usage_known ? "not recorded for codex skills" : u.last_used ? `${ago(u.last_used, now)} · ${u.loads} load${u.loads === 1 ? "" : "s"} in ${u.sessions} session${u.sessions === 1 ? "" : "s"}` : "never";

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(skill.id);
      setCopied(skill.id);
    } catch {
      setCopied(null);
    }
  };

  return (
    <aside className="sticky top-16 flex max-w-full min-w-0 flex-[1_1_380px] flex-col overflow-hidden rounded-[14px] border border-line-2 bg-[color-mix(in_srgb,var(--panel)_92%,transparent)]">
      <div className="flex items-start gap-2.5 border-b border-line px-[18px] pt-4 pb-3.5">
        <div className="flex min-w-0 flex-1 flex-col gap-[7px]">
          <span className="label">{kicker}</span>
          <span className="font-mono text-lg leading-[1.2] font-semibold break-words text-accent-soft">/{skill.invocation}</span>
          <span className="text-[13px] leading-[1.5] text-pretty text-nav">{skill.description || "No description."}</span>
        </div>
        <button
          onClick={onToggle}
          disabled={busy || !skill.can_toggle}
          title={skill.can_toggle ? `${skill.enabled ? "Disable" : "Enable"} for Claude in this workspace` : (skill.toggle_note ?? undefined)}
          className={clsx(
            "flex h-7 flex-none items-center gap-2 rounded-[7px] border bg-transparent px-2.5 font-mono text-[11px] font-medium whitespace-nowrap enabled:hover:bg-white/5 disabled:cursor-not-allowed",
            skill.enabled ? "border-ok/40 text-ok" : "border-line-3 text-muted",
            busy && "opacity-60",
          )}
        >
          <span className="size-[7px] rounded-full bg-current" />
          {busy ? "saving…" : skill.enabled ? "enabled" : "disabled"}
        </button>
      </div>
      {skill.toggle_note && <p className="border-b border-line px-[18px] py-2 font-mono text-[10.5px] leading-[1.5] text-muted">{skill.toggle_note}</p>}

      <div className="flex flex-col gap-2 px-[18px] py-3.5">
        <Row k="Path">{skill.path}</Row>
        <Row k="Loads in">{loadNote(skill)}</Row>
        <Row k="Tools">{skill.allowed_tools.length ? skill.allowed_tools.join(", ") : "no allowed-tools (asks as usual)"}</Row>
        <Row k="Last used">{last}</Row>
        <Row k="Invoke">
          /{skill.invocation}
          {skill.model_invocable ? "" : " · only when a person types it"}
        </Row>
      </div>

      <div className="flex flex-col gap-1.5 px-[18px] pb-3.5">
        <span className="label">SKILL.md</span>
        <SkillMarkdown project={project} skill={skill} />
      </div>

      <div className="flex flex-col gap-1.5 px-[18px] pb-3.5">
        <span className="label">{used.length ? `Used by ${used.length} task${used.length === 1 ? "" : "s"}` : skill.usage_known ? "Not used by any task yet" : "Task use not recorded"}</span>
        <div className="flex flex-col">
          {used.map(({ id, task }) => (
            <div key={id} className="grid grid-cols-[34px_minmax(0,1fr)_auto] items-center gap-2 border-t border-line py-[7px]">
              <span className="font-mono text-[10.5px] text-muted">#{id}</span>
              <span className="truncate text-[12.5px]">{task?.title ?? "deleted task"}</span>
              {task ? <TaskBadge status={task.status} /> : <span />}
            </div>
          ))}
        </div>
      </div>

      <div className="flex flex-wrap gap-2 border-t border-line px-[18px] pt-3.5 pb-4">
        <button
          onClick={onRun}
          disabled={!skill.enabled}
          title={skill.enabled ? "Queue a task in this project that uses the skill" : "Enable it for this project first"}
          className="flex h-8 items-center gap-1.5 rounded-[7px] border border-accent bg-accent px-[13px] text-xs font-semibold whitespace-nowrap text-accent-text hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
        >
          <Play className="size-3 fill-current" />
          Run as task
        </button>
        <button
          onClick={() => void copy()}
          className="flex h-8 items-center gap-1.5 rounded-[7px] border border-line-3 bg-transparent px-[13px] text-xs font-semibold whitespace-nowrap text-text hover:bg-white/5"
        >
          {copied === skill.id ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
          {copied === skill.id ? "Path copied" : "Copy path"}
        </button>
      </div>
    </aside>
  );
}
