"use client";

/** The command centre's "Lanes" card (left column): the current project's lanes with role, name, state and context %,
 * each a link to its card on the Lanes screen; in all-projects mode, a count per project. Lanes change on the
 * backend's tick, so the card re-reads them on a timer. */

import clsx from "clsx";
import { Layers } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { lanesApi } from "@/lib/api";
import type { LaneInfo } from "@/lib/generated/LaneInfo";
import type { LaneState } from "@/lib/generated/LaneState";

import { useT } from "@/lib/i18n";
import { useApp } from "../app-context";
import { Pill, SideTitle, type Tone } from "../command-ui";
import { STANDARD_ROLES } from "./add-lane";

const REFRESH_MS = 15_000;

const WORD: Record<LaneState, [string, Tone]> = {
  busy: ["working", "ok"],
  idle: ["idle", "idle"],
  waiting: ["waiting", "accent"],
  pending_input: ["unsent", "warn"],
  rate_limited: ["limit", "warn"],
  missing: ["missing", "bad"],
};

/** The order a lane's sessions are summed up in: the loudest state wins. */
const LOUDNESS: LaneState[] = ["missing", "waiting", "pending_input", "rate_limited", "busy", "idle"];

/** One word and tone for the whole lane. */
export function laneState(l: LaneInfo): [string, Tone] {
  if (l.stopped) return ["stopped", "bad"];
  if (l.error) return ["error", "bad"];
  if (!l.checked_at) return ["…", "idle"];
  if (l.sessions.length === 0) return ["no session", "warn"];
  const worst = LOUDNESS.find((s) => l.sessions.some((x) => x.state === s)) ?? "idle";
  return WORD[worst];
}

const contextOf = (l: LaneInfo) => {
  const pcts = l.sessions.map((s) => s.context_pct).filter((p): p is number => p !== null);
  return pcts.length ? Math.max(...pcts) : null;
};

export function LanesCard() {
  const { project, scope, projects, go } = useApp();
  const t = useT();
  const all = scope === "all";
  const [lanes, setLanes] = useState<LaneInfo[] | null>(null);

  const load = useCallback(() => lanesApi.list(all ? null : project).then(setLanes, () => setLanes((l) => l ?? [])), [all, project]);
  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(t);
  }, [load]);

  // The default workspace has no lanes of its own; `?project=` is not sent, so keep only lanes of no project.
  const shown = (lanes ?? []).filter((l) => all || project !== null || l.project === null);

  if (all) {
    const counts = new Map<string | null, LaneInfo[]>();
    for (const l of shown) counts.set(l.project, [...(counts.get(l.project) ?? []), l]);
    const rows = [...projects.map((p) => p.project.slug), ...[...counts.keys()].filter((k) => k === null || !projects.some((p) => p.project.slug === k))].filter(
      (k, i, a) => a.indexOf(k) === i,
    );
    return (
      <section className="flex flex-col gap-3">
        <SideTitle glyph={<Layers className="size-3" />} note={<a href="#lanes" className="text-accent-soft hover:underline">{shown.length} lanes →</a>}>
          Lanes
        </SideTitle>
        <div className="flex flex-col overflow-hidden rounded-[10px] border border-line">
          {rows.map((slug) => {
            const list = counts.get(slug) ?? [];
            const working = list.filter((l) => laneState(l)[0] === "working").length;
            const loud = list.filter((l) => ["waiting", "unsent", "limit", "missing", "stopped", "error", "no session"].includes(laneState(l)[0])).length;
            const name = slug === null ? "No project" : (projects.find((p) => p.project.slug === slug)?.project.name ?? slug);
            return (
              <a key={slug ?? "-"} href="#lanes" className="flex items-center justify-between gap-2 border-b border-line px-3 py-2 last:border-b-0 hover:bg-white/[0.025]">
                <span className="truncate text-[12.5px] font-medium text-text">{name}</span>
                <span className="flex-none font-mono text-[10.5px] text-muted">
                  {list.length} lane{list.length === 1 ? "" : "s"}
                  {working > 0 && <span className="text-ok"> · {working} working</span>}
                  {loud > 0 && <span className="text-warn"> · {loud} need a look</span>}
                </span>
              </a>
            );
          })}
          {rows.length === 0 && <p className="px-3 py-2.5 font-mono text-[11px] text-muted">No lanes.</p>}
        </div>
      </section>
    );
  }

  const missing = project ? STANDARD_ROLES.filter((r) => !shown.some((l) => l.role === r)) : [];
  return (
    <section className="flex flex-col gap-3">
      <SideTitle glyph={<Layers className="size-3" />} note={<a href="#lanes" className="text-accent-soft hover:underline">{project ? t("cc.addLane") : "all lanes →"}</a>}>
        {t("cc.lanes")}
      </SideTitle>
      <div className="flex flex-col overflow-hidden rounded-[10px] border border-line">
        {shown.map((l) => {
          const [word, tone] = laneState(l);
          const ctx = contextOf(l);
          return (
            <button
              key={l.name}
              type="button"
              onClick={() => go("lanes", l.name)}
              className="grid cursor-pointer grid-cols-[minmax(0,1fr)_auto] items-center gap-2.5 border-b border-line px-3 py-2 text-left last:border-b-0 hover:bg-white/[0.025]"
            >
              <span className="flex min-w-0 flex-col gap-1">
                <span className="font-mono text-[9.5px] leading-none tracking-[0.12em] text-muted uppercase">{l.role ?? "no role"}</span>
                <span className="truncate font-mono text-[12.5px] leading-[1.15] font-medium text-text" title={l.name}>
                  {l.label}
                </span>
              </span>
              <span className="flex items-center gap-2">
                {ctx !== null && <span className={clsx("font-mono text-[10.5px]", ctx >= 80 ? "text-warn" : "text-muted")}>{ctx}%</span>}
                <Pill tone={tone} small>
                  {word}
                </Pill>
              </span>
            </button>
          );
        })}
        {missing.map((r) => (
          <a key={r} href="#lanes" className="flex items-center justify-between gap-2 border-b border-dashed border-line px-3 py-2 last:border-b-0 hover:bg-white/[0.025]">
            <span className="flex min-w-0 flex-col gap-1">
              <span className="font-mono text-[9.5px] leading-none tracking-[0.12em] text-muted uppercase">{r}</span>
              <span className="truncate font-mono text-[12px] text-text-2">not set up</span>
            </span>
            <span className="font-mono text-[10.5px] text-accent-soft">Set up →</span>
          </a>
        ))}
        {shown.length === 0 && missing.length === 0 && <p className="px-3 py-2.5 font-mono text-[11px] text-muted">{lanes === null ? "loading…" : "No lanes."}</p>}
      </div>
    </section>
  );
}
