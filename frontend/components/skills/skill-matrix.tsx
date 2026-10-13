import clsx from "clsx";

import type { Skill } from "@/lib/generated/Skill";
import type { Task } from "@/lib/generated/Task";

import { slash, STATUS_COLOR, tint } from "./shared";

/** Rows: skills; columns: this project's tasks. A filled cell means Claude loaded the skill during that task's run;
 * its colour is the task's status. */
export function SkillMatrix({ skills, tasks, selected, onSelect }: { skills: Skill[]; tasks: Task[]; selected: string | null; onSelect: (id: string) => void }) {
  if (!tasks.length) return <p className="rounded-xl border border-line bg-surface px-4 py-6 text-center text-sm text-muted">No tasks in this project yet: the matrix fills in as tasks run.</p>;
  if (!skills.length) return <p className="rounded-xl border border-line bg-surface px-4 py-6 text-center text-sm text-muted">No skills match the filters.</p>;
  return (
    <div className="overflow-auto rounded-xl border border-line bg-white/[.015]">
      <div className="flex min-w-max flex-col px-3 py-2.5">
        <div className="flex h-6 items-center">
          <span className="w-[190px] flex-none" />
          {tasks.map((t) => (
            <span key={t.id} title={`#${t.id} ${t.title} · ${t.status}`} className="w-[30px] flex-none text-center font-mono text-[10px] text-muted">
              #{t.id}
            </span>
          ))}
        </div>
        {skills.map((s) => {
          const on = s.id === selected;
          return (
            <button key={s.id} onClick={() => onSelect(s.id)} className={clsx("flex h-7 items-center rounded-md p-0 text-text", on ? "bg-accent/[.08]" : "hover:bg-white/[.03]")}>
              <span className={clsx("w-[190px] flex-none truncate px-2 text-left font-mono text-[11.5px] font-medium", !s.enabled ? "text-muted/70" : on ? "text-accent-soft" : "text-nav")}>/{slash(s)}</span>
              {tasks.map((t) => {
                const used = s.usage.task_ids.includes(t.id);
                const c = STATUS_COLOR[t.status];
                const faint = t.status === "pending" || t.status === "cancelled";
                return (
                  <span key={t.id} className="flex w-[30px] flex-none items-center justify-center" title={used ? `loaded during #${t.id} (${t.status})` : undefined}>
                    <span className="size-3 rounded-[3px]" style={{ background: used ? tint(c, faint ? 25 : 80) : "transparent", border: `1px solid ${used ? c : "var(--line)"}` }} />
                  </span>
                );
              })}
            </button>
          );
        })}
      </div>
    </div>
  );
}
