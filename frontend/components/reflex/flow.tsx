import type { ReactNode } from "react";

import type { DecisionSummary } from "@/lib/generated/DecisionSummary";
import type { Lane } from "@/lib/generated/Lane";
import type { Question } from "@/lib/generated/Question";

import { useApp } from "../app-context";
import { LANE_COLOR, type Axis, fallbackName, fmtConf, fmtMs, kindLabel, mix, type Corpus, type Mode, pct } from "./lib";
import { Bar } from "./ui";

const GATE_NOTE: Record<Mode, (n: number, domains: number) => string> = {
  live: (n) => `set by calibration on ${n} cases`,
  shadow: () => "not calibrated · every question goes to the LLM",
  off: () => "Reflex is not installed",
  nocorpus: () => "no corpus to answer from",
  blocked: (_, d) => `paused · ${d} domains, limit 8`,
  error: () => "Reflex is not running",
  noq: () => "no questions defined",
};

function Box({ kicker, children, accent }: { kicker: string; children: ReactNode; accent?: boolean }) {
  return (
    <div
      className="flex min-w-[140px] flex-[1_1_150px] flex-col gap-[7px] rounded-[10px] border bg-r-panel2 px-3.5 py-3"
      style={{ borderColor: accent ? "var(--r-accent)" : "var(--r-line)" }}
    >
      <span className="text-[10px] leading-none font-medium tracking-[0.16em] text-r-muted uppercase">{kicker}</span>
      {children}
    </div>
  );
}

const Arrow = () => (
  <span className="flex-none self-center text-base leading-none text-r-faint" aria-hidden>
    →
  </span>
);

/** 01 · How it works: question → Reflex → gate → the three lanes with their real 24 h share and latency. */
export function Flow({
  question,
  corpus,
  threshold,
  mode,
  cases,
  fallback,
  summary,
  axis,
}: {
  question: Question | null;
  corpus: Corpus | null;
  threshold: number | null;
  mode: Mode;
  cases: number;
  fallback: string;
  summary: DecisionSummary | null;
  axis: Axis;
}) {
  const { server } = useApp();
  const total = summary?.total ?? 0;
  const share = (l: Lane) => (total ? (summary?.[l].count ?? 0) / total : 0);
  const avg = (l: Lane) => summary?.[l].avg_ms ?? 0;
  const lanes: { when: string; lane: Lane; note: string }[] = [
    { when: "yes", lane: "reflex", note: summary?.reflex.count ? `free · ~${fmtMs(avg("reflex"))}` : "free" },
    { when: "no", lane: "llm", note: `${fallbackName(fallback, server?.api_fallback)}${summary?.llm.count ? ` · ~${fmtMs(avg("llm"))}` : ""}` },
    { when: "nobody", lane: "abstain", note: "no answer" },
  ];
  const domains = corpus?.domains.length ?? 0;
  const reflexN = summary?.reflex.count ?? 0;
  return (
    <div className="flex flex-col gap-3.5">
      <div className="flex flex-wrap items-stretch gap-2">
        <Box kicker="01 · Question">
          <span className="text-[12.5px] leading-[1.4] font-medium text-pretty">{question?.prompt ?? "No question defined yet"}</span>
          <span className="text-[10.5px] leading-tight text-r-faint">
            {question ? `${kindLabel(question)} · ${question.kind === "noul" ? 2 : (question.options?.length ?? 0)} options` : "choice · score · yes-no"}
          </span>
        </Box>
        <Arrow />
        <Box kicker="02 · Reflex" accent={mode === "live"}>
          <span className="text-[12.5px] leading-[1.4] font-semibold">local · no model</span>
          <span className="text-[10.5px] leading-[1.4] text-r-faint">{domains && !corpus?.placeholder ? `${domains} domains · ${corpus?.files} files` : "no corpus"}</span>
          <span className="text-[10.5px] leading-[1.4] text-r-faint">confidence 1e{axis.lo} … 1e{axis.hi}</span>
        </Box>
        <Arrow />
        <Box kicker="03 · Gate">
          <span className="text-[12.5px] leading-[1.4] font-semibold">
            confidence ≥ <span className="text-r-accent">{threshold === null ? "not set" : fmtConf(threshold)}</span> ?
          </span>
          <span className="text-[10.5px] leading-[1.4] text-pretty text-r-faint">{GATE_NOTE[mode](cases, domains)}</span>
        </Box>
        <Arrow />
        <div className="flex min-w-[260px] flex-[2.2_1_300px] flex-col gap-1.5">
          {lanes.map((l) => {
            const c = LANE_COLOR[l.lane];
            return (
              <div
                key={l.lane}
                className="grid grid-cols-[62px_minmax(0,1fr)_auto] items-center gap-2.5 rounded-lg border px-3 py-[9px]"
                style={{ borderColor: mix(c, 35), background: mix(c, 6) }}
              >
                <span className="text-[9.5px] leading-tight font-medium tracking-[0.12em] text-r-muted uppercase">{l.when}</span>
                <span className="flex min-w-0 flex-col gap-1.5">
                  <span className="flex flex-wrap items-center gap-2">
                    <span className="size-[7px] rounded-full" style={{ background: c }} />
                    <span className="text-[11px] leading-none font-semibold tracking-[0.14em] uppercase" style={{ color: c }}>
                      {l.lane}
                    </span>
                    <span className="text-[10.5px] leading-none whitespace-nowrap text-r-faint">{l.note}</span>
                  </span>
                  <Bar width={pct(share(l.lane))} color={c} />
                </span>
                <span className="text-[20px] leading-none font-semibold whitespace-nowrap" style={{ color: c }}>
                  {pct(share(l.lane))}
                </span>
              </div>
            );
          })}
        </div>
      </div>
      <span className="text-[11px] leading-normal text-pretty text-r-muted">
        {total
          ? `Last 24 h · ${total} questions · ${pct(share("reflex"))} answered by Reflex, so ${reflexN} LLM call${reflexN === 1 ? " was" : "s were"} never made.`
          : "No decisions in the last 24 h yet. Ask one in the playground, or let the project's agents call $MAP_DECIDE_URL."}
      </span>
    </div>
  );
}
