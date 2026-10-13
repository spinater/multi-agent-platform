"use client";

import clsx from "clsx";

import type { Event } from "@/lib/generated/Event";
import type { Level } from "@/lib/generated/Level";

import { Empty, Panel } from "./ui";

const dot: Record<Level, string> = { info: "bg-info", success: "bg-ok", warning: "bg-warn", error: "bg-bad" };

export function EventFeed({ events, onOpenSession }: { events: Event[]; onOpenSession: (name: string) => void }) {
  return (
    <Panel title="Activity">
      {events.length === 0 ? (
        <Empty>Nothing yet.</Empty>
      ) : (
        <ol className="max-h-[520px] divide-y divide-line overflow-auto">
          {events.map((e) => (
            <li key={`${e.id}-${e.at}`} className="flex gap-2.5 px-4 py-2 text-xs">
              <span className={clsx("mt-1.5 size-1.5 shrink-0 rounded-full", dot[e.level])} aria-label={e.level} />
              <div className="min-w-0 flex-1">
                <p className="break-words">{e.message}</p>
                <p className="mt-0.5 flex flex-wrap gap-x-2 text-muted">
                  <time dateTime={e.at}>{new Date(e.at).toLocaleTimeString()}</time>
                  {e.session && (
                    <button className="font-mono hover:underline" onClick={() => onOpenSession(e.session!)}>
                      {e.session}
                    </button>
                  )}
                  {e.task_id !== null && <span>task #{e.task_id}</span>}
                  <span className="opacity-70">{e.kind}</span>
                </p>
              </div>
            </li>
          ))}
        </ol>
      )}
    </Panel>
  );
}
