"use client";

/** The task queue's lane part: the to-do cards (`tasks/todo/*.md`) of the lanes in scope, one block per lane, each card
 * a link to its lane on the Lanes screen. The lanes' own agents pick these up; the orchestrator never runs them. */

import type { LaneInfo } from "@/lib/generated/LaneInfo";

import { useApp } from "../app-context";

/** `470-queue-card-overdue-pulse` → ["470", "queue card overdue pulse"]. */
function split(card: string): [string | null, string] {
  const m = /^(\d+)[-_ ](.*)$/.exec(card);
  return m ? [m[1], m[2].replace(/[-_]+/g, " ")] : [null, card.replace(/[-_]+/g, " ")];
}

export function LaneCards({ lanes }: { lanes: LaneInfo[] }) {
  const { go } = useApp();
  const withCards = lanes.filter((l) => (l.tasks?.todo ?? 0) > 0);
  if (withCards.length === 0) return null;
  return (
    <div className="flex flex-col gap-2">
      {withCards.map((l) => {
        const t = l.tasks!;
        const more = t.todo - t.todo_cards.length;
        return (
          <div key={l.name} className="flex flex-col overflow-hidden rounded-[10px] border border-line">
            <button
              type="button"
              onClick={() => go("lanes", l.name)}
              className="flex cursor-pointer items-baseline justify-between gap-2 border-b border-line px-3 py-1.5 text-left hover:bg-white/[0.025]"
            >
              <span className="truncate font-mono text-[9.5px] tracking-[0.12em] text-muted uppercase">tasks/todo · {l.label}</span>
              <span className="flex-none font-mono text-[10.5px] text-text-2">
                {t.todo}
                {t.todo_human > 0 && <span className="text-warn"> · {t.todo_human} for you</span>}
              </span>
            </button>
            {t.todo_cards.map((c) => {
              const [num, title] = split(c);
              return (
                <button
                  key={c}
                  type="button"
                  title={`${c}.md`}
                  onClick={() => go("lanes", l.name)}
                  className="grid cursor-pointer grid-cols-[34px_minmax(0,1fr)] items-baseline gap-2 border-b border-line px-3 py-[7px] text-left last:border-b-0 hover:bg-white/[0.025]"
                >
                  <span className="font-mono text-[10.5px] text-muted">{num ?? "·"}</span>
                  <span className="truncate text-[12px] leading-[1.3] text-text">{title}</span>
                </button>
              );
            })}
            {more > 0 && <span className="px-3 py-1.5 font-mono text-[10.5px] text-muted">+{more} more</span>}
          </div>
        );
      })}
    </div>
  );
}
