"use client";

import { ArrowLeft, ChevronLeft, ChevronRight, GitBranch, RefreshCw, Search } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { Session } from "@/lib/generated/Session";
import type { TaskPage } from "@/lib/generated/TaskPage";
import type { TaskPageItem } from "@/lib/generated/TaskPageItem";

import { useApp } from "../app-context";
import { rel } from "../command-ui";
import { Btn, errText } from "../project-ui";
import { Card, Main, PageHeader } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";

const PAGE = 50;

/** `1h 4m`, `3m 20s`, `12s`. */
function span(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  if (s >= 60) return `${Math.floor(s / 60)}m ${s % 60}s`;
  return `${s}s`;
}

function stamp(iso: string): string {
  const d = new Date(iso);
  return `${d.toLocaleDateString(undefined, { day: "2-digit", month: "short" })} ${d.toTimeString().slice(0, 5)}`;
}

/** The Done tasks page (`#command/done`): finished tasks, off the command centre's live views. The current project's
 * (or every project's, in the all-projects scope), newest finished first, searched by title or `#id` on the server. */
export function DoneTasksScreen() {
  const { scope, project, projects, server, now, go, tasks, sessions } = useApp();
  const all = scope === "all";
  const [q, setQ] = useState("");
  const [query, setQuery] = useState("");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<TaskPage | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  // Typing settles for a moment before the server is asked; a new search starts at the first page.
  useEffect(() => {
    const t = window.setTimeout(() => {
      setQuery(q);
      setOffset(0);
    }, 250);
    return () => window.clearTimeout(t);
  }, [q]);
  const [lastScope, setLastScope] = useState(`${scope}|${project ?? ""}`);
  if (`${scope}|${project ?? ""}` !== lastScope) {
    setLastScope(`${scope}|${project ?? ""}`);
    setOffset(0);
  }

  // A task finishing (or a done one removed) while the page is open reloads it.
  const doneNow = tasks.filter((t) => t.status === "done").length;
  const load = useCallback(
    () =>
      api.taskPage({ status: "done", project: all ? undefined : project, q: query, limit: PAGE, offset }).then(
        (p) => {
          setPage(p);
          setError("");
        },
        (e) => setError(errText(e)),
      ),
    [all, project, query, offset],
  );
  const refresh = () => {
    setBusy(true);
    void load().finally(() => setBusy(false));
  };
  useEffect(() => {
    void load();
  }, [load, doneNow]);

  const projectName = (slug: string | null) => (slug == null ? workspaceName(server?.workspace_dir) : (projects.find((p) => p.project.slug === slug)?.project.name ?? slug));
  const where = all ? "all projects" : projectName(project);
  const total = page?.total ?? 0;
  const open = (id: number) => go("command", `tasks/${id}`);

  return (
    <>
      <TopBar />
      <Main>
        <PageHeader
          kicker="Command centre"
          title="Done tasks"
          lead={`Finished tasks of ${where}, newest first. They are kept here, off the command centre's live views; failed and cancelled tasks stay in the command centre.`}
          actions={
            <>
              <Btn size="sm" onClick={() => go("command")}>
                <ArrowLeft className="size-3" /> Command centre
              </Btn>
              <Btn size="sm" onClick={refresh} disabled={busy}>
                <RefreshCw className={busy ? "size-3 animate-spin" : "size-3"} /> Refresh
              </Btn>
            </>
          }
        />
        <div className="flex flex-wrap items-center justify-between gap-3">
          <label className="flex h-[34px] w-full max-w-[360px] items-center gap-2 rounded-[7px] border border-line-2 bg-inset px-2.5">
            <Search className="size-3.5 text-muted" />
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="title or #id"
              aria-label="Search done tasks"
              className="min-w-0 flex-1 bg-transparent font-mono text-[12.5px] text-text outline-none placeholder:text-muted/80"
            />
          </label>
          <Pager offset={offset} total={total} onOffset={setOffset} />
        </div>
        {error && <p className="font-mono text-[12px] text-bad">{error}</p>}
        <Card className="overflow-hidden">
          {page && page.items.length === 0 ? (
            <p className="px-4 py-6 font-mono text-[12px] text-muted">{query.trim() ? "No done task matches." : "No done tasks yet."}</p>
          ) : (
            <div className="flex flex-col">
              {(page?.items ?? []).map((item) => (
                <Row key={item.task.id} item={item} now={now} ran={sessions.find((s) => s.name === item.task.session)} projectName={all ? projectName : undefined} onOpen={open} />
              ))}
            </div>
          )}
        </Card>
        {total > PAGE && (
          <div className="flex justify-end">
            <Pager offset={offset} total={total} onOffset={setOffset} />
          </div>
        )}
      </Main>
    </>
  );
}

function Pager({ offset, total, onOffset }: { offset: number; total: number; onOffset: (n: number) => void }) {
  const last = Math.min(offset + PAGE, total);
  return (
    <span className="flex items-center gap-2 font-mono text-[11px] text-muted">
      {total ? `${offset + 1}–${last} of ${total}` : "0 tasks"}
      <Btn size="sm" onClick={() => onOffset(Math.max(0, offset - PAGE))} disabled={offset === 0} aria-label="Newer">
        <ChevronLeft className="size-3" />
      </Btn>
      <Btn size="sm" onClick={() => onOffset(offset + PAGE)} disabled={last >= total} aria-label="Older">
        <ChevronRight className="size-3" />
      </Btn>
    </span>
  );
}

/** `ran`: the task's last session, when MAP still has it (it names the provider and model that did the work). */
function Row({ item, now, ran, projectName, onOpen }: { item: TaskPageItem; now: number; ran?: Session; projectName?: (slug: string | null) => string; onOpen: (id: number) => void }) {
  const t = item.task;
  const provider = ran?.provider ?? t.provider;
  const model = ran?.model ?? t.model;
  const started = item.started_at ?? t.created_at;
  const took = new Date(t.updated_at).getTime() - new Date(started).getTime();
  const summary = t.result?.split("\n").find((l) => l.trim()) ?? "";
  const commit = t.git?.merged?.slice(0, 10);
  return (
    <div className="grid grid-cols-[56px_minmax(0,1fr)] gap-x-3 gap-y-1 border-b border-line px-4 py-3 last:border-b-0 sm:grid-cols-[56px_minmax(0,1fr)_auto]">
      <button onClick={() => onOpen(t.id)} className="cursor-pointer self-start text-left font-mono text-[12px] text-accent-soft hover:underline" title="Open in the command centre">
        #{t.id}
      </button>
      <div className="flex min-w-0 flex-col gap-1">
        <button onClick={() => onOpen(t.id)} className="cursor-pointer text-left text-[13.5px] leading-[1.3] font-medium text-text hover:underline">
          {projectName && <span className="mr-1.5 font-mono text-[11px] text-muted">{projectName(t.project)}</span>}
          {t.title}
        </button>
        {summary && <p className="line-clamp-2 text-[12.5px] leading-[1.45] text-text-2">{summary}</p>}
        <span className="flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[10.5px] text-muted">
          {provider && (
            <span className="flex items-center gap-1.5">
              <span className="size-1.5 rounded-full" style={{ background: providerColor(provider) }} />
              {provider}
              {model ? ` · ${model}` : ""}
            </span>
          )}
          {t.git && (
            <span className="flex items-center gap-1" title={t.git.note ?? undefined}>
              <GitBranch className="size-3" />
              {t.git.branch}
              {commit ? ` → ${commit}` : ""}
              {t.git.pr_url && (
                <a href={t.git.pr_url} target="_blank" rel="noreferrer" className="text-accent-soft hover:underline">
                  PR
                </a>
              )}
            </span>
          )}
          {t.attempts > 1 && <span>{t.attempts} attempts</span>}
        </span>
      </div>
      <div className="col-start-2 flex flex-col items-start gap-0.5 font-mono text-[10.5px] text-muted sm:col-start-auto sm:items-end">
        <span title={new Date(t.updated_at).toLocaleString()}>
          {stamp(t.updated_at)} · {rel(t.updated_at, now)}
        </span>
        <span title={item.started_at ? "from its first session's start" : "from when it was created"}>took {span(took)}</span>
      </div>
    </div>
  );
}
