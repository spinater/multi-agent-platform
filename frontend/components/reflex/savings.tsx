"use client";

import clsx from "clsx";
import { useEffect, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { SavingsDay } from "@/lib/generated/SavingsDay";
import type { SavingsReport } from "@/lib/generated/SavingsReport";
import type { SavingsRow } from "@/lib/generated/SavingsRow";
import type { TokenMethod } from "@/lib/generated/TokenMethod";
import { dur, fmtTokens } from "@/lib/use-usage";

import { useApp } from "../app-context";
import { Main, PageHeader } from "../shell/page";
import { LANE_COLOR, fmtMs, mix, pct } from "./lib";
import { Banner, Cap, Chip, ReflexTabs, type ReflexView, RxButton, RxSection, Tile, errText } from "./ui";

/** The windows the page offers, in hours. */
const WINDOWS: [number, string][] = [
  [24, "24 h"],
  [168, "7 d"],
  [720, "30 d"],
];

const METHOD: Record<TokenMethod, { label: string; color: string; note: string }> = {
  measured: { label: "measured", color: "var(--r-accent)", note: "from the API's reported usage" },
  estimated: { label: "estimated", color: "var(--r-muted)", note: "≈ 4 chars per token" },
  mixed: { label: "mixed", color: "var(--r-text)", note: "part measured, part ≈ 4 chars per token" },
};

/** Spent bars are hatched as well as yellow, so saved and spent differ by more than hue (green and yellow are close
 * for protan and deutan readers). */
const SPENT_FILL = `repeating-linear-gradient(135deg, ${LANE_COLOR.llm} 0 2px, ${mix(LANE_COLOR.llm, 45)} 2px 4px)`;

/** Tokens with `≈` unless every figure behind them was reported by the provider. */
const tokens = (n: number, method: TokenMethod) => `${method === "measured" ? "" : "≈"}${fmtTokens(n)}`;
const fmtTime = (ms: number | null) => (ms === null ? "—" : ms < 1000 ? fmtMs(ms) : dur(ms, true));
const fmtPct = (p: number | null) => (p === null ? "—" : `${Math.round(p)}%`);
const dayLabel = (day: string) => new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });

/** The Engine / Savings tabs in the Reflex screen's header. */
export function ViewTabs({ active }: { active: ReflexView }) {
  const { go } = useApp();
  return <ReflexTabs active={active} onPick={(v) => go("reflex", v === "savings" ? "savings" : null)} />;
}

/** Refetches when any project logs a decision (its live info's count changes). */
function useSavings(hours: number, project: string | null) {
  const { projects } = useApp();
  const key = projects.map((p) => `${p.project.slug}:${p.stats.total}`).join(",");
  const [report, setReport] = useState<SavingsReport | null>(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let live = true;
    api.savings(hours, project).then(
      (r) => {
        if (!live) return;
        setReport(r);
        setError("");
      },
      (e) => live && setError(errText(e)),
    );
    return () => {
      live = false;
    };
  }, [hours, project, key, tick]);
  return { report, error, retry: () => setTick((t) => t + 1) };
}

/** `#reflex/savings`: the LLM tokens Reflex saved, against what the fallbacks spent, per day and per project. */
export function Savings() {
  const { current } = useApp();
  const [hours, setHours] = useState(24);
  const [scope, setScope] = useState<"all" | "project">("all");
  const slug = scope === "project" && current ? current.project.slug : null;
  const { report, error, retry } = useSavings(hours, slug);
  const loading = !report || report.hours !== hours || (report.totals.slug || null) !== slug;
  const t = report?.totals;
  const windowLabel = WINDOWS.find(([h]) => h === hours)?.[1] ?? `${hours} h`;

  return (
    <Main className="gap-[18px]">
      <PageHeader
        kicker={`${slug ? `project · ${slug}` : "all projects"} · decision engine`}
        title="Savings"
        lead="Every question Reflex answers is a fallback call you don't make. This is what those calls would have cost in LLM tokens, next to what the fallbacks spent on the questions Reflex left to them."
        actions={<ViewTabs active="savings" />}
      />

      <div className="flex flex-wrap items-center gap-x-5 gap-y-2.5">
        <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label="Window">
          <Cap className="mr-1">window</Cap>
          {WINDOWS.map(([h, label]) => (
            <Chip key={h} active={hours === h} onClick={() => setHours(h)}>
              {label}
            </Chip>
          ))}
        </div>
        {current && (
          <div className="flex min-w-0 flex-wrap items-center gap-1.5" role="group" aria-label="Projects">
            <Cap className="mr-1">scope</Cap>
            <Chip active={scope === "all"} onClick={() => setScope("all")}>
              All projects
            </Chip>
            <Chip active={scope === "project"} onClick={() => setScope("project")} className="max-w-[200px] truncate">
              {current.project.name}
            </Chip>
          </div>
        )}
        <span className="text-[10.5px] text-r-faint" aria-live="polite">
          {error ? "" : loading ? "loading…" : `since ${new Date(report.since).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}`}
        </span>
      </div>

      {error && <Banner color="var(--lane-abstain)" title="Could not load the savings" text={error} actions={<RxButton onClick={retry}>Retry</RxButton>} />}

      {t && report && (
        <>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(150px,1fr))] gap-2">
            <Tile
              dot
              k="tokens saved"
              v={tokens(t.tokens_saved, t.method)}
              color="var(--lane-reflex)"
              sub={`${METHOD[t.method].label} · ${t.saved_pct === null ? "nothing to compare yet" : `${fmtPct(t.saved_pct)} of all fallback tokens`}`}
            />
            <Tile dot k="LLM calls avoided" v={String(t.llm_calls_avoided)} color="var(--lane-reflex)" vcolor="var(--r-text)" sub={`of ${t.decisions} decisions · ${windowLabel}`} />
            <Tile
              dot
              k="answered by Reflex"
              v={t.decisions ? pct(t.reflex / t.decisions) : "—"}
              color="var(--lane-reflex)"
              vcolor="var(--r-text)"
              sub={`${t.reflex} reflex · ${t.llm} llm · ${t.abstain} abstain`}
            />
            <Tile
              dot
              k="time saved"
              v={fmtTime(t.time_saved_ms)}
              color="var(--r-accent)"
              vcolor="var(--r-text)"
              sub={t.avg_ms_llm === null ? "no LLM answers to compare with" : `~${fmtMs(t.avg_ms_reflex ?? 0)} reflex vs ~${fmtMs(t.avg_ms_llm)} llm`}
            />
            <Tile dot k="spent on the fallback" v={tokens(t.tokens_spent, t.method)} color="var(--lane-llm)" sub={`${t.llm} llm answer${t.llm === 1 ? "" : "s"}`} />
          </div>

          <RxSection id="sv-days" n="01" title="Saved vs spent" meta={`tokens per UTC day · ${windowLabel}`}>
            <DayChart days={report.days} method={t.method} />
          </RxSection>

          <RxSection id="sv-projects" n="02" title="By project" meta={`${report.projects.length} project${report.projects.length === 1 ? "" : "s"} · ${windowLabel}`}>
            <ProjectTable rows={report.projects} current={current?.project.slug ?? null} />
          </RxSection>

          <RxSection id="sv-how" n="03" title="How this is counted">
            <HowCounted report={report} />
          </RxSection>
        </>
      )}
    </Main>
  );
}

function Legend() {
  return (
    <span className="flex gap-3 text-[10px] tracking-[0.08em] text-r-muted uppercase">
      <span className="flex items-center gap-[5px]">
        <span className="size-2 rounded-sm" style={{ background: LANE_COLOR.reflex }} />
        saved
      </span>
      <span className="flex items-center gap-[5px]">
        <span className="size-2 rounded-sm" style={{ background: SPENT_FILL }} />
        spent
      </span>
    </span>
  );
}

/** Saved (solid green, left) and spent (hatched yellow, right) per day; hover or tap a day for its figures. */
function DayChart({ days, method }: { days: SavingsDay[]; method: TokenMethod }) {
  const [hover, setHover] = useState<number | null>(null);
  const max = Math.max(0, ...days.map((d) => Math.max(d.saved, d.spent)));
  const shown = hover === null ? null : days[hover];
  const h = (n: number) => (n > 0 ? `max(2px, ${(n / max) * 100}%)` : "0");
  const labels = days.length <= 2 ? days.map((_, i) => i) : [0, Math.floor((days.length - 1) / 2), days.length - 1];
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2.5">
        <span className="min-h-[14px] text-[11px] leading-tight text-r-muted">
          {shown ? (
            <>
              <span className="text-r-text">{dayLabel(shown.day)}</span> · saved {tokens(shown.saved, method)} · spent {tokens(shown.spent, method)} · {shown.reflex} reflex ·{" "}
              {shown.llm} llm
            </>
          ) : max > 0 ? (
            `peak ${tokens(max, method)} tokens in a day · hover a day for its figures`
          ) : (
            ""
          )}
        </span>
        <Legend />
      </div>
      <div
        className="relative flex h-[150px] items-end gap-[3px] rounded-lg border border-r-line px-2 pt-2"
        role="img"
        aria-label={days.map((d) => `${dayLabel(d.day)}: ${d.saved} saved, ${d.spent} spent`).join("; ")}
        onMouseLeave={() => setHover(null)}
      >
        {max === 0 && <span className="absolute inset-0 flex items-center justify-center text-[11.5px] text-r-muted">No reflex or llm answers in this window.</span>}
        {days.map((d, i) => (
          <div
            key={d.day}
            onMouseEnter={() => setHover(i)}
            onClick={() => setHover(hover === i ? null : i)}
            className={clsx("flex h-full min-w-0 flex-1 items-end justify-center gap-[2px] rounded-t-[3px]", hover === i && "bg-r-line")}
          >
            <span className="w-full max-w-[26px] rounded-t-[3px]" style={{ height: h(d.saved), background: LANE_COLOR.reflex }} />
            <span className="w-full max-w-[26px] rounded-t-[3px]" style={{ height: h(d.spent), background: SPENT_FILL }} />
          </div>
        ))}
      </div>
      <div className="relative h-3 text-[9.5px] leading-none text-r-faint" aria-hidden>
        {labels.map((i, k) => (
          <span
            key={days[i].day}
            className="absolute whitespace-nowrap"
            style={k === 0 ? { left: 0 } : k === labels.length - 1 ? { right: 0 } : { left: `${((i + 0.5) / days.length) * 100}%`, transform: "translateX(-50%)" }}
          >
            {dayLabel(days[i].day)}
          </span>
        ))}
      </div>
    </div>
  );
}

function MethodBadge({ method }: { method: TokenMethod }) {
  const m = METHOD[method];
  return (
    <span
      title={m.note}
      className="inline-flex h-5 items-center rounded-[4px] border px-1.5 text-[9.5px] font-semibold tracking-[0.12em] whitespace-nowrap uppercase"
      style={{ borderColor: mix(m.color, 55), color: m.color }}
    >
      {m.label}
    </span>
  );
}

/** reflex / llm / abstain as one thin split bar. */
function LaneSplit({ r }: { r: SavingsRow }) {
  const total = r.reflex + r.llm + r.abstain;
  return (
    <span className="flex min-w-0 flex-col gap-1">
      <span className="text-[11px] whitespace-nowrap">
        <span className="text-lane-reflex">{r.reflex}</span> · <span className="text-lane-llm">{r.llm}</span> · <span className="text-lane-abstain">{r.abstain}</span>
      </span>
      <span className="flex h-1 gap-px overflow-hidden rounded-sm bg-r-line">
        {total > 0 &&
          (["reflex", "llm", "abstain"] as const).map((l) => (r[l] ? <span key={l} style={{ width: `${(100 * r[l]) / total}%`, background: LANE_COLOR[l] }} /> : null))}
      </span>
    </span>
  );
}

const COLS = "lg:grid-cols-[minmax(140px,1.6fr)_76px_minmax(110px,1fr)_56px_84px_84px_64px_100px]";

function Head({ children, right }: { children: ReactNode; right?: boolean }) {
  return <Cap className={clsx(right && "text-right")}>{children}</Cap>;
}

/** One row per project: a table on wide screens, a stacked card below `lg`. */
function ProjectTable({ rows, current }: { rows: SavingsRow[]; current: string | null }) {
  const sorted = [...rows].sort((a, b) => b.tokens_saved - a.tokens_saved || b.decisions - a.decisions || a.name.localeCompare(b.name));
  if (!sorted.length) return <p className="text-[11.5px] text-r-muted">No projects yet.</p>;
  return (
    <div className="flex flex-col overflow-hidden rounded-lg border border-r-line">
      <div className={clsx("hidden items-end gap-x-3 border-b border-r-line px-3 py-2 lg:grid", COLS)}>
        <Head>project</Head>
        <Head right>decisions</Head>
        <Head>reflex · llm · abstain</Head>
        <Head right>thai</Head>
        <Head right>saved</Head>
        <Head right>spent</Head>
        <Head right>saved %</Head>
        <Head right>method</Head>
      </div>
      {sorted.map((r) => {
        const idle = r.decisions === 0;
        const name = (
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="truncate text-[12px] font-semibold">{r.name}</span>
            <span className="truncate text-[10px] text-r-faint">{r.slug}</span>
            {r.slug === current && <span className="text-[9.5px] tracking-[0.12em] text-r-accent uppercase">current</span>}
          </span>
        );
        return (
          <div key={r.slug} className={clsx("border-b border-r-line px-3 py-2.5 last:border-b-0", idle && "opacity-60")}>
            {/* wide: one grid row */}
            <div className={clsx("hidden items-center gap-x-3 text-[11.5px] lg:grid", COLS)}>
              {name}
              <span className="text-right">{r.decisions}</span>
              <LaneSplit r={r} />
              <span className="text-right" title="Thai input never reaches Reflex">
                {r.skipped_thai}
              </span>
              <span className="text-right font-semibold text-lane-reflex">{tokens(r.tokens_saved, r.method)}</span>
              <span className="text-right">{tokens(r.tokens_spent, r.method)}</span>
              <span className="text-right">{fmtPct(r.saved_pct)}</span>
              <span className="flex justify-end">
                <MethodBadge method={r.method} />
              </span>
            </div>
            {/* narrow: a card */}
            <div className="flex flex-col gap-2 lg:hidden">
              <div className="flex items-center justify-between gap-2">
                {name}
                <MethodBadge method={r.method} />
              </div>
              <div className="grid grid-cols-3 gap-2 text-[11.5px]">
                <Figure k="saved" v={<span className="font-semibold text-lane-reflex">{tokens(r.tokens_saved, r.method)}</span>} />
                <Figure k="spent" v={tokens(r.tokens_spent, r.method)} />
                <Figure k="saved %" v={fmtPct(r.saved_pct)} />
                <Figure k="decisions" v={String(r.decisions)} />
                <Figure k="lanes" v={<LaneSplit r={r} />} />
                <Figure k="thai" v={String(r.skipped_thai)} />
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function Figure({ k, v }: { k: string; v: ReactNode }) {
  return (
    <span className="flex min-w-0 flex-col gap-1">
      <Cap className="truncate">{k}</Cap>
      <span className="min-w-0">{v}</span>
    </span>
  );
}

function HowCounted({ report }: { report: SavingsReport }) {
  const overhead = Object.entries(report.overhead_tokens).filter(([, n]) => n > 0);
  const thai = report.totals.skipped_thai;
  const items: [string, ReactNode][] = [
    [
      "Saved",
      <>
        Each answer Reflex gave is a fallback call that was not made, priced at what its prompt would have cost. When a project&apos;s API fallback reported its
        usage on 3 or more calls in the last 30 days, that is their mean tokens per call, scaled by this prompt&apos;s length (<b>measured</b>). Otherwise it
        is ceil(prompt chars / 4) + 16 reply tokens (<b>estimated</b>). A project without a fallback is priced the same, as if its questions had gone to an LLM.
      </>,
    ],
    [
      "Spent",
      <>
        Input plus output tokens of each answer the fallback gave. The API fallback reports them; headless CLIs (<code>claude -p</code>, <code>codex exec</code>,{" "}
        <code>agy -p</code>) report nothing, so theirs are ceil(chars / 4) of the prompt and the reply. Fallback replies that matched no option are counted as
        abstentions, not here.
      </>,
    ],
    [
      "CLI overhead",
      overhead.length ? (
        <>
          A headless CLI call also sends its own system prompt and tools. <code>[reflex] fallback_overhead_tokens</code> adds{" "}
          {overhead.map(([f, n]) => `${fmtTokens(n)} per ${f} call`).join(", ")} to both saved and spent.
        </>
      ) : (
        <>
          A headless CLI call also sends its own system prompt and tools, often thousands of tokens, which these figures leave out. Set{" "}
          <code>[reflex] fallback_overhead_tokens</code> in <code>map.toml</code> to count it, in both saved and spent.
        </>
      ),
    ],
    [
      "Thai",
      <>
        Thai text never reaches Reflex, which reads English only, so those decisions go straight to the fallback and cannot save anything
        {thai ? ` (${thai} in this window)` : ""}.
      </>,
    ],
    ["Time", <>Reflex answers × (mean LLM time − mean Reflex time), per project, from this window&apos;s own timings.</>],
    ["Older decisions", <>Decisions logged before tokens were recorded are estimated from their stored message, which was cut at 1000 characters.</>],
  ];
  return (
    <dl className="grid gap-x-4 gap-y-2.5 sm:grid-cols-[140px_minmax(0,1fr)]">
      {items.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-[10px] leading-[1.6] font-medium tracking-[0.12em] text-r-muted uppercase">{k}</dt>
          <dd className="text-[11.5px] leading-[1.6] text-pretty text-r-text">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
