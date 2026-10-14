"use client";

/** 06 · Thai words (backend thai.rs): Reflex reads English words only, so MAP glosses Thai into English with a
 * built-in glossary plus the project's own words. Shows what recent Thai decisions did not cover, the project's words
 * as an editable table, and a live preview of what Reflex reads. */

import { Fragment, useEffect, useMemo, useState } from "react";

import { api } from "@/lib/api";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ThaiPreview } from "@/lib/generated/ThaiPreview";
import type { ThaiReport } from "@/lib/generated/ThaiReport";

import { mix, pct } from "./lib";
import { Cap, RxButton, errText } from "./ui";

/** Wait this long after the last keystroke before previewing. */
const PREVIEW_DEBOUNCE_MS = 350;

type Row = { th: string; en: string };

const toRows = (words: Record<string, string>): Row[] => Object.entries(words).map(([th, en]) => ({ th, en }));
/** The table as the API takes it; half-filled rows are left out of a preview (the server would refuse them). */
function toWords(rows: Row[], complete = false): Record<string, string> {
  const out: Record<string, string> = {};
  for (const r of rows) {
    const th = r.th.trim();
    const en = r.en.trim();
    if (!th && !en) continue;
    if (complete && (!th || !en)) continue;
    out[th] = en;
  }
  return out;
}
const sameWords = (a: Record<string, string>, b: Record<string, string>) => {
  const ka = Object.keys(a);
  return ka.length === Object.keys(b).length && ka.every((k) => b[k] === a[k]);
};

const field = "h-7 w-full min-w-0 rounded-md border border-r-line2 bg-r-bg px-2 text-[12px] text-r-text outline-none placeholder:text-r-faint focus:border-r-accent";

export function ThaiWords({ info, toast }: { info: ProjectInfo; toast: (text: string, tone?: "ok" | "warn" | "bad") => void }) {
  const slug = info.project.slug;
  const [report, setReport] = useState<ThaiReport | null>(null);
  const [loadError, setLoadError] = useState("");
  const [rows, setRows] = useState<Row[] | null>(null);
  const [saveError, setSaveError] = useState("");
  const [busy, setBusy] = useState(false);
  const [adds, setAdds] = useState<Record<string, string>>({});

  // A new decision changes `info`: read the report again (the table keeps unsaved edits).
  useEffect(() => {
    let live = true;
    api.thai(slug).then(
      (r) => {
        if (!live) return;
        setReport(r);
        setLoadError("");
      },
      (e) => live && setLoadError(errText(e)),
    );
    return () => {
      live = false;
    };
  }, [slug, info]);

  const saved = useMemo(() => report?.words ?? {}, [report]);
  const table = rows ?? toRows(saved);
  const words = toWords(table);
  const dirty = rows !== null && !sameWords(words, saved);
  const inTable = new Set(table.map((r) => r.th.trim()));

  const setRow = (i: number, patch: Partial<Row>) => setRows(table.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const removeRow = (i: number) => setRows(table.filter((_, j) => j !== i));
  const addRow = (th = "", en = "") => setRows([...table, { th, en }]);

  const save = async () => {
    setBusy(true);
    setSaveError("");
    try {
      const r = await api.setThaiWords(slug, words);
      setReport(r);
      setRows(null);
      toast(`${Object.keys(r.words).length} Thai word${Object.keys(r.words).length === 1 ? "" : "s"} saved for Reflex`);
    } catch (e) {
      setSaveError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const addUnknown = (fragment: string) => {
    const en = (adds[fragment] ?? "").trim();
    if (!en) return;
    addRow(fragment, en);
    setAdds((a) => {
      const next = { ...a };
      delete next[fragment];
      return next;
    });
  };

  if (!report) {
    return <p className="text-[11.5px] text-r-muted">{loadError ? `Could not read the Thai words: ${loadError}` : "loading…"}</p>;
  }

  const unknown = report.unknown.filter((u) => !inTable.has(u.fragment));
  return (
    <div className="flex flex-col gap-4">
      <p className="max-w-[860px] text-[12px] leading-[1.6] text-r-muted">
        Reflex reads English words only. MAP turns Thai into English with a built-in glossary of {report.builtin_words} words plus yours; below {pct(report.min_coverage)} known, a Thai
        decision goes to the fallback.
      </p>
      <div className="flex flex-wrap items-baseline gap-x-5 gap-y-1 text-[12px]">
        <span>
          <span className="font-semibold" style={{ color: report.thai_decisions && report.readable < report.thai_decisions ? "var(--lane-llm)" : "var(--lane-reflex)" }}>
            {report.readable} / {report.thai_decisions}
          </span>{" "}
          <span className="text-r-muted">recent Thai decisions readable</span>
        </span>
        <span className="text-r-muted">
          {Object.keys(saved).length} word{Object.keys(saved).length === 1 ? "" : "s"} of your own
        </span>
      </div>

      <div className="grid min-w-0 items-start gap-[18px] lg:grid-cols-2">
        <div className="flex min-w-0 flex-col gap-4">
          <div className="flex flex-col gap-2">
            <Cap>Unknown Thai · most frequent first</Cap>
            {unknown.length === 0 ? (
              <span className="text-[11.5px] text-r-faint">{report.thai_decisions ? "Every Thai word of the recent decisions is known." : "No Thai decisions yet."}</span>
            ) : (
              <div className="flex flex-col gap-1">
                {unknown.slice(0, 30).map((u) => (
                  <div key={u.fragment} className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)_auto] items-center gap-2">
                    <span className="truncate text-[13px]" title={u.fragment}>
                      {u.fragment}
                      <span className="text-[10.5px] text-r-faint"> ×{u.count}</span>
                    </span>
                    <input
                      value={adds[u.fragment] ?? ""}
                      onChange={(e) => setAdds({ ...adds, [u.fragment]: e.target.value })}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") addUnknown(u.fragment);
                      }}
                      placeholder="English, or - to skip"
                      aria-label={`English for ${u.fragment}`}
                      className={field}
                    />
                    <RxButton size="sm" onClick={() => addUnknown(u.fragment)} disabled={!(adds[u.fragment] ?? "").trim()}>
                      Add
                    </RxButton>
                  </div>
                ))}
                {unknown.length > 30 && <span className="text-[10.5px] text-r-faint">+{unknown.length - 30} more</span>}
              </div>
            )}
          </div>

          <div className="flex flex-col gap-2">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <Cap>Your words</Cap>
              <span className="text-[10.5px] text-r-faint">yours win over the built-in glossary · - skips a word</span>
            </div>
            {table.length === 0 && <span className="text-[11.5px] text-r-faint">None yet. Add the unknown words above, or your own.</span>}
            {table.map((r, i) => (
              <div key={i} className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)_auto] items-center gap-2">
                <input value={r.th} onChange={(e) => setRow(i, { th: e.target.value })} placeholder="Thai" aria-label="Thai word" className={field} />
                <input value={r.en} onChange={(e) => setRow(i, { en: e.target.value })} placeholder="English, or -" aria-label="English" className={field} />
                <button
                  type="button"
                  onClick={() => removeRow(i)}
                  aria-label={`Remove ${r.th || "row"}`}
                  className="size-7 rounded-md border border-r-line text-[13px] leading-none text-r-muted hover:text-r-text"
                >
                  ×
                </button>
              </div>
            ))}
            <div className="flex flex-wrap items-center gap-2">
              <RxButton size="sm" onClick={() => addRow()}>
                + Add a word
              </RxButton>
              <RxButton size="sm" tone="primary" onClick={() => void save()} disabled={!dirty || busy}>
                {busy ? "Saving…" : "Save words"}
              </RxButton>
              {dirty && (
                <RxButton
                  size="sm"
                  onClick={() => {
                    setRows(null);
                    setSaveError("");
                  }}
                >
                  Undo changes
                </RxButton>
              )}
              {dirty && <span className="text-[10.5px]" style={{ color: "var(--lane-llm)" }}>unsaved · the preview already uses them</span>}
            </div>
            {saveError && <span className="text-[11.5px] text-[var(--lane-abstain)]">{saveError}</span>}
          </div>
        </div>

        <Preview slug={slug} words={dirty ? toWords(table, true) : undefined} />
      </div>
    </div>
  );
}

/** A textarea and, as you type, what Reflex reads: the English, the coverage, the unknown Thai highlighted. */
function Preview({ slug, words }: { slug: string; words: Record<string, string> | undefined }) {
  const [text, setText] = useState("");
  const [result, setResult] = useState<{ for: string; preview: ThaiPreview } | null>(null);
  const [error, setError] = useState("");
  // The table as a string, so the effect runs again only when the words change.
  const key = JSON.stringify(words ?? null);

  useEffect(() => {
    const t = text.trim();
    if (!t) return;
    let live = true;
    const timer = window.setTimeout(() => {
      api.thaiPreview(slug, t, (JSON.parse(key) as Record<string, string> | null) ?? undefined).then(
        (p) => {
          if (!live) return;
          setResult({ for: t, preview: p });
          setError("");
        },
        (e) => live && setError(errText(e)),
      );
    }, PREVIEW_DEBOUNCE_MS);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [slug, text, key]);

  const p = text.trim() && result ? result.preview : null;
  const color = p ? (!p.has_thai || p.readable ? "var(--lane-reflex)" : "var(--lane-abstain)") : "var(--r-muted)";
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <Cap>Preview</Cap>
      <textarea
        rows={4}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder="Paste a Thai task, message or alert"
        aria-label="Text to preview"
        className="w-full resize-y rounded-lg border border-r-line2 bg-r-bg px-3 py-2.5 text-[13px] leading-[1.6] text-r-text outline-none placeholder:text-r-faint focus:border-r-accent"
      />
      {error && <span className="text-[11.5px] text-[var(--lane-abstain)]">{error}</span>}
      {p && (
        <div className="flex flex-col gap-2.5 rounded-lg border px-3 py-2.5" style={{ borderColor: mix(color, 40), background: mix(color, 5) }}>
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-[11.5px]">
            <span className="font-semibold" style={{ color }}>
              {!p.has_thai ? "no Thai" : p.readable ? "readable" : "not readable · goes to the fallback"}
            </span>
            {p.has_thai && <span className="text-r-muted">{pct(p.coverage)} of the Thai known</span>}
          </div>
          {p.unknown.length > 0 && (
            <div className="flex flex-col gap-1">
              <Cap>Unknown words highlighted</Cap>
              <span className="text-[13px] leading-[1.7] break-words whitespace-pre-wrap">
                <Highlight text={result!.for} marks={p.unknown} />
              </span>
            </div>
          )}
          <div className="flex flex-col gap-1">
            <Cap>Reflex reads</Cap>
            <span className="text-[12.5px] leading-[1.6] break-words whitespace-pre-wrap">{p.text || <span className="text-r-faint">nothing</span>}</span>
          </div>
        </div>
      )}
    </div>
  );
}

/** `text` with every occurrence of `marks` highlighted, longest first. */
function Highlight({ text, marks }: { text: string; marks: string[] }) {
  const list = [...new Set(marks.filter(Boolean))].sort((a, b) => b.length - a.length);
  if (!list.length) return <>{text}</>;
  const re = new RegExp(`(${list.map((m) => m.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "g");
  const set = new Set(list);
  return (
    <>
      {text.split(re).map((part, i) =>
        set.has(part) ? (
          <mark key={i} className="rounded-[3px] px-0.5 text-r-text" style={{ background: mix("var(--lane-abstain)", 30) }}>
            {part}
          </mark>
        ) : (
          <Fragment key={i}>{part}</Fragment>
        ),
      )}
    </>
  );
}
