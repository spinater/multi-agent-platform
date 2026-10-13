"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { decisionsApi } from "@/lib/api";
import type { DecisionCounts } from "@/lib/generated/DecisionCounts";
import type { DecisionModel } from "@/lib/generated/DecisionModel";
import type { DecisionNode } from "@/lib/generated/DecisionNode";
import type { Event } from "@/lib/generated/Event";

import { useApp } from "../app-context";
import { edgeKey, isDecision, kindMap, startOfToday, type KindMap } from "./model";

export type Pulse = { id: number; from: DecisionNode; to: DecisionNode };

export type DecisionData = {
  model: DecisionModel | null;
  kinds: KindMap;
  /** Times each edge was taken today, by `edgeKey`. */
  counts: Map<string, number>;
  /** Newest decision per task id (server snapshot merged with live events). */
  latest: Map<number, Event>;
  /** Edges taken in the last second, for the travelling dots. */
  pulses: Pulse[];
  error: string | null;
};

/** The decision model, today's edge counts and each task's newest decision for the current project, kept live from
 * the dashboard's event stream (counts are re-read from the server shortly after new decisions). */
export function useDecisions(): DecisionData {
  const { events, project, now } = useApp();
  const [model, setModel] = useState<DecisionModel | null>(null);
  const [counts, setCounts] = useState<DecisionCounts | null>(null);
  const [snapshot, setSnapshot] = useState<Event[]>([]);
  const [pulses, setPulses] = useState<Pulse[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const seen = useRef<number | null>(null);
  const day = startOfToday(now);
  const kinds = useMemo(() => kindMap(model), [model]);

  useEffect(() => {
    decisionsApi.model().then(setModel, (e: Error) => setError(e.message));
  }, []);

  useEffect(() => {
    let live = true;
    Promise.all([decisionsApi.edges(day, project), decisionsApi.latest(project)]).then(
      ([c, l]) => {
        if (!live) return;
        setCounts(c);
        setSnapshot(l);
        setError(null);
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [day, project, refresh]);

  // New decision events: a dot travels along the edge, and the counts are re-read once things settle.
  const newest = events[0]?.id ?? 0;
  useEffect(() => {
    if (seen.current === null) {
      seen.current = newest;
      return;
    }
    const fresh = events.filter((e) => e.id > (seen.current ?? 0) && isDecision(e));
    seen.current = Math.max(seen.current, newest);
    if (!fresh.length || !kinds.size) return;
    const moves = fresh.flatMap((e) => {
      const k = kinds.get(e.kind);
      return k && k.from !== k.to ? [{ id: e.id, from: k.from, to: k.to }] : [];
    });
    if (moves.length) {
      setPulses((p) => [...p, ...moves]);
      window.setTimeout(() => setPulses((p) => p.filter((x) => !moves.some((m) => m.id === x.id))), 1200);
    }
    const t = window.setTimeout(() => setRefresh((n) => n + 1), 700);
    return () => window.clearTimeout(t);
  }, [newest, events, kinds]);

  const latest = useMemo(() => {
    const map = new Map<number, Event>();
    for (const e of snapshot) if (e.task_id !== null) map.set(e.task_id, e);
    // `events` is newest first: the first decision per task wins if it is newer than the snapshot.
    const taken = new Set<number>();
    for (const e of events) {
      if (!isDecision(e) || taken.has(e.task_id!)) continue;
      taken.add(e.task_id!);
      const prev = map.get(e.task_id!);
      if (!prev || prev.id < e.id) map.set(e.task_id!, e);
    }
    return map;
  }, [snapshot, events]);

  const countMap = useMemo(() => new Map((counts?.edges ?? []).map((e) => [edgeKey(e.from, e.to), e.count])), [counts]);

  return { model, kinds, counts: countMap, latest, pulses, error };
}

/** A task's full trace (newest first): fetched once per selection, then extended by live events. */
export function useTrace(taskId: number | null): Event[] {
  const { events } = useApp();
  const [fetched, setFetched] = useState<{ id: number; events: Event[] } | null>(null);

  useEffect(() => {
    if (taskId === null) return;
    let live = true;
    decisionsApi.trace(taskId).then(
      (list) => live && setFetched({ id: taskId, events: list }),
      () => live && setFetched({ id: taskId, events: [] }),
    );
    return () => {
      live = false;
    };
  }, [taskId]);

  return useMemo(() => {
    if (taskId === null) return [];
    const base = fetched?.id === taskId ? fetched.events : [];
    const top = base[0]?.id ?? 0;
    const extra = events.filter((e) => e.task_id === taskId && isDecision(e) && e.id > top);
    return [...extra, ...base];
  }, [taskId, fetched, events]);
}
