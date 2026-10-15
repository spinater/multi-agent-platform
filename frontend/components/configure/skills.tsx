"use client";

/** Configure › Skills (backend skills.rs): turn each skill on or off for the project (a deny rule in the project's
 * `.claude/settings.local.json`), see which CLIs load it and where it comes from. Load history and the skill × task
 * matrix stay on the Skills screen. */

import { useEffect, useState } from "react";

import { api, type ConfigOp } from "@/lib/api";
import type { SkillsOverview } from "@/lib/generated/SkillsOverview";

import { useApp } from "../app-context";
import { AskAgent } from "./ask-agent";
import { Note, SourceBadge, Switch, TabHead, useConfigure } from "./ui";

export function SkillsTab() {
  const { scope, slug, setPending } = useConfigure();
  const { go, project } = useApp();
  const [data, setData] = useState<SkillsOverview | null>(null);
  const [error, setError] = useState("");
  const [want, setWant] = useState<Record<string, boolean>>({});
  const [tick, setTick] = useState(0);

  // Machine scope lists the skills of the current workspace that come from the home folder or a plugin.
  const target = scope === "project" ? slug : project;
  useEffect(() => {
    let live = true;
    api.skills(target).then(
      (d) => {
        if (!live) return;
        setData(d);
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [target, tick]);

  const changed = Object.entries(want).filter(([id, on]) => data?.skills.find((s) => s.id === id)?.enabled !== on);
  useEffect(() => {
    setPending(
      "skills",
      changed.length
        ? {
            count: changed.length,
            files: [".claude/settings.local.json"],
            save: async () => {
              for (const [id, on] of changed) await api.setSkillEnabled(target, id, on);
              setWant({});
              setTick((t) => t + 1);
            },
            discard: () => setWant({}),
          }
        : null,
    );
  }, [changed.length, JSON.stringify(changed), target, setPending]); // eslint-disable-line react-hooks/exhaustive-deps

  const skills = (data?.skills ?? []).filter((s) => scope === "project" || s.source !== "project");
  return (
    <>
      <TabHead
        title="Skills"
        desc="Turn skills on or off for this project. Agents pick which enabled skills they load in the Agents tab."
        savesTo={scope === "machine" ? "~/.claude/settings.json (per project: .claude/settings.local.json)" : ".claude/settings.local.json"}
      />
      {scope === "project" && (
        <AskAgent
          tab="skills"
          context={{ skills: skills.map((s) => ({ name: s.name, description: s.description, enabled: want[s.id] ?? s.enabled, loads: s.usage.loads, last_used: s.usage.last_used })) }}
          onApply={(ops: ConfigOp[]) => {
            const next = { ...want };
            let n = 0;
            for (const o of ops) {
              const sk = skills.find((s) => s.name === o.target.replace(/^skill:/, ""));
              const en = (o.value as { enabled?: boolean } | null)?.enabled;
              if (o.op !== "change" || !sk || typeof en !== "boolean" || !sk.can_toggle) continue;
              next[sk.id] = en;
              n++;
            }
            setWant(next);
            return n;
          }}
        />
      )}
      {scope === "machine" && <Note>Skills are switched on or off per project. Here are the skills every project gets from the home folder and plugins; switch to Project to change one.</Note>}
      {error && <Note tone="bad">Could not read the skills: {error}</Note>}
      {!data && !error && <Note>Reading the skill folders…</Note>}
      {data && skills.length === 0 && <Note>No skills found in {data.workspace} or the home folders.</Note>}
      <div className="flex flex-col gap-1.5">
        {skills.map((s) => {
          const on = want[s.id] ?? s.enabled;
          return (
            <div key={s.id} className="flex flex-wrap items-center gap-x-3.5 gap-y-2 rounded-[9px] border border-line bg-surface px-3 py-[11px]">
              <Switch
                on={on}
                disabled={scope === "machine" || !s.can_toggle}
                title={s.toggle_note ?? (on ? "Turn off for this project" : "Turn on for this project")}
                onClick={() => setWant({ ...want, [s.id]: !on })}
              />
              <span className="flex min-w-0 flex-[1_1_240px] flex-col gap-[5px]">
                <span className="font-mono text-[13px] leading-none font-semibold">{s.name}</span>
                <span className="text-[11.5px] leading-[1.35] text-muted">{s.description || "no description"}</span>
              </span>
              <span className="font-mono text-[10.5px] text-text-2">loaded by {s.loads_in.map((l) => (l.how === "native" ? l.provider : `${l.provider} (${l.how})`)).join(", ") || "no CLI"}</span>
              <SourceBadge source={s.source === "project" ? "project" : "machine"} label={s.source === "plugin" ? `plugin${s.plugin ? ` · ${s.plugin}` : ""}` : undefined} />
            </div>
          );
        })}
      </div>
      <button onClick={() => go("skills")} className="mt-1 cursor-pointer self-start text-[12.5px] font-medium text-link hover:text-link-hover">
        Load history and skill × task matrix →
      </button>
    </>
  );
}
