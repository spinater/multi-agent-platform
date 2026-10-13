"use client";

import { useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { Question } from "@/lib/generated/Question";

import { Modal } from "../shell/page";
import { RxButton, errText } from "./ui";

const field = "w-full rounded-md border border-r-line2 bg-r-bg px-2.5 text-[12px] text-r-text outline-none placeholder:text-r-faint focus:border-r-accent";

/** A starter question that routes a message to one of the corpus domains: a sensible first question for any corpus. */
export const starterQuestion = (domains: string[]): Question => ({
  id: "domain",
  kind: "choice",
  prompt: "Which area of the corpus does this message belong to?",
  options: domains.length >= 2 ? domains : ["first-option", "second-option"],
});

const clean = (qs: Question[]): Question[] =>
  qs.map((q) => ({
    ...q,
    id: q.id.trim(),
    prompt: q.prompt.trim(),
    options: q.kind === "noul" ? undefined : (q.options ?? []).map((o) => o.trim()).filter(Boolean),
    criteria: q.criteria?.trim() || undefined,
  }));

function Labeled({ text, children, className }: { text: string; children: ReactNode; className?: string }) {
  return (
    <label className={`flex min-w-0 flex-col gap-1 text-[9.5px] font-medium tracking-[0.14em] text-r-muted uppercase ${className ?? ""}`}>
      {text}
      {children}
    </label>
  );
}

/** Edits the project's saved questions: fixed answers only (choice, score, yes-no), as Reflex requires. */
export function QuestionsModal({ slug, initial, domains, onClose, onSaved }: { slug: string; initial: Question[]; domains: string[]; onClose: () => void; onSaved: () => void }) {
  const [qs, setQs] = useState<Question[]>(initial.length ? initial : [starterQuestion(domains)]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const set = (i: number, patch: Partial<Question>) => setQs((list) => list.map((q, j) => (j === i ? { ...q, ...patch } : q)));

  const save = async () => {
    setBusy(true);
    setError("");
    try {
      await api.updateProject(slug, { questions: clean(qs) });
      onSaved();
      onClose();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal label="Questions" onClose={onClose} width={720}>
      <div className="reflex-theme flex flex-col gap-4 rounded-[14px] bg-r-panel p-5 text-r-text">
        <div className="flex flex-col gap-1.5">
          <h2 className="text-[11px] font-semibold tracking-[0.16em] uppercase">
            <span className="text-r-accent">■</span>&nbsp;&nbsp;Questions
          </h2>
          <p className="text-[11.5px] leading-normal text-r-muted">
            Each question has fixed answers: choice (2+ options), score (2+ levels, lowest first) or yes-no. Reflex never writes free text. Options that name the corpus domains
            route best.
          </p>
        </div>
        {qs.map((q, i) => (
          <div key={i} className="flex flex-col gap-2.5 rounded-[10px] border border-r-line bg-r-panel2 p-3">
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-[8rem_8rem_minmax(0,1fr)_auto] sm:items-end">
              <Labeled text="id">
                <input className={`${field} h-8`} value={q.id} onChange={(e) => set(i, { id: e.target.value })} />
              </Labeled>
              <Labeled text="kind">
                <select
                  className={`${field} h-8`}
                  value={q.kind}
                  onChange={(e) => set(i, { kind: e.target.value as Question["kind"], options: e.target.value === "noul" ? undefined : (q.options ?? ["", ""]) })}
                >
                  <option value="choice">choice</option>
                  <option value="score">score</option>
                  <option value="noul">yes-no</option>
                </select>
              </Labeled>
              <Labeled text="prompt">
                <input className={`${field} h-8`} value={q.prompt} onChange={(e) => set(i, { prompt: e.target.value })} />
              </Labeled>
              <RxButton aria-label={`Remove question ${q.id}`} onClick={() => setQs((list) => list.filter((_, j) => j !== i))}>
                ✕
              </RxButton>
            </div>
            {q.kind !== "noul" && (
              <Labeled text={`options, comma separated${q.kind === "score" ? " · lowest first" : ""}`}>
                <input className={`${field} h-8`} value={(q.options ?? []).join(", ")} onChange={(e) => set(i, { options: e.target.value.split(",").map((s) => s.trimStart()) })} />
              </Labeled>
            )}
            <Labeled text="criteria (optional) · what decides between the options">
              <input className={`${field} h-8`} value={q.criteria ?? ""} onChange={(e) => set(i, { criteria: e.target.value || undefined })} />
            </Labeled>
          </div>
        ))}
        <div className="flex flex-wrap items-center gap-2">
          <RxButton onClick={() => setQs((list) => [...list, { id: `q${list.length + 1}`, kind: "choice", prompt: "", options: ["", ""] }])}>+ question</RxButton>
          {domains.length >= 2 && !qs.some((q) => q.id === "domain") && <RxButton onClick={() => setQs((list) => [...list, starterQuestion(domains)])}>+ route to a domain</RxButton>}
          <span className="flex-1" />
          <RxButton onClick={onClose}>Cancel</RxButton>
          <RxButton tone="primary" onClick={save} disabled={busy}>
            {busy ? "Saving…" : "Save questions"}
          </RxButton>
        </div>
        {error && <p className="text-[11.5px] text-lane-abstain">{error}</p>}
      </div>
    </Modal>
  );
}
