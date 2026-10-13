"use client";

import { useState } from "react";

import type { Decision } from "@/lib/generated/Decision";
import type { DecisionSummary } from "@/lib/generated/DecisionSummary";
import type { Lane } from "@/lib/generated/Lane";

import { LANE_COLOR, LANES, fmtAnswer, fmtConf, fmtMs, pct } from "./lib";
import { Cap, Chip, LaneLabel, Tile } from "./ui";

const ROWS = 40;

/** 05 · Results: 24 h tiles, decisions per hour by lane, and the filterable decision log. */
export function Log({ summary, decisions, emptyText, now }: { summary: DecisionSummary | null; decisions: Decision[]; emptyText: string; now: number }) {
  const [filter, setFilter] = useState<Lane | "all">("all");
  const [more, setMore] = useState(false);
  const total = summary?.total ?? 0;
  const s = (l: Lane) => summary?.[l] ?? { count: 0, avg_ms: 0 };
  const buckets = summary?.buckets ?? [];
  const max = Math.max(1, ...buckets.map((b) => b.reflex + b.llm + b.abstain));
  const filtered = decisions.filter((d) => filter === "all" || d.lane === filter);
  const rows = more ? filtered : filtered.slice(0, ROWS);

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(130px,1fr))] gap-2">
        <Tile dot k="LLM calls saved" v={pct(total ? s("reflex").count / total : 0)} color="var(--lane-reflex)" sub={`${s("reflex").count} of ${total} questions · 24 h`} />
        <Tile dot k="reflex" v={String(s("reflex").count)} color="var(--lane-reflex)" vcolor="var(--r-text)" sub={s("reflex").count ? `avg ${fmtMs(s("reflex").avg_ms)}` : "no answers yet"} />
        <Tile dot k="llm" v={String(s("llm").count)} color="var(--lane-llm)" vcolor="var(--r-text)" sub={s("llm").count ? `avg ${fmtMs(s("llm").avg_ms)}` : "no LLM calls"} />
        <Tile dot k="abstain" v={String(s("abstain").count)} color="var(--lane-abstain)" vcolor="var(--r-text)" sub="nobody answered" />
        <Tile
          dot
          k="feedback"
          v={`${summary?.feedback_right ?? 0} ✓ · ${summary?.feedback_wrong ?? 0} ✕`}
          color="var(--r-accent)"
          vcolor="var(--r-text)"
          sub="from the playground and reviews"
        />
      </div>

      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap justify-between gap-2.5">
          <Cap>decisions per hour · last 24 h</Cap>
          <span className="flex gap-3 text-[10px] tracking-[0.08em] text-r-muted uppercase">
            {LANES.map((l) => (
              <span key={l} className="flex items-center gap-[5px]">
                <span className="size-2 rounded-sm" style={{ background: LANE_COLOR[l] }} />
                {l}
              </span>
            ))}
          </span>
        </div>
        <div className="flex h-[110px] items-end gap-[3px] rounded-lg border border-r-line p-2" role="img" aria-label={`${total} decisions in the last 24 hours`}>
          {buckets.map((b) => {
            const ago = Math.max(0, Math.floor((now - new Date(b.start).getTime()) / 3600e3));
            return (
              <div
                key={b.start}
                title={`${new Date(b.start).toTimeString().slice(0, 5)} · ${ago ? `${ago} h ago` : "this hour"} · ${b.reflex} reflex · ${b.llm} llm · ${b.abstain} abstain`}
                className="flex h-full min-w-[3px] flex-1 flex-col-reverse gap-px"
              >
                {(["reflex", "llm", "abstain"] as const).map((l) =>
                  b[l] ? <span key={l} className="rounded-[1px]" style={{ height: `${(b[l] / max) * 100}%`, background: LANE_COLOR[l] }} /> : null,
                )}
              </div>
            );
          })}
        </div>
        <div className="flex justify-between text-[9.5px] leading-none text-r-faint">
          <span>-24 h</span>
          <span>-12 h</span>
          <span>now</span>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center justify-between gap-2.5">
          <Cap>decision log</Cap>
          <div className="flex flex-wrap gap-1">
            {(["all", ...LANES] as const).map((k) => (
              <Chip key={k} active={filter === k} color={k === "all" ? "var(--r-accent)" : LANE_COLOR[k]} onClick={() => setFilter(k)} className="h-6 px-[9px] text-[10.5px]">
                {k}
              </Chip>
            ))}
          </div>
        </div>
        <div className="flex flex-col overflow-hidden rounded-lg border border-r-line">
          {rows.map((d) => (
            <div key={d.id} className="grid grid-cols-[46px_70px_minmax(0,1fr)_auto] items-baseline gap-x-2.5 gap-y-1 border-b border-r-line px-3 py-[9px] last:border-b-0 sm:grid-cols-[58px_82px_minmax(0,1fr)_auto]">
              <span className="text-[10.5px] leading-[1.4] text-r-faint" title={new Date(d.at).toLocaleString()}>
                {new Date(d.at).toTimeString().slice(0, 5)}
              </span>
              <LaneLabel lane={d.lane} />
              <span className="flex min-w-0 flex-col gap-[3px]">
                <span className="truncate text-[11.5px] leading-[1.35] font-medium">
                  {fmtAnswer(d.answer)} <span className="font-normal text-r-faint">· {d.question.id}</span>
                </span>
                <span className="truncate text-[10.5px] leading-[1.35] text-r-muted" title={d.state}>
                  {d.state}
                </span>
              </span>
              <span className="flex flex-col items-end gap-[3px] text-[10.5px] leading-[1.35] whitespace-nowrap text-r-muted">
                <span>{fmtConf(d.confidence)}</span>
                <span>
                  {fmtMs(d.ms)}
                  {d.feedback === true && <span className="text-lane-reflex"> ✓</span>}
                  {d.feedback === false && <span className="text-lane-abstain"> ✕</span>}
                </span>
              </span>
            </div>
          ))}
          {!rows.length && <div className="px-3 py-[18px] text-center text-[11.5px] leading-normal text-r-muted">{decisions.length ? "No decisions in this lane." : emptyText}</div>}
        </div>
        {filtered.length > ROWS && (
          <button onClick={() => setMore((m) => !m)} className="self-start text-[10.5px] text-r-muted hover:text-r-text">
            {more ? "show fewer" : `show all ${filtered.length}`}
          </button>
        )}
      </div>
    </div>
  );
}
