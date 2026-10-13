"use client";

import clsx from "clsx";
import { Search } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { cloneUrl } from "@/lib/api";
import type { CheckLine } from "@/lib/generated/CheckLine";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";

import { useApp } from "../app-context";
import { ProjectDetail } from "../projects";
import { ProviderChip, STATUS_TONE, StatusPill, TaskBar, ago, figures, projectProviders, useDefaults } from "../project-ui";
import { ProjectWizard } from "../project-wizard";
import { Main, PageHeader } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";

const LEVEL_TEXT: Record<CheckLine["level"], string> = { success: "text-ok", info: "text-info", warning: "text-warn", error: "text-bad" };
const LEVEL_MARK: Record<CheckLine["level"], string> = { success: "✓", info: "·", warning: "!", error: "✗" };

/** Check lines from a create, revealed one by one on the new project's card. */
type Reveal = { slug: string; checks: CheckLine[]; shown: number };

/** Projects (Projects.dc.html): one card per project plus the default workspace, the "New project" wizard at
 * `#projects/new`, and a project's page at `#projects/<slug>`. */
export function ProjectsScreen() {
  const { route, go } = useApp();
  const [reveal, setReveal] = useState<Reveal | null>(null);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => {
    if (!reveal) return;
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(
      () => setReveal((r) => (r && r.shown < r.checks.length ? { ...r, shown: r.shown + 1 } : null)),
      reveal.shown < reveal.checks.length ? 420 : 2400,
    );
    return () => window.clearTimeout(timer.current);
  }, [reveal]);

  const detail = route.arg && route.arg !== "new" ? route.arg : null;
  return (
    <>
      <TopBar />
      {detail ? (
        <ProjectDetail slug={detail} />
      ) : (
        <ProjectList reveal={reveal} />
      )}
      {route.arg === "new" && (
        <ProjectWizard
          onClose={() => go("projects")}
          onCreated={(slug, checks) => {
            go("projects");
            setReveal({ slug, checks, shown: 0 });
          }}
        />
      )}
    </>
  );
}

type Card = {
  key: string;
  slug: string | null;
  name: string;
  path: string;
  repo: string;
  info: ProjectInfo | null;
};

function ProjectList({ reveal }: { reveal: Reveal | null }) {
  const { projects, tasks, sessions, orchestrator, providers, server, now, go, setProject, setScope, scope, project, openAuth } = useApp();
  const defaults = useDefaults();
  const [query, setQuery] = useState("");

  const origin = typeof location === "undefined" ? "" : location.origin;
  const cards: Card[] = [
    {
      key: "",
      slug: null,
      name: workspaceName(server?.workspace_dir),
      path: server?.workspace_dir ?? "",
      repo: "default workspace · workspace.dir in map.toml",
      info: null,
    },
    ...projects.map((p) => {
      const src = p.project.settings.source;
      const repo =
        src.kind === "empty"
          ? `${cloneUrl(p.project.slug, "code").replace(origin, "")} · main`
          : `${src.location} @ ${src.branch ?? "default"} → ${cloneUrl(p.project.slug, "code").replace(origin, "")}`;
      return { key: p.project.slug, slug: p.project.slug, name: p.project.name, path: p.workspace, repo, info: p };
    }),
  ];
  const needle = query.trim().toLowerCase();
  const shown = cards.filter((c) => !needle || [c.name, c.path, c.slug ?? "", c.repo].some((x) => x.toLowerCase().includes(needle)));
  const all = cards.map((c) => figures(c.slug, c.info, tasks, sessions, orchestrator));
  const live = all.reduce((a, f) => a + f.live, 0);
  const running = all.filter((f) => f.status === "running").length;
  const attention = all.reduce((a, f) => a + f.attention, 0);

  const open = (slug: string | null, screen: "command" | "skills" | "decisions") => {
    setProject(slug);
    setScope("project");
    go(screen);
  };

  return (
    <Main className="gap-7">
      <PageHeader
        kicker="Workspaces"
        title="Projects"
        lead="Each project is a code repo, a corpus repo and its own Reflex engine, with its own providers, verify commands and task history. Tasks without a project run in the default workspace. Pick one to open its command centre."
        actions={
          <>
            <label className="flex h-9 items-center gap-2 rounded-lg border border-line-2 bg-surface px-3">
              <Search className="size-3.5 text-muted" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="filter projects"
                aria-label="Filter projects"
                className="w-40 border-0 bg-transparent font-mono text-[12.5px] text-text outline-none placeholder:text-muted/70"
              />
            </label>
            <button onClick={() => go("projects", "new")} className="h-9 rounded-lg border border-accent bg-accent px-4 text-[13px] font-semibold whitespace-nowrap text-accent-text hover:opacity-90">
              + New project
            </button>
          </>
        }
      />

      <div className="flex flex-wrap gap-x-[22px] gap-y-1.5 font-mono text-[11px] tracking-[0.04em] text-muted">
        <span>{cards.length} projects</span>
        <span>{live} live sessions</span>
        <span>
          {running} running{orchestrator && !orchestrator.running ? " · orchestrator paused" : ""}
        </span>
        <span>{attention} items need you</span>
      </div>

      <div className="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(min(330px,100%),1fr))]">
        {shown.map((c) => {
          const f = figures(c.slug, c.info, tasks, sessions, orchestrator);
          const current = scope === "project" && project === c.slug;
          const revealing = reveal && reveal.slug === c.slug ? reveal : null;
          const status = revealing ? "initializing" : f.status;
          const provs = projectProviders(c.info, defaults, providers);
          return (
            <div
              key={c.key}
              className={clsx(
                "flex min-w-0 flex-col gap-4 rounded-[14px] border px-[18px] pt-[18px] pb-4",
                current ? "border-accent/55 bg-[linear-gradient(180deg,rgba(255,122,26,.06),rgba(255,255,255,.015))]" : "border-white/[.08] bg-surface",
              )}
            >
              <div className="flex items-start gap-2.5">
                <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="size-2 shrink-0 rounded-full" style={{ background: STATUS_TONE[status].color }} />
                    {c.slug ? (
                      <a
                        href={`#projects/${c.slug}`}
                        onClick={(e) => {
                          e.preventDefault();
                          go("projects", c.slug);
                        }}
                        className="text-[17px] leading-tight font-semibold tracking-[-0.01em] break-words text-text hover:text-accent-soft"
                      >
                        {c.name}
                      </a>
                    ) : (
                      <span className="text-[17px] leading-tight font-semibold tracking-[-0.01em] break-words">{c.name}</span>
                    )}
                    {current && <span className="font-mono text-[9px] font-semibold tracking-[0.14em] whitespace-nowrap text-accent">CURRENT</span>}
                  </div>
                  <span className="font-mono text-[11px] leading-snug break-all text-text-2">{c.path}</span>
                  <span className="font-mono text-[11px] leading-snug break-all text-muted">{c.repo}</span>
                </div>
                <StatusPill status={status} />
              </div>

              {revealing ? (
                <div className="flex flex-col gap-1 rounded-lg border border-line bg-inset px-3 py-2.5 font-mono text-[11px] leading-[1.6]">
                  <span className="text-muted">$ create {c.slug}</span>
                  {revealing.checks.slice(0, revealing.shown).map((l, i) => (
                    <span key={i} className={clsx("break-words", LEVEL_TEXT[l.level])}>
                      {LEVEL_MARK[l.level]} {l.text}
                    </span>
                  ))}
                </div>
              ) : (
                <TaskBar f={f} />
              )}

              <div className="flex flex-wrap items-center gap-2">
                {provs.map((name) => (
                  <ProviderChip key={name} name={name} info={providers.find((p) => p.name === name)} onSignIn={openAuth} />
                ))}
                <span className="font-mono text-[10.5px] whitespace-nowrap text-muted">prefix {server?.session_prefix}</span>
                {c.info && c.info.reflex.state !== "running" && <span className="font-mono text-[10.5px] whitespace-nowrap text-muted">reflex {c.info.reflex.state.replace("_", " ")}</span>}
              </div>

              <div className="flex flex-wrap items-center justify-between gap-2.5 border-t border-line pt-3">
                <span className="font-mono text-[11px] leading-snug text-muted">
                  {f.live} live · {ago(f.last, now)}
                  {f.attention > 0 && <span className="text-warn"> · {f.attention} need you</span>}
                </span>
                <div className="flex gap-1.5">
                  <SmallBtn onClick={() => open(c.slug, "skills")}>Skills</SmallBtn>
                  <SmallBtn onClick={() => open(c.slug, "decisions")}>Decisions</SmallBtn>
                  <button
                    onClick={() => open(c.slug, "command")}
                    className={clsx("h-7 rounded-md border border-accent px-3 text-[11.5px] font-semibold whitespace-nowrap", current ? "bg-accent text-accent-text" : "text-accent-soft hover:bg-accent-tint")}
                  >
                    Open →
                  </button>
                </div>
              </div>
            </div>
          );
        })}
        <button
          onClick={() => go("projects", "new")}
          className="flex min-h-60 flex-col items-center justify-center gap-2.5 rounded-[14px] border border-dashed border-line-3 text-text-2 hover:border-accent hover:text-accent-soft"
        >
          <span className="text-3xl leading-none font-light">+</span>
          <span className="text-[13px] font-semibold">New project</span>
          <span className="font-mono text-[11px] text-muted">new repo, local repo or git URL</span>
        </button>
      </div>
    </Main>
  );
}

function SmallBtn({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button onClick={onClick} className="h-7 rounded-md border border-line-2 px-2.5 text-[11.5px] font-medium whitespace-nowrap text-nav hover:text-text">
      {children}
    </button>
  );
}
