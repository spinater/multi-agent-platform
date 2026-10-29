"use client";

import { useEffect, useState } from "react";

import { api } from "@/lib/api";
import type { CorpusLesson } from "@/lib/generated/CorpusLesson";

import { fmtConf } from "./lib";
import { Cap, RxButton, errText } from "./ui";

const ROWS = 30;

/** 06 · Lessons: what the corpus learned from the panel and the owner, newest first; Remove undoes a wrong one. */
export function Lessons({ slug, tick, toast }: { slug: string; tick: number; toast: (m: string) => void }) {
  const [lessons, setLessons] = useState<CorpusLesson[] | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const [more, setMore] = useState(false);

  useEffect(() => {
    let live = true;
    api
      .corpusLessons(slug)
      .then((l) => live && setLessons(l))
      .catch(() => live && setLessons([]));
    return () => {
      live = false;
    };
  }, [slug, tick]);

  const remove = async (l: CorpusLesson) => {
    setBusy(l.id);
    try {
      await api.removeLesson(slug, l.id);
      setLessons((all) => (all ?? []).filter((x) => x.id !== l.id));
      toast(`Lesson removed from ${l.domain}; Reflex restarts in a minute`);
    } catch (e) {
      toast(errText(e));
    } finally {
      setBusy(null);
    }
  };

  const all = lessons ?? [];
  const rows = more ? all : all.slice(0, ROWS);
  return (
    <div className="flex flex-col gap-2">
      <Cap>answers the panel agreed on or you confirmed become examples in the corpus · remove a wrong one</Cap>
      <div className="flex flex-col overflow-hidden rounded-lg border border-r-line">
        {rows.map((l) => (
          <div key={l.id} className="grid grid-cols-[58px_minmax(0,1fr)_auto] items-center gap-x-2.5 border-b border-r-line px-3 py-[9px] last:border-b-0">
            <span className="text-[10.5px] leading-[1.4] text-r-faint" title={new Date(l.at).toLocaleString()}>
              {new Date(l.at).toLocaleDateString(undefined, { day: "numeric", month: "short" })}
            </span>
            <span className="flex min-w-0 flex-col gap-[3px]">
              <span className="truncate text-[11.5px] leading-[1.35] font-medium">
                {l.domain} <span className="font-normal text-r-faint">· by {l.by} · decision {l.decision_id}</span>
              </span>
              <span className="truncate text-[10.5px] leading-[1.35] text-r-muted" title={l.example}>
                {l.example.replace(/^- /, "") || "(the decision's facts)"}
              </span>
              <span className="text-[10px] leading-[1.35] text-r-faint">
                Reflex said {l.reflex_answer ?? "nothing"} before ({fmtConf(l.confidence)}){l.correct ? " · already right" : ""}
              </span>
            </span>
            <RxButton size="sm" disabled={busy === l.id} onClick={() => remove(l)}>
              {busy === l.id ? "Removing…" : "Remove"}
            </RxButton>
          </div>
        ))}
        {!all.length && (
          <div className="px-3 py-[18px] text-center text-[11.5px] leading-normal text-r-muted">
            {lessons ? "Nothing learned yet. Answers the panel agrees on show up here." : "Loading…"}
          </div>
        )}
      </div>
      {all.length > ROWS && (
        <button onClick={() => setMore((m) => !m)} className="self-start text-[10.5px] text-r-muted hover:text-r-text">
          {more ? "show fewer" : `show all ${all.length}`}
        </button>
      )}
    </div>
  );
}
