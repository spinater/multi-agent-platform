"use client";

import { useState } from "react";

import { api } from "@/lib/api";
import type { Answer } from "@/lib/generated/Answer";
import type { DecisionFeedback } from "@/lib/generated/DecisionFeedback";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { Question } from "@/lib/generated/Question";

import { LANE_COLOR, THAI, type Axis, fmtAnswer, fmtConf, fmtMs, kindLabel, mix, type Mode, optionsOf, pctPos, pos, toLlm } from "./lib";
import { AxisTicks, Bar, Cap, Chip, LaneBadge, RxButton, errText } from "./ui";

const NOTICE: Partial<Record<Mode, (info: ProjectInfo) => string>> = {
  off: (i) => `Reflex is not installed. You can still ask, and ${toLlm(i.project.fallback)} will answer.`,
  nocorpus: (i) => `No corpus. Reflex has nothing to answer from, so every question goes to ${toLlm(i.project.fallback)}.`,
  blocked: (i) => `Reflex is paused: ${i.reflex.domains.length} domains, the limit is 8. Questions go to ${toLlm(i.project.fallback)}.`,
  error: (i) => `Reflex is not running, so every question goes to ${toLlm(i.project.fallback)} until it is restarted.`,
  noq: () => "No questions defined yet. Add one to ask Reflex.",
  shadow: (i) => `Not calibrated. Reflex scores every question in shadow, but ${toLlm(i.project.fallback)} answers until you calibrate.`,
};

type Result = { answer: Answer; text: string; question: Question; threshold: number | null };

/** 03 · Playground: ask one saved question about a pasted message and see the lane, the reason and the scores.
 * Feedback marks the logged decision right or wrong and adds the message as a calibration case. */
export function Playground({
  info,
  mode,
  axis,
  question,
  onPick,
  examples,
  onEditQuestions,
  onChanged,
}: {
  info: ProjectInfo;
  mode: Mode;
  axis: Axis;
  question: Question | null;
  onPick: (id: string) => void;
  examples: string[];
  onEditQuestions: () => void;
  onChanged: () => void;
}) {
  const p = info.project;
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<Result | null>(null);
  const [fb, setFb] = useState<{ correct: boolean | null; picking: boolean; msg: string; busy: boolean }>({ correct: null, picking: false, msg: "", busy: false });
  const thaiNow = THAI.test(input);

  const ask = async () => {
    const text = input.trim();
    if (!text || !question || busy) return;
    setBusy(true);
    setError("");
    try {
      const r = await api.decide(p.slug, { state: text, questions: [question], route: "auto" });
      setResult({ answer: r.answers[0], text, question, threshold: r.threshold });
      setFb({ correct: null, picking: false, msg: "", busy: false });
      if (r.reflex_error && mode === "live") setError(r.reflex_error);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const send = async (correct: boolean, expected?: string | boolean) => {
    if (!result) return;
    setFb((f) => ({ ...f, busy: true }));
    try {
      const r: DecisionFeedback = await api.decisionFeedback(p.slug, result.answer.decision_id, { correct, expected });
      const parts = [
        r.case ? (correct ? "Saved as a correct calibration case." : `Saved as a calibration case with the answer ${fmtAnswer(r.case.expected)}.`) : "Thanks, logged for review.",
        r.reflex_outcome === false ? "Reflex's own answer was wrong; its calibrator was told." : r.reflex_outcome ? "Reflex's own answer was right; its calibrator was told." : "",
        r.refit ? "Reflex's calibrator refitted." : "",
        r.reflex_error ? `(Reflex: ${r.reflex_error})` : "",
      ];
      setFb({ correct, picking: false, msg: parts.filter(Boolean).join(" "), busy: false });
      onChanged();
    } catch (e) {
      setFb((f) => ({ ...f, busy: false, msg: errText(e) }));
    }
  };

  const notice = NOTICE[mode]?.(info);
  return (
    <div className="flex flex-col gap-3">
      {notice && (
        <div className="rounded-lg border px-3 py-2.5 text-[11.5px] leading-normal text-pretty" style={{ borderColor: mix("var(--lane-llm)", 40), background: mix("var(--lane-llm)", 6) }}>
          {notice}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-1.5">
        {p.questions.map((q) => (
          <Chip
            key={q.id}
            active={q.id === question?.id}
            onClick={() => {
              onPick(q.id);
              setResult(null);
            }}
          >
            {q.id}
            <span className="text-r-faint"> · {kindLabel(q)}</span>
          </Chip>
        ))}
        <button onClick={onEditQuestions} className="h-7 px-1.5 text-[10.5px] text-r-muted underline-offset-2 hover:text-r-text hover:underline">
          {p.questions.length ? "edit questions" : "+ add a question"}
        </button>
      </div>
      {question && (
        <>
          <span className="text-[12.5px] leading-[1.4] font-medium">{question.prompt}</span>
          <span className="text-[10.5px] leading-[1.4] break-words text-r-faint">options: {optionsOf(question).map(fmtAnswer).join(" · ")}</span>
        </>
      )}
      <textarea
        rows={3}
        value={input}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void ask();
        }}
        placeholder="Paste an alert, a log line or a ticket in English"
        aria-label="Message to decide on"
        className="w-full resize-y rounded-lg border border-r-line2 bg-r-bg px-3 py-2.5 text-[12.5px] leading-[1.55] text-r-text outline-none placeholder:text-r-faint focus:border-r-accent"
      />
      {examples.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {examples.map((t) => (
            <button
              key={t}
              onClick={() => setInput(t)}
              className="h-6 max-w-full overflow-hidden rounded-[5px] border border-r-line px-[9px] text-[10.5px] text-ellipsis whitespace-nowrap text-r-muted hover:text-r-text"
            >
              try: {t}
            </button>
          ))}
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2.5">
        <RxButton tone="primary" size="lg" onClick={ask} disabled={!input.trim() || !question || busy}>
          {busy ? "Asking…" : "Ask Reflex"}
        </RxButton>
        <span className="text-[11px] leading-[1.4]" style={{ color: thaiNow ? "var(--lane-llm)" : "var(--r-muted)" }}>
          {thaiNow
            ? "Thai detected · Reflex reads English only, this will go to the LLM"
            : mode === "live"
              ? `answers locally when confidence ≥ ${fmtConf(p.threshold)}`
              : p.fallback === "none"
                ? "no fallback: below the threshold nobody answers"
                : "the LLM will answer"}
        </span>
      </div>
      {error && <p className="text-[11.5px] text-lane-abstain">{error}</p>}
      {result && <ResultCard r={result} axis={axis} fb={fb} onPicking={() => setFb((f) => ({ ...f, picking: !f.picking }))} onSend={send} />}
    </div>
  );
}

function ResultCard({
  r,
  axis,
  fb,
  onPicking,
  onSend,
}: {
  r: Result;
  axis: Axis;
  fb: { correct: boolean | null; picking: boolean; msg: string; busy: boolean };
  onPicking: () => void;
  onSend: (correct: boolean, expected?: string | boolean) => void;
}) {
  const a = r.answer;
  const c = LANE_COLOR[a.lane];
  const conf = a.confidence;
  const t = r.threshold;
  const options = optionsOf(r.question);
  // Reflex's probabilities: one per option (choice, score), or P(yes) for a yes/no question.
  const probs = a.probabilities ?? [];
  const scores = (r.question.kind === "noul" && probs.length === 1 ? [probs[0], 1 - probs[0]] : probs)
    .map((v, i) => ({ name: fmtAnswer(options[i] ?? `#${i + 1}`), v }))
    .sort((x, y) => y.v - x.v)
    .slice(0, 4);
  const top = Math.max(...scores.map((s) => s.v), 1e-9);
  const given = fb.correct !== null;
  return (
    <div className="flex flex-col gap-3.5 rounded-[10px] border p-3.5" style={{ borderColor: mix(c, 40), background: mix(c, 5) }}>
      <div className="flex flex-wrap items-center gap-x-3.5 gap-y-2.5">
        <LaneBadge lane={a.lane} />
        <span className="text-[18px] leading-tight font-semibold break-words">{fmtAnswer(a.answer)}</span>
        <span className="ml-auto text-[11px] leading-none font-medium whitespace-nowrap text-r-muted">{fmtMs(a.ms)}</span>
      </div>
      <span className="text-[11.5px] leading-normal text-pretty text-r-muted">
        {a.reason}
        {a.error && <span className="text-lane-abstain"> {a.error}</span>}
      </span>
      {conf !== null && (
        <div className="flex flex-col gap-1.5">
          <div className="flex justify-between gap-2.5">
            <Cap>confidence · log scale</Cap>
            <span className="text-[10px] leading-none text-r-text">
              {t === null ? `${fmtConf(conf)} · no threshold` : `${fmtConf(conf)} ${conf >= t ? "≥" : "<"} ${fmtConf(t)}`}
            </span>
          </div>
          <div className="relative h-[26px]" role="img" aria-label={`confidence ${fmtConf(conf)}${t === null ? "" : `, threshold ${fmtConf(t)}`}`}>
            <span className="absolute inset-x-0 top-3 h-0.5 bg-r-line2" />
            {t !== null && (
              <>
                <span className="absolute top-3 right-0 h-0.5 bg-lane-reflex opacity-60" style={{ left: pctPos(pos(t, axis)) }} />
                <span className="absolute top-1 bottom-1 w-0.5 bg-r-accent" style={{ left: pctPos(pos(t, axis)) }} />
              </>
            )}
            <span className="absolute top-1.5 -ml-[7px] size-3.5 rounded-full border-2 border-r-panel" style={{ left: pctPos(pos(conf, axis)), background: c }} />
          </div>
          <AxisTicks axis={axis} />
        </div>
      )}
      {scores.length > 1 && (
        <div className="flex flex-col gap-[5px]">
          <Cap>scores per option · probability</Cap>
          {scores.map((s, i) => (
            <div key={s.name} className="grid grid-cols-[minmax(0,130px)_minmax(0,1fr)_56px] items-center gap-2.5 text-[11px] leading-none">
              <span className="truncate" style={{ color: i === 0 ? c : "var(--r-faint)" }}>
                {s.name}
              </span>
              <Bar width={`${(s.v / top) * 100}%`} color={i === 0 ? c : "var(--r-faint)"} />
              <span className="text-right text-r-muted">{s.v.toFixed(2)}</span>
            </div>
          ))}
        </div>
      )}
      <div className="flex flex-col gap-2 border-t border-r-line pt-3">
        <div className="flex flex-wrap items-center gap-2">
          <Cap className="mr-1">{a.lane === "reflex" ? "Was Reflex right?" : "Was this answer right?"}</Cap>
          <button
            onClick={() => onSend(true)}
            disabled={given || fb.busy || a.answer === null || !a.decision_id}
            className="h-7 rounded-md border bg-transparent px-[11px] text-[11px] font-medium whitespace-nowrap disabled:opacity-50"
            style={{ borderColor: fb.correct === true ? "var(--lane-reflex)" : "var(--r-line2)" }}
          >
            ✓ Correct
          </button>
          <button
            onClick={onPicking}
            disabled={given || fb.busy || !a.decision_id}
            aria-expanded={fb.picking}
            className="h-7 rounded-md border bg-transparent px-[11px] text-[11px] font-medium whitespace-nowrap disabled:opacity-50"
            style={{ borderColor: fb.correct === false || fb.picking ? "var(--lane-abstain)" : "var(--r-line2)" }}
          >
            ✕ Wrong
          </button>
          <span className="text-[10.5px] leading-[1.4] text-r-muted">{fb.msg}</span>
        </div>
        {fb.picking && !given && (
          <div className="flex flex-wrap items-center gap-1.5">
            <Cap className="mr-1">correct answer</Cap>
            {options
              .filter((o) => o !== a.answer)
              .map((o) => (
                <RxButton key={String(o)} size="sm" disabled={fb.busy} onClick={() => onSend(false, o)}>
                  {fmtAnswer(o)}
                </RxButton>
              ))}
          </div>
        )}
      </div>
    </div>
  );
}
