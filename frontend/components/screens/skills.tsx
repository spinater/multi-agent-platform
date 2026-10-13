"use client";

import { RefreshCw, Search } from "lucide-react";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { SkillSource } from "@/lib/generated/SkillSource";
import type { SkillsOverview } from "@/lib/generated/SkillsOverview";

import { useApp } from "../app-context";
import { Main, PageHeader, SectionTitle } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";
import { ContextFiles } from "../skills/context-files";
import { RunSkillModal } from "../skills/run-modal";
import { SkillCard } from "../skills/skill-card";
import { SkillDetail } from "../skills/skill-detail";
import { SkillMatrix } from "../skills/skill-matrix";
import { compareSkills, DOT_PROVIDERS, SOURCE_ORDER, tint } from "../skills/shared";
import { Empty } from "../ui";

/** Usage comes from transcripts on disk, which change without a push; re-read them this often. */
const REFRESH_MS = 30_000;
/** Cards shown before "Show all": a home folder full of synced and plugin skills would bury the matrix. */
const CARD_LIMIT = 12;

type Loaded = { project: string | null; data: SkillsOverview } | { project: string | null; error: string };

function Chip({ active, color, onClick, children }: { active: boolean; color: string; onClick: () => void; children: ReactNode }) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className="flex h-7 items-center gap-[7px] rounded-md border px-[11px] font-mono text-[11.5px] font-medium whitespace-nowrap text-text"
      style={{ borderColor: active ? color : "var(--line-2)", background: active ? tint(color, 12) : "transparent" }}
    >
      {children}
    </button>
  );
}

/** What each agent can load in the current project's workspace, where it comes from, and which tasks used it
 * (design: Skills.dc.html). `#skills/<invocation>` opens one skill. */
export function SkillsScreen() {
  const { project, current, server, tasks, providers, now, inProject, toast, go, route } = useApp();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [source, setSource] = useState<SkillSource | "all">("all");
  const [prov, setProv] = useState<string>("all");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [running, setRunning] = useState(false);
  const [allRows, setAllRows] = useState(false);
  const [allCards, setAllCards] = useState(false);
  const [tick, setTick] = useState(0);

  const refresh = useCallback(() => setTick((t) => t + 1), []);
  useEffect(() => {
    let live = true;
    api
      .skills(project)
      .then((data) => live && setLoaded({ project, data }))
      .catch((e: Error) => live && setLoaded({ project, error: e.message }));
    const timer = window.setInterval(refresh, REFRESH_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [project, tick, refresh]);

  const projectName = current?.project.name ?? (project === null ? workspaceName(server?.workspace_dir) : project);
  const ours = loaded && loaded.project === project ? loaded : null;
  const data = ours && "data" in ours ? ours.data : null;
  const skills = useMemo(() => [...(data?.skills ?? [])].sort(compareSkills), [data]);
  const projectTasks = useMemo(() => tasks.filter((t) => inProject(t.project)).sort((a, b) => a.id - b.id), [tasks, inProject]);

  const q = query.trim().toLowerCase();
  const visible = skills.filter(
    (s) =>
      (source === "all" || s.source === source) &&
      (prov === "all" || s.loads_in.some((l) => l.provider === prov)) &&
      (!q || s.invocation.toLowerCase().includes(q) || s.description.toLowerCase().includes(q)),
  );
  // `#skills/<invocation>` deep-links a skill.
  const sel = skills.find((s) => s.id === selected) ?? skills.find((s) => s.invocation === route.arg) ?? visible[0] ?? null;
  const enabled = skills.filter((s) => s.enabled).length;
  const week = skills.reduce((n, s) => n + s.usage.loads_week, 0);
  const matrixRows = allRows ? visible : visible.filter((s) => s.source === "project" || s.usage.task_ids.length > 0 || s.id === sel?.id);
  const provList = [...new Set([...DOT_PROVIDERS, ...providers.map((p) => p.name)])];

  const toggle = async () => {
    if (!sel) return;
    setBusy(true);
    try {
      const next = await api.setSkillEnabled(project, sel.id, !sel.enabled);
      setLoaded({ project, data: next });
      toast(`/${sel.invocation} ${sel.enabled ? "disabled" : "enabled"} for Claude in ${projectName}`);
    } catch (e) {
      toast((e as Error).message, "bad");
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <TopBar />
      <Main className="gap-[26px]">
        <PageHeader
          kicker={`Project · ${projectName}`}
          title="Skills"
          lead="What each agent can load in this workspace, where it comes from, and which tasks used it."
          actions={
            <div className="flex flex-wrap items-center gap-x-[22px] gap-y-1.5 font-mono text-[11px] leading-[1.4] text-muted">
              <span className="whitespace-nowrap">{skills.length} skills</span>
              <span className="whitespace-nowrap">{enabled} enabled</span>
              <span className="whitespace-nowrap">{week} loads this week</span>
              <button onClick={refresh} className="flex items-center gap-1.5 whitespace-nowrap hover:text-text" title="Scan again">
                <RefreshCw className="size-3" />
                rescan
              </button>
            </div>
          }
        />

        {ours && "error" in ours && <p className="rounded-lg border border-bad/40 bg-bad/10 px-3 py-2 text-sm text-bad">Could not scan skills: {ours.error}</p>}
        {!ours && <Empty>Scanning the workspace…</Empty>}

        {data && (
          <>
            <section className="flex flex-col gap-3">
              <SectionTitle glyph="▤" note="read at session start, by provider">
                Context files
              </SectionTitle>
              <ContextFiles contexts={data.contexts} />
            </section>

            <div className="flex flex-wrap items-center justify-between gap-2.5">
              <div className="flex flex-wrap items-center gap-1.5">
                {(["all", ...SOURCE_ORDER] as const).map((s) => (
                  <Chip key={s} active={source === s} color="var(--accent)" onClick={() => setSource(s)}>
                    {s}
                    <span className="text-muted">{s === "all" ? skills.length : skills.filter((k) => k.source === s).length}</span>
                  </Chip>
                ))}
                <span className="mx-1.5 my-1 h-5 w-px bg-line-2" />
                {["all", ...provList].map((p) => {
                  const color = p === "all" ? "var(--text)" : providerColor(p);
                  return (
                    <Chip key={p} active={prov === p} color={color} onClick={() => setProv(p)}>
                      <span className="size-1.5 rounded-full" style={{ background: color }} />
                      {p === "all" ? "any agent" : p}
                    </Chip>
                  );
                })}
              </div>
              <label className="flex h-8 items-center gap-2 rounded-lg border border-line-2 px-3">
                <Search className="size-3.5 text-muted" />
                <input
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="search skills"
                  aria-label="Search skills"
                  className="w-[170px] max-w-full bg-transparent font-mono text-xs text-text outline-none placeholder:text-muted/80"
                />
              </label>
            </div>

            <div className="flex flex-wrap items-start gap-5">
              <div className="grid min-w-0 flex-[1_1_440px] grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-2.5">
                {(allCards ? visible : visible.slice(0, CARD_LIMIT)).map((s) => (
                  <SkillCard key={s.id} skill={s} selected={s.id === sel?.id} onSelect={() => setSelected(s.id)} />
                ))}
                {visible.length > CARD_LIMIT && (
                  <button onClick={() => setAllCards(!allCards)} className="col-span-full h-9 rounded-xl border border-dashed border-line-2 font-mono text-[11.5px] text-link hover:border-line-3 hover:text-link-hover">
                    {allCards ? "Show fewer" : `Show all ${visible.length} skills (${visible.length - CARD_LIMIT} more)`}
                  </button>
                )}
                {!visible.length && <p className="col-span-full rounded-xl border border-line px-4 py-8 text-center text-sm text-muted">No skills match.</p>}
              </div>
              {sel ? (
                <SkillDetail skill={sel} project={project} tasks={tasks} now={now} busy={busy} onToggle={() => void toggle()} onRun={() => setRunning(true)} />
              ) : (
                <aside className="flex-[1_1_380px] rounded-[14px] border border-line-2 px-5 py-8 text-center text-sm text-muted">No skills found in this workspace, your home folder or installed plugins.</aside>
              )}
            </div>

            <section className="flex flex-col gap-3">
              <SectionTitle glyph="▦" note="filled = loaded during that task · colour = task status">
                Skill × task
              </SectionTitle>
              <SkillMatrix skills={matrixRows} tasks={projectTasks} selected={sel?.id ?? null} onSelect={setSelected} />
              <p className="font-mono text-[10.5px] leading-[1.6] break-words text-muted">
                {data.transcripts
                  ? `Loads counted from ${data.transcript_files} Claude Code transcript${data.transcript_files === 1 ? "" : "s"} in ${data.transcripts}, matched to tasks by session name or run time.`
                  : "No Claude Code transcripts for this workspace yet, so every load count is 0."}{" "}
                {data.unmatched_loads > 0 && `${data.unmatched_loads} load${data.unmatched_loads === 1 ? "" : "s"} named skills not listed here (bundled with Claude Code, or since removed). `}
                Codex and Gemini do not record skill loads anywhere MAP can read.
                {visible.length > matrixRows.length && (
                  <>
                    {" "}
                    {visible.length - matrixRows.length} skills never loaded during a task are hidden.{" "}
                    <button onClick={() => setAllRows(true)} className="text-link hover:text-link-hover">
                      Show all
                    </button>
                  </>
                )}
                {allRows && (
                  <>
                    {" "}
                    <button onClick={() => setAllRows(false)} className="text-link hover:text-link-hover">
                      Hide unused
                    </button>
                  </>
                )}
              </p>
            </section>
          </>
        )}
      </Main>
      {running && sel && (
        <RunSkillModal
          skill={sel}
          project={project}
          projectName={projectName}
          providers={providers}
          onClose={() => setRunning(false)}
          onCreated={(task) => {
            setRunning(false);
            toast(`Queued task #${task.id} "${task.title}" in ${projectName}`);
            go("command", "tasks");
          }}
        />
      )}
    </>
  );
}
