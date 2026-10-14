"use client";

import { useSyncExternalStore } from "react";

import { request } from "./api";
import type { Session } from "./generated/Session";
import type { UpdateInfo } from "./generated/UpdateInfo";
import type { UpdatesReport } from "./generated/UpdatesReport";

/** One update report for the whole page: fetched once from `/api/updates`, then replaced by `updates` messages from
 * the WebSocket (see `use-dashboard.ts`), so the top bar's bell and the Usage screen share it. */
type UpdatesStore = { report: UpdatesReport | null; checking: boolean; error: string | null };

let state: UpdatesStore = { report: null, checking: false, error: null };
let fetched = false;
const listeners = new Set<() => void>();
/** Called with each update that becomes available after the first report (not for ones already there on load). */
const newUpdateListeners = new Set<(info: UpdateInfo) => void>();

function set(next: Partial<UpdatesStore>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

const key = (p: UpdateInfo) => `${p.provider}@${p.latest ?? ""}`;

export function setUpdatesReport(report: UpdatesReport) {
  const prev = state.report;
  set({ report, error: null });
  if (!prev) return;
  const before = new Set(prev.providers.filter((p) => p.available).map(key));
  for (const p of report.providers) if (p.available && !before.has(key(p))) newUpdateListeners.forEach((l) => l(p));
}

/** Subscribes to newly available updates; returns the unsubscribe function. */
export function onNewUpdate(listener: (info: UpdateInfo) => void): () => void {
  newUpdateListeners.add(listener);
  return () => {
    newUpdateListeners.delete(listener);
  };
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!fetched) {
    fetched = true;
    request<UpdatesReport>("GET", "/updates")
      .then((report) => {
        // A push that arrived first is newer.
        if (!state.report) set({ report, error: null });
      })
      .catch((err: unknown) => {
        fetched = false;
        set({ error: err instanceof Error ? err.message : String(err) });
      });
  }
  return () => {
    listeners.delete(listener);
  };
}

/** Asks the server to check every provider now. */
export async function checkUpdatesNow() {
  if (state.checking) return;
  set({ checking: true });
  try {
    setUpdatesReport(await request<UpdatesReport>("POST", "/updates/refresh", {}));
  } catch (err) {
    set({ error: err instanceof Error ? err.message : String(err) });
  } finally {
    set({ checking: false });
  }
}

/** Runs the provider's update command in its helper session; the report follows over the WebSocket. */
export function startUpdate(provider: string): Promise<Session> {
  return request<Session>("POST", `/providers/${encodeURIComponent(provider)}/update`, {});
}

export function useUpdates(): UpdatesStore {
  return useSyncExternalStore(
    subscribe,
    () => state,
    () => state,
  );
}

/** The provider's row, if the report has one. */
export function updateOf(report: UpdatesReport | null, provider: string): UpdateInfo | null {
  return report?.providers.find((p) => p.provider === provider) ?? null;
}
