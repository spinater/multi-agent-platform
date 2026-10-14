import type { Skill } from "@/lib/generated/Skill";
import type { SkillSource } from "@/lib/generated/SkillSource";
import type { TaskStatus } from "@/lib/generated/TaskStatus";

/** Badge colour per source (design: project accent, user info, plugin violet). */
export const SOURCE_COLOR: Record<SkillSource, string> = {
  project: "var(--accent)",
  user: "var(--info)",
  plugin: "var(--p-gemini)",
};

export const SOURCE_ORDER: SkillSource[] = ["project", "user", "plugin"];

/** The providers a skill card shows dots for, in this order. */
export const DOT_PROVIDERS = ["claude", "codex", "gemini"];

/** Task status colours in the matrix and lists: the same tones as `TaskBadge`. */
export const STATUS_COLOR: Record<TaskStatus, string> = {
  pending: "var(--idle)",
  running: "var(--ok)",
  verifying: "var(--info)",
  auditing: "var(--info)",
  landing: "var(--info)",
  review: "var(--warn)",
  done: "var(--accent)",
  failed: "var(--bad)",
  cancelled: "var(--idle)",
};

export function tint(color: string, pct: number): string {
  return `color-mix(in srgb, ${color} ${pct}%, transparent)`;
}

/** The short name a card shows: synced skills answer to `/name` as well as `/anthropic-skills:name`. */
export function slash(s: Skill): string {
  return s.invocation.startsWith("anthropic-skills:") ? s.name : s.invocation;
}

export function sourceLabel(s: Skill): string {
  return s.source === "plugin" ? (s.plugin ?? "plugin") : s.source;
}

/** "3m ago", "5h ago", "2d ago". */
export function ago(iso: string | null, now: number): string {
  if (!iso) return "never";
  const s = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 48 * 3600) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function usageText(s: Skill): string {
  if (!s.enabled) return "disabled";
  if (!s.usage_known) return "use not recorded";
  if (!s.usage.loads) return "not used yet";
  const tasks = s.usage.task_ids.length;
  return `${s.usage.loads} load${s.usage.loads === 1 ? "" : "s"} · ${tasks} task${tasks === 1 ? "" : "s"}`;
}

export function loadNote(s: Skill): string {
  return s.loads_in.map((l) => `${l.provider} (${l.how})`).join(" · ");
}

/** Project skills first; then the most loaded; then user before plugin, by name. */
export function compareSkills(a: Skill, b: Skill): number {
  const project = Number(b.source === "project") - Number(a.source === "project");
  return project || b.usage.loads - a.usage.loads || SOURCE_ORDER.indexOf(a.source) - SOURCE_ORDER.indexOf(b.source) || a.invocation.localeCompare(b.invocation);
}
