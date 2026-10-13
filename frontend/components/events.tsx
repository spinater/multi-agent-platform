"use client";

import { useState } from "react";

import type { Event } from "@/lib/generated/Event";

import { LEVEL_TONE, Pill, hms } from "./command-ui";

/** The event log of the right column, newest first: time · kind · who · message (design: `eventRows`). */
export function EventLog({ events, who, onOpen }: { events: Event[]; who: (e: Event) => string; onOpen: (e: Event) => void }) {
  const [limit, setLimit] = useState(40);
  if (events.length === 0) return <p className="font-mono text-[11px] text-muted">Nothing yet.</p>;
  return (
    <div className="flex flex-col gap-0.5">
      {events.slice(0, limit).map((e) => (
        <button key={`${e.id}-${e.at}`} onClick={() => onOpen(e)} className="grid cursor-pointer grid-cols-[58px_minmax(0,1fr)] gap-2.5 py-1.5 text-left text-text">
          <time dateTime={e.at} className="font-mono text-[10.5px] leading-[1.6] text-muted/80">
            {hms(e.at)}
          </time>
          <span className="flex min-w-0 flex-col gap-1">
            <span className="flex flex-wrap items-center gap-1.5">
              <Pill tone={LEVEL_TONE[e.level]} small>
                {e.kind}
              </Pill>
              <span className="font-mono text-[10.5px] leading-none text-muted">{who(e)}</span>
            </span>
            <span className="text-[12px] leading-[1.4] break-words text-nav">{e.message}</span>
          </span>
        </button>
      ))}
      {events.length > limit && (
        <button onClick={() => setLimit(limit + 60)} className="mt-1 cursor-pointer self-start font-mono text-[11px] text-link hover:text-link-hover">
          show {Math.min(60, events.length - limit)} more
        </button>
      )}
    </div>
  );
}
