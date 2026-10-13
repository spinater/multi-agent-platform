"use client";

import { useEffect, useRef, useState } from "react";

import { api } from "@/lib/api";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { Question } from "@/lib/generated/Question";
import type { ReflexCase } from "@/lib/generated/ReflexCase";

import { MIN_CASES, THAI, type Axis, fmtAnswer, fmtConf, fmtLog, hash, metrics, optionsOf, pct, pctPos, posLog, scored, suggest } from "./lib";
import { AxisTicks, Cap, RxButton, Tile, errText } from "./ui";

/** Parses `message | answer` lines for one question. Answers must be one of its options (yes/no for yes-no). */
export function parseCases(text: string, q: Question): { cases: { question_id: string; text: string; expected: string | boolean }[]; errors: string[] } {
  const cases: { question_id: string; text: string; expected: string | boolean }[] = [];
  const errors: string[] = [];
  const options = optionsOf(q);
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const i = line.lastIndexOf("|");
    if (i < 0) {
      errors.push(`missing "|": ${line}`);
      continue;
    }
    const msg = line.slice(0, i).trim();
    const ans = line.slice(i + 1).trim();
    const yes = /^(yes|y|true)$/i.test(ans);
    const no = /^(no|n|false)$/i.test(ans);
    const expected = q.kind === "noul" ? (yes ? true : no ? false : null) : (options.find((o) => String(o).toLowerCase() === ans.toLowerCase()) ?? null);
    if (!msg) errors.push(`no message: ${line}`);
    else if (THAI.test(msg)) errors.push(`Thai input always goes to the LLM, so it cannot calibrate Reflex: ${msg}`);
    else if (expected === null) errors.push(`"${ans}" is not one of ${options.map(fmtAnswer).join(", ")}`);
    else cases.push({ question_id: q.id, text: msg, expected });
  }
  return { cases, errors };
}

/** 04 · Calibrate: the cases on the log axis, a draggable threshold with live tiles, and the case list. */
export function Calibrate({
  info,
  can,
  cases,
  axis,
  question,
  onChanged,
  toast,
}: {
  info: ProjectInfo;
  can: boolean;
  cases: ReflexCase[];
  axis: Axis;
  question: Question | null;
  onChanged: () => void;
  toast: (text: string, tone?: "ok" | "warn" | "bad") => void;
}) {
  const p = info.project;
  const saved = p.threshold === null ? null : Math.log10(p.threshold);
  const calibrated = saved !== null;
  const known = new Set(p.questions.map((q) => q.id));
  const usable = cases.filter((c) => known.has(c.question_id));
  const pts = scored(usable);
  const n = usable.length;
  const [draft, setDraft] = useState<number | null>(null);
  const [busy, setBusy] = useState("");
  const [paste, setPaste] = useState("");
  const [showList, setShowList] = useState(false);
  const t = draft ?? saved ?? suggest(pts, axis) ?? -4;
  const M = metrics(t, pts);
  const unsaved = calibrated && Math.abs(t - saved) > 0.004;

  // A threshold saved elsewhere (fit, settings, another tab) replaces the draft.
  const [seen, setSeen] = useState(p.threshold);
  if (seen !== p.threshold) {
    setSeen(p.threshold);
    setDraft(null);
  }

  const track = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  const canDrag = can && pts.length > 0;
  const trackAt = useRef<(x: number) => void>(() => {});
  useEffect(() => {
    const at = (x: number) => {
      const el = track.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      const f = Math.max(0, Math.min(1, (x - r.left) / r.width));
      setDraft(Math.round((axis.lo + (axis.hi - axis.lo) * f) * 100) / 100);
    };
    const move = (e: PointerEvent) => dragging.current && at(e.clientX);
    const up = () => (dragging.current = false);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    trackAt.current = at;
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, [axis.lo, axis.hi]);

  const run = async (label: string, f: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await f();
      onChanged();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy("");
    }
  };

  const fit = () =>
    run("fit", async () => {
      const r = await api.fitCases(p.slug, { rho: 0.3, apply: true });
      setDraft(null);
      toast(`Threshold ${fmtConf(r.threshold)} from ${r.n} cases · Reflex answers ${pct(r.coverage)} of them${r.pass_precision === null ? "" : `, ${pct(r.pass_precision)} correct`}`);
    });
  const save = () =>
    run("save", async () => {
      const v = Number((10 ** t).toPrecision(2));
      await api.updateProject(p.slug, { threshold: v });
      setDraft(null);
      toast(`Threshold saved: ${fmtConf(v)}`);
    });
  const add = () =>
    run("add", async () => {
      if (!question) throw new Error("pick a question in the playground first");
      const { cases: parsed, errors } = parseCases(paste, question);
      if (errors.length) throw new Error(errors.slice(0, 3).join(" · "));
      if (!parsed.length) throw new Error("one case per line: message | correct answer");
      const added = await api.addCases(p.slug, parsed);
      setPaste("");
      toast(`${added.length} case${added.length === 1 ? "" : "s"} added for ${question.id}${added.some((c) => !c.score) ? " · Reflex could not score some yet" : ""}`);
    });

  const btn = "transition-opacity";
  const actions = calibrated ? (
    <>
      <RxButton className={btn} disabled={!canDrag} onClick={() => setDraft(suggest(pts, axis) ?? t)}>
        Suggest (≤ 5% wrong)
      </RxButton>
      <RxButton tone="primary" className={btn} disabled={!unsaved || !!busy} onClick={save}>
        {busy === "save" ? "Saving…" : "Save threshold"}
      </RxButton>
      <RxButton className={btn} disabled={!can || n < MIN_CASES || !!busy} onClick={fit} title="Re-score every case and refit the ρ = 0.3 quantile">
        {busy === "fit" ? "Fitting…" : "Refit from cases"}
      </RxButton>
    </>
  ) : (
    <RxButton tone="primary" disabled={!can || n < MIN_CASES || !!busy} onClick={fit}>
      {busy === "fit" ? "Calibrating…" : "Run calibration"}
    </RxButton>
  );
  const actionNote = !can
    ? ""
    : calibrated
      ? "Saving changes the gate for new questions only."
      : n < MIN_CASES
        ? `Need ${MIN_CASES - n} more case${MIN_CASES - n === 1 ? "" : "s"}.`
        : "Ready: Reflex scores every case and sets the threshold to the 30% quantile of its confidence on the ones it got right.";
  const draftText = !can ? "set up steps 1–3 first" : !calibrated ? "not calibrated · every question goes to the LLM" : unsaved ? "draft · drag, then save" : "saved · live traffic uses this";
  const draftColor = unsaved ? "var(--r-accent)" : !calibrated ? "var(--lane-llm)" : "var(--r-muted)";
  const countColor = n >= MIN_CASES ? "var(--lane-reflex)" : "var(--lane-llm)";
  const tPos = pctPos(posLog(t, axis));
  const qOf = (id: string) => (id === question?.id ? "" : ` · ${id}`);

  return (
    <div className="flex flex-col gap-3.5">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(120px,1fr))] gap-2">
        <Tile k="LLM calls saved" v={n ? pct(M.saved) : "—"} color="var(--lane-reflex)" sub={`${M.answered} of ${pts.length} cases above threshold`} />
        <Tile k="Reflex wrong" v={M.answered ? pct(M.wrong) : "—"} color={M.wrong > 0.05 ? "var(--lane-abstain)" : "var(--r-text)"} sub={`${M.wrongN} wrong answer${M.wrongN === 1 ? "" : "s"} would ship`} />
        <Tile k="Still to LLM" v={pts.length ? pct(1 - M.saved) : "—"} color="var(--lane-llm)" sub={`${pts.length - M.answered} cases below threshold or abstained`} />
        <Tile k="Threshold" v={fmtLog(t)} color="var(--r-accent)" sub={!calibrated ? "not saved yet" : unsaved ? `draft · saved ${fmtLog(saved)}` : "saved"} />
      </div>

      <div className="flex flex-col gap-1.5">
        <div className="flex flex-wrap justify-between gap-2.5">
          <Cap>confidence of each case · log scale</Cap>
          <span className="text-[10px] leading-tight" style={{ color: draftColor }}>
            {draftText}
          </span>
        </div>
        <div
          ref={track}
          role="slider"
          aria-label="Threshold"
          aria-valuemin={axis.lo}
          aria-valuemax={axis.hi}
          aria-valuenow={t}
          aria-valuetext={fmtLog(t)}
          tabIndex={canDrag ? 0 : -1}
          onKeyDown={(e) => {
            if (!canDrag) return;
            if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
              e.preventDefault();
              setDraft(Math.round((t + (e.key === "ArrowRight" ? 0.01 : -0.01) * (e.shiftKey ? 10 : 1)) * 100) / 100);
            }
          }}
          onPointerDown={(e) => {
            if (!canDrag) return;
            dragging.current = true;
            trackAt.current(e.clientX);
          }}
          className="relative h-[150px] touch-none overflow-hidden rounded-lg border border-r-line select-none focus-visible:outline-2 focus-visible:outline-r-accent"
          style={{ cursor: canDrag ? "ew-resize" : "not-allowed" }}
        >
          <span className="absolute inset-y-0 left-0" style={{ width: tPos, background: "color-mix(in srgb, var(--lane-llm) 9%, transparent)" }} />
          <span className="absolute inset-y-0 right-0" style={{ left: tPos, background: "color-mix(in srgb, var(--lane-reflex) 10%, transparent)" }} />
          <span className="absolute inset-x-0 top-1/2 h-px bg-r-line" />
          <span className="absolute top-2 left-2 text-[9.5px] leading-none font-medium tracking-[0.14em] text-r-muted">CORRECT</span>
          <span className="absolute bottom-2 left-2 text-[9.5px] leading-none font-medium tracking-[0.14em] text-r-muted">WRONG · ABSTAINED</span>
          <span className="absolute top-2 right-2 text-[9.5px] leading-none font-semibold tracking-[0.14em] text-lane-reflex">→ REFLEX</span>
          <span className="absolute top-2 left-[84px] text-[9.5px] leading-none font-semibold tracking-[0.14em] text-lane-llm">LLM ←</span>
          {pts.map((d) => {
            const h = hash(`y${d.c.id}`);
            const top = d.ok ? 18 + h * 48 : 94 + h * 40;
            const ring = d.ok ? "var(--r-text)" : d.answered ? "var(--lane-abstain)" : "var(--r-faint)";
            const title = `${d.c.text} · ${fmtConf(d.c.score!.confidence)} · ${d.ok ? "correct" : d.answered ? `wrong (said ${fmtAnswer(d.c.score!.answer)})` : "abstained"}`;
            return (
              <span
                key={d.c.id}
                title={title}
                className="absolute -mt-1 -ml-1 size-[9px] rounded-full border-[1.5px]"
                style={{ left: pctPos(posLog(d.l, axis)), top, background: d.ok ? "var(--r-text)" : "transparent", borderColor: ring }}
              />
            );
          })}
          <span className="absolute inset-y-0 -ml-px w-0.5 bg-r-accent" style={{ left: tPos }} />
          <span
            className="absolute top-1/2 flex h-[22px] -translate-x-1/2 -translate-y-1/2 items-center rounded-[5px] bg-r-accent px-2 text-[10.5px] leading-none font-semibold whitespace-nowrap text-r-accent-ink"
            style={{ left: tPos }}
          >
            ⇔ {fmtLog(t)}
          </span>
        </div>
        <AxisTicks axis={axis} />
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {actions}
        <span className="text-[10.5px] leading-[1.4] text-r-muted">{actionNote}</span>
      </div>

      <div className="flex flex-col gap-2 border-t border-r-line pt-3">
        <div className="flex flex-wrap items-center justify-between gap-2.5">
          <Cap>calibration cases · {n} cases</Cap>
          <span className="text-[10.5px] leading-none" style={{ color: countColor }}>
            {n >= MIN_CASES ? "enough to calibrate" : `${MIN_CASES - n} more needed`}
          </span>
        </div>
        <span className="flex h-1 overflow-hidden rounded-sm bg-r-line">
          <span className="h-full" style={{ width: `${Math.min(100, (n / MIN_CASES) * 100)}%`, background: countColor }} />
        </span>
        <textarea
          rows={3}
          value={paste}
          onChange={(e) => setPaste(e.target.value)}
          disabled={!can}
          aria-label="Cases to add"
          placeholder={`one case per line:  message | correct answer${question ? `\nfor ${question.id}: ${optionsOf(question).map(fmtAnswer).join(" / ")}` : ""}`}
          className="w-full resize-y rounded-lg border border-r-line2 bg-r-bg px-3 py-2.5 text-[12px] leading-[1.55] text-r-text outline-none placeholder:text-r-faint focus:border-r-accent disabled:opacity-50"
        />
        <div className="flex flex-wrap items-center gap-2">
          <RxButton size="md" disabled={!can || !paste.includes("|") || !!busy} onClick={add}>
            {busy === "add" ? "Adding…" : "Add pasted cases"}
          </RxButton>
          <RxButton size="md" disabled={!cases.length} onClick={() => setShowList((s) => !s)} aria-expanded={showList}>
            {showList ? "Hide cases" : `Show ${cases.length} cases`}
          </RxButton>
          {question && <span className="text-[10.5px] text-r-faint">pasted cases are for {question.id}</span>}
        </div>
        {showList && (
          <ul className="flex max-h-72 flex-col overflow-y-auto rounded-lg border border-r-line">
            {[...cases].reverse().map((c) => (
              <li key={c.id} className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-baseline gap-2.5 border-b border-r-line px-3 py-2 last:border-b-0">
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className="truncate text-[11.5px]">{c.text}</span>
                  <span className="truncate text-[10.5px] text-r-faint">
                    {fmtAnswer(c.expected)}
                    {qOf(c.question_id)}
                    {!known.has(c.question_id) && " · question removed"}
                  </span>
                </span>
                <span className="text-right text-[10.5px] whitespace-nowrap text-r-muted">
                  {c.score ? (
                    <>
                      {fmtConf(c.score.confidence)}{" "}
                      <span style={{ color: c.score.correct ? "var(--lane-reflex)" : c.score.answer === null ? "var(--r-faint)" : "var(--lane-abstain)" }}>
                        {c.score.correct ? "✓" : c.score.answer === null ? "abstain" : `✕ ${fmtAnswer(c.score.answer)}`}
                      </span>
                    </>
                  ) : (
                    "not scored"
                  )}
                </span>
                <button
                  aria-label={`Delete case ${c.text}`}
                  className="text-[12px] text-r-faint hover:text-lane-abstain"
                  onClick={() => run("delete", () => api.deleteCase(p.slug, c.id))}
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
