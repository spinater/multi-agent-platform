import type { ReactNode } from "react";

import { mix } from "./lib";

export type Step = {
  title: string;
  desc: ReactNode;
  done: boolean;
  status: string;
  statusColor: string;
  cta?: { label: string; onClick: () => void; busy?: boolean };
};

/** 02 · Set up: five steps that tick themselves from real state; the first open one is current. */
export function Setup({ steps }: { steps: Step[] }) {
  const cur = steps.findIndex((s) => !s.done);
  const done = steps.filter((s) => s.done).length;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-[10.5px] leading-none font-medium tracking-[0.14em] whitespace-nowrap text-r-muted uppercase">
          {done} of {steps.length} done
        </span>
        <span className="flex min-w-[100px] flex-[1_1_120px] gap-[3px]" aria-hidden>
          {steps.map((s, i) => (
            <span key={s.title} className="h-1 flex-1 rounded-sm" style={{ background: s.done ? "var(--lane-reflex)" : i === cur ? "var(--r-accent)" : "var(--r-line)" }} />
          ))}
        </span>
      </div>
      <ol className="grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-2">
        {steps.map((s, i) => {
          const current = i === cur;
          return (
            <li
              key={s.title}
              aria-current={current ? "step" : undefined}
              className="flex min-w-0 flex-col gap-2 rounded-[10px] border p-3"
              style={{ borderColor: current ? "var(--r-accent)" : "var(--r-line)", background: current ? mix("var(--r-accent)", 6) : "transparent" }}
            >
              <div className="flex items-center gap-[9px]">
                <span
                  className="size-5 flex-none rounded-full border text-center text-[10.5px] leading-[18px] font-semibold"
                  style={{
                    borderColor: s.done ? "var(--lane-reflex)" : current ? "var(--r-accent)" : "var(--r-line2)",
                    background: s.done ? "var(--lane-reflex)" : "transparent",
                    color: s.done ? "var(--r-bg)" : current ? "var(--r-accent)" : "var(--r-muted)",
                  }}
                >
                  {s.done ? "✓" : i + 1}
                </span>
                <span className="text-[12.5px] leading-tight font-semibold">{s.title}</span>
              </div>
              <span className="text-[11.5px] leading-normal text-pretty text-r-muted">{s.desc}</span>
              <span className="text-[10.5px] leading-[1.4] font-medium break-words" style={{ color: s.statusColor }}>
                {s.status}
              </span>
              {s.cta && (
                <button
                  onClick={s.cta.onClick}
                  disabled={s.cta.busy}
                  className="h-7 self-start rounded-md border px-[11px] text-[11px] font-semibold tracking-[0.04em] whitespace-nowrap disabled:opacity-50"
                  style={{
                    borderColor: current ? "var(--r-accent)" : "var(--r-line2)",
                    background: current ? "var(--r-accent)" : "transparent",
                    color: current ? "var(--r-accent-ink)" : "var(--r-text)",
                  }}
                >
                  {s.cta.busy ? "working…" : s.cta.label}
                </button>
              )}
            </li>
          );
        })}
      </ol>
    </div>
  );
}
