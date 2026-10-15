"use client";

import { ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { lanesApi } from "@/lib/api";
import type { LaneInfo } from "@/lib/generated/LaneInfo";

import { useApp } from "../app-context";
import { ProjectLanesHead } from "../lanes/add-lane";
import { ImportWatcher } from "../lanes/import-watcher";
import { LaneCard } from "../lanes/lane-card";
import { Btn, errText } from "../project-ui";
import { Card, Main, PageHeader, SectionTitle } from "../shell/page";
import { TopBar } from "../shell/top-bar";

/** Lanes change on the backend's tick, not through the WebSocket; re-read them this often while the screen is open. */
const REFRESH_MS = 10_000;

const TOML_EXAMPLE = `[[lanes]]
name = "developer-myapp"
project = "myapp"             # the project's slug; default: the project whose folder is path
role = "developer"            # manager, developer or another word
socket = ""                   # empty: tmux's default server
session_match = ["my-agent*"] # globs; empty: the name
path = "/path/to/project"     # status file, cards, git`;

/** Manager first, then developer, then other roles, then lanes without one; MAP's manager before a watched one. */
const roleOrder = (l: LaneInfo) => (l.role === "manager" ? 0 : l.role === "developer" ? 1 : l.role ? 2 : 3);
const byRole = (a: LaneInfo, b: LaneInfo) =>
  roleOrder(a) - roleOrder(b) || (a.owned === "manager" ? -1 : b.owned === "manager" ? 1 : 0) || a.label.localeCompare(b.label);

/** Lanes by project (every project of the dashboard, even one without lanes), then the lanes of no project. */
function groupLanes(lanes: LaneInfo[], projects: string[]): [string | null, LaneInfo[]][] {
  const keys = [...new Set([...projects, ...lanes.map((l) => l.project).filter((p): p is string => p !== null)])];
  const out: [string | null, LaneInfo[]][] = keys.map((k) => [k, lanes.filter((l) => l.project === k).sort(byRole)]);
  const loose = lanes.filter((l) => l.project === null).sort(byRole);
  if (loose.length > 0) out.push([null, loose]);
  return out;
}

/** Lanes (`#lanes`, `#lanes/<name>` scrolls to one): agent sessions MAP watches, each belonging to a project. The
 * current project's lanes, or every project's under a header each in "All projects". MAP reports them to Telegram
 * and, when allowed, nudges them (`/clear`, the status prompt) while idle with nobody attached. */
export function LanesScreen() {
  const { route, project, scope, projects } = useApp();
  const [lanes, setLanes] = useState<LaneInfo[] | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const all = scope === "all";

  const load = useCallback(
    () =>
      lanesApi.list().then(
        (l) => {
          setLanes(l);
          setError("");
        },
        (e) => setError(errText(e)),
      ),
    [],
  );

  const refresh = () => {
    setBusy(true);
    void load().finally(() => setBusy(false));
  };

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const focused = route.arg;
  const ready = lanes !== null;
  useEffect(() => {
    if (!focused || !ready) return;
    const t = window.setTimeout(() => document.getElementById(`lane-${focused}`)?.scrollIntoView({ behavior: "smooth", block: "start" }), 30);
    return () => window.clearTimeout(t);
  }, [focused, ready]);

  const replace = (l: LaneInfo) => setLanes((xs) => xs?.map((x) => (x.name === l.name ? l : x)) ?? null);
  const nameOf = (slug: string) => projects.find((p) => p.project.slug === slug)?.project.name ?? slug;

  // The current project's group (the default workspace: lanes of no project); a deep link to a lane of another
  // project shows that lane's group too.
  const linked = focused ? lanes?.find((l) => l.name === focused) : undefined;
  const groups = lanes
    ? groupLanes(
        lanes,
        projects.map((p) => p.project.slug),
      ).filter(([g]) => all || g === project || (linked !== undefined && g === linked.project))
    : [];

  return (
    <>
      <TopBar />
      <Main>
        <PageHeader
          kicker="Watch"
          title="Lanes"
          lead="Every lane belongs to a project: its manager and developer agents, in tmux sessions MAP watches and reports to Telegram. MAP only types into one (/clear, the status prompt) when that is allowed, the session is idle, and nobody is attached."
          actions={
            <Btn size="sm" onClick={refresh} disabled={busy}>
              <RefreshCw className={busy ? "size-3 animate-spin" : "size-3"} />
              Refresh
            </Btn>
          }
        >
          {lanes && lanes.length > 0 && (
            <div className="flex flex-col gap-2">
              <button
                type="button"
                aria-expanded={importOpen}
                onClick={() => setImportOpen(!importOpen)}
                className="flex items-center gap-1.5 self-start font-mono text-[11px] text-text-2 hover:text-text"
              >
                {importOpen ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
                Import watcher settings
              </button>
              {importOpen && (
                <Card className="max-w-[760px] p-4">
                  <ImportWatcher onDone={() => void load()} />
                </Card>
              )}
            </div>
          )}
        </PageHeader>

        {error && <p className="text-[12px] text-bad">{error}</p>}
        {!lanes ? (
          !error && <p className="font-mono text-[11.5px] text-muted">loading…</p>
        ) : (
          <div className="flex min-w-0 flex-col gap-8">
            {groups.map(([group, list]) => (
              <section key={group ?? "-"} className="flex min-w-0 flex-col gap-3">
                {group === null ? (
                  <SectionTitle note="[[lanes]] naming no project and outside every project's folder">No project</SectionTitle>
                ) : (
                  <ProjectLanesHead slug={group} title={nameOf(group)} lanes={list} onDone={() => void load()} />
                )}
                {list.length > 0 && (
                  <div className="grid min-w-0 gap-4 xl:grid-cols-2">
                    {list.map((l) => (
                      <LaneCard key={l.name} lane={l} onChange={replace} onReload={() => void load()} focused={focused === l.name} />
                    ))}
                  </div>
                )}
              </section>
            ))}
            {groups.length === 0 && (
              <Card className="flex max-w-[860px] flex-col gap-3.5 p-[18px]">
                <SectionTitle note="pick a project">No lanes here</SectionTitle>
                <p className="text-[13px] leading-normal text-text-2">
                  Lanes belong to projects. Pick a project in the top bar to set up its manager and developer lanes, or switch to All projects to see every lane.
                </p>
              </Card>
            )}
            {lanes.length === 0 && (
              <Card className="flex max-w-[860px] flex-col gap-3.5 p-[18px]">
                <SectionTitle note="map.toml">Or declare lanes in map.toml</SectionTitle>
                <p className="text-[13px] leading-normal text-text-2">
                  A <code>[[lanes]]</code> block in <code>map.toml</code> watches a tmux session too (restart MAP after adding it). Thresholds, the status prompt and Telegram go in the
                  same file, or come from an agent-watcher config below.
                </p>
                <pre className="overflow-x-auto rounded-lg border border-line bg-inset px-3 py-2 font-mono text-[11.5px] whitespace-pre text-accent-soft">{TOML_EXAMPLE}</pre>
                <div className="border-t border-line pt-3.5">
                  <ImportWatcher onDone={() => void load()} />
                </div>
              </Card>
            )}
          </div>
        )}
      </Main>
    </>
  );
}
