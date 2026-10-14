"use client";

import { useSyncExternalStore } from "react";

import { request } from "./api";
import type { ProviderUsage } from "./generated/ProviderUsage";
import type { UsageReport } from "./generated/UsageReport";
import type { UsageWindow } from "./generated/UsageWindow";

/** One usage report for the whole page: fetched once from `/api/usage`, then replaced by `usage` messages from the
 * WebSocket (see `use-dashboard.ts`), so every top bar's pill and the Usage screen share it. */
type UsageStore = { report: UsageReport | null; reading: boolean; error: string | null };

let state: UsageStore = { report: null, reading: false, error: null };
let fetched = false;
const listeners = new Set<() => void>();

function set(next: Partial<UsageStore>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

export function setUsageReport(report: UsageReport) {
  set({ report, error: null });
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!fetched) {
    fetched = true;
    request<UsageReport>("GET", "/usage")
      .then((report) => set({ report, error: null }))
      .catch((err: unknown) => {
        fetched = false;
        set({ error: err instanceof Error ? err.message : String(err) });
      });
  }
  return () => {
    listeners.delete(listener);
  };
}

/** Asks the server to read every provider now. */
export async function readUsageNow() {
  if (state.reading) return;
  set({ reading: true });
  try {
    set({ report: await request<UsageReport>("POST", "/usage/refresh", {}), error: null });
  } catch (err) {
    set({ error: err instanceof Error ? err.message : String(err) });
  } finally {
    set({ reading: false });
  }
}

export function useUsage(): UsageStore {
  return useSyncExternalStore(
    subscribe,
    () => state,
    () => state,
  );
}

// ---- shared formatting (the design's renderVals)

const NAMES: Record<string, [string, string]> = {
  claude: ["Claude Code", "Claude"],
  codex: ["Codex", "Codex"],
  gemini: ["Gemini CLI", "Gemini"],
  cursor: ["Cursor CLI", "Cursor"],
  copilot: ["GitHub Copilot CLI", "Copilot"],
  openrouter: ["OpenRouter · Claude Code", "OpenRouter"],
};

/** Display name of a provider: long for cards, short for the top bar. */
export function providerName(name: string, short = false): string {
  const known = NAMES[name];
  if (known) return known[short ? 1 : 0];
  return name.charAt(0).toUpperCase() + name.slice(1);
}

/** "3d 5h", "2h 07m", "4m 12s", "9s" (`seconds = false` drops the seconds of "4m 12s"). */
export function dur(ms: number, seconds = true): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m) return seconds ? `${m}m ${s % 60}s` : `${m}m`;
  return `${s}s`;
}

/** "16:40" today, else "Thu 15 Oct 16:00". */
export function clockAt(ms: number, now: number): string {
  const d = new Date(ms);
  const hm = d.toTimeString().slice(0, 5);
  if (d.toDateString() === new Date(now).toDateString()) return hm;
  return `${d.toLocaleDateString("en-GB", { weekday: "short", day: "2-digit", month: "short" })} ${hm}`;
}

/** 1.84M, 212k, 940. */
export function fmtTokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 1 : 2)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}k`;
  return String(n);
}

/** Red at the limit, yellow at or above the skip line, else the provider's colour. */
export function meterColor(used: number | null, skip: number, base: string): string {
  if (used === null) return "var(--muted)";
  if (used >= 100) return "var(--bad)";
  if (used >= skip) return "var(--warn)";
  return base;
}

export type Level = "ok" | "high" | "skipped" | "limited";

/** The design's status from the fullest window: limited at 100, skipped at the skip line, high within 15 points. */
export function levelOf(worst: number, skip: number): Level {
  return worst >= 100 ? "limited" : worst >= skip ? "skipped" : worst >= skip - 15 ? "high" : "ok";
}

export function worstOf(p: ProviderUsage): UsageWindow | null {
  let top: UsageWindow | null = null;
  for (const w of p.windows) if (w.used_pct !== null && (top === null || w.used_pct > (top.used_pct ?? -1))) top = w;
  return top;
}

/** Text and token colour of a provider's badge: problems with the reading first, then the usage level. */
export function badgeOf(p: ProviderUsage, skip: number): { text: string; tone: "ok" | "warn" | "bad" | "idle" } {
  switch (p.status) {
    case "pending":
      return { text: "reading", tone: "idle" };
    case "no_reader":
      return { text: "no reader", tone: "idle" };
    case "disabled":
      return { text: "off", tone: "idle" };
    case "signed_out":
      return { text: "signed out", tone: "bad" };
    case "error":
      if (!p.windows.length) return { text: "error", tone: "bad" };
  }
  const worst = worstOf(p)?.used_pct ?? 0;
  const level = p.skipped && worst < skip ? "skipped" : levelOf(worst, skip);
  return { text: level, tone: level === "ok" ? "ok" : level === "limited" ? "bad" : "warn" };
}

export function windowValue(w: UsageWindow, compact = false): string {
  if (w.cap !== null && w.used !== null)
    return compact ? `${Math.round(w.used)}/${w.cap}` : `${Math.round(w.used).toLocaleString("en-US")} / ${w.cap.toLocaleString("en-US")}`;
  return w.used_pct === null ? "—" : `${Math.round(w.used_pct)}%`;
}

/** Windows shortest first (the pill's two gauges are the first and the last). */
export function byLength(windows: UsageWindow[]): UsageWindow[] {
  return [...windows].sort((a, b) => (a.window_minutes ?? Infinity) - (b.window_minutes ?? Infinity));
}

/** The pill's text: who is over the skip line, else the tightest window anywhere. */
export function headline(report: UsageReport | null): { text: string; hot: boolean } {
  if (!report) return { text: "usage", hot: false };
  const skip = report.skip_pct;
  const hot = report.providers.filter((p) => p.skipped || (worstOf(p)?.used_pct ?? 0) >= skip);
  if (hot.length) {
    const names = hot.map((p) => providerName(p.provider, true));
    return { text: `${names.join(", ")} ${hot.length > 1 ? "are" : "is"} over ${Math.round(skip)}%`, hot: true };
  }
  let tight: { p: ProviderUsage; w: UsageWindow } | null = null;
  for (const p of report.providers) {
    const w = worstOf(p);
    if (w && (tight === null || (w.used_pct ?? 0) > (tight.w.used_pct ?? 0))) tight = { p, w };
  }
  if (!tight) return { text: report.read_at ? "no usage readers" : "reading usage…", hot: false };
  return { text: `${providerName(tight.p.provider, true)} ${tight.w.short} ${Math.round(tight.w.used_pct ?? 0)}%`, hot: false };
}
