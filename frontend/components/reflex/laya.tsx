"use client";

import { useEffect, useState } from "react";

import { api } from "@/lib/api";
import type { LayaInfo } from "@/lib/generated/LayaInfo";

import { Cap, RxButton, errText } from "./ui";

const STATE: Record<string, [string, string]> = {
  ready: ["ready", "var(--lane-reflex)"],
  loading: ["loading (the first start downloads about 800 MB)", "var(--lane-llm)"],
  off: ["off", "var(--r-faint)"],
  stopped: ["Reflex is not running", "var(--lane-abstain)"],
};
const conf = (c: number) => c.toFixed(2);

/** 07 · Laya: Reflex's model lane as a second stage before the fallback. On/off, its state, and its own threshold. */
export function Laya({ slug, tick, can, toast }: { slug: string; tick: number; can: boolean; toast: (m: string) => void }) {
  const [info, setInfo] = useState<LayaInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState("");
  const [poll, setPoll] = useState(0);

  useEffect(() => {
    let live = true;
    api
      .laya(slug)
      .then((i) => live && setInfo(i))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [slug, tick, poll]);

  // While it loads, or waits for its threshold, look again every few seconds.
  const waiting = info?.enabled && (info.state === "loading" || (info.state === "ready" && info.threshold === null && info.lessons === 0));
  useEffect(() => {
    if (!waiting) return;
    const t = setTimeout(() => setPoll((n) => n + 1), 4000);
    return () => clearTimeout(t);
  }, [waiting, poll]);

  const put = async (body: { enabled?: boolean; threshold?: number | null }, done: string) => {
    setBusy(true);
    try {
      setInfo(await api.setLaya(slug, body));
      setDraft("");
      toast(done);
    } catch (e) {
      toast(errText(e));
    } finally {
      setBusy(false);
    }
  };

  if (!info) return <Cap>Loading…</Cap>;
  const [stateText, stateColor] = STATE[info.state] ?? [info.state, "var(--r-faint)"];
  const t = Number(draft);
  const draftOk = draft.trim() !== "" && Number.isFinite(t) && t >= 0 && t <= 1;
  return (
    <div className="flex flex-col gap-2.5">
      <Cap>
        a question Reflex is unsure of goes to laya, a small model Reflex runs on this machine, before the fallback · about 1.7 GB of memory while on · the first start
        downloads about 800 MB
      </Cap>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border border-r-line px-3 py-2.5">
        <RxButton tone={info.enabled ? "ghost" : "primary"} disabled={busy || !can || (!info.enabled && !info.available)} onClick={() => put({ enabled: !info.enabled }, info.enabled ? "Laya off; Reflex restarted" : "Laya on; Reflex is restarting")}>
          {busy ? "Working…" : info.enabled ? "Turn laya off" : "Turn laya on"}
        </RxButton>
        <span className="flex items-center gap-1.5 text-[11.5px]">
          <span className="inline-block size-[7px] rounded-full" style={{ background: info.enabled ? stateColor : "var(--r-faint)" }} />
          {info.enabled ? stateText : "off"}
        </span>
        {!info.available && <span className="text-[11px] text-r-muted">The installed Reflex has no laya lane; update Reflex to use it.</span>}
        {info.enabled && (
          <span className="text-[11.5px] text-r-muted">
            {info.threshold === null ? (
              <>
                shadow: laya&apos;s answers are logged, the fallback still answers · threshold fitted from lessons ({info.lessons} / {info.lessons_needed} scored
                {info.lessons ? `, ${info.right} right` : ""})
              </>
            ) : (
              <>
                answers at confidence ≥ <b className="font-medium text-r-text">{conf(info.threshold)}</b> · right on {info.right} of {info.lessons} lessons
              </>
            )}
          </span>
        )}
      </div>
      {info.enabled && can && (
        <div className="flex flex-wrap items-center gap-2 text-[11px] text-r-muted">
          <span>threshold</span>
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder={info.threshold === null ? "0 – 1" : conf(info.threshold)}
            inputMode="decimal"
            className="w-[72px] rounded-md border border-r-line bg-transparent px-2 py-1 text-[11.5px] text-r-text outline-none focus:border-r-accent"
          />
          <RxButton size="sm" disabled={busy || !draftOk} onClick={() => put({ threshold: t }, `Laya answers at confidence ≥ ${conf(t)}`)}>
            Set
          </RxButton>
          {info.threshold !== null && (
            <RxButton size="sm" disabled={busy} onClick={() => put({ threshold: null }, "Laya's threshold cleared; it is fitted from the lessons again")}>
              Fit from lessons
            </RxButton>
          )}
        </div>
      )}
    </div>
  );
}
