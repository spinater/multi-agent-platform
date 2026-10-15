"use client";

import { ChevronDown, ChevronRight, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { lanesApi } from "@/lib/api";
import type { LaneInfo } from "@/lib/generated/LaneInfo";

import { useApp } from "../app-context";
import { ImportWatcher } from "../lanes/import-watcher";
import { LaneCard } from "../lanes/lane-card";
import { Btn, errText } from "../project-ui";
import { Card, Main, PageHeader, SectionTitle } from "../shell/page";
import { TopBar } from "../shell/top-bar";

/** Lanes change on the backend's tick, not through the WebSocket; re-read them this often while the screen is open. */
const REFRESH_MS = 10_000;

const TOML_EXAMPLE = `[[lanes]]
name = "my-agent"
socket = ""                  # empty: tmux's default server
session_match = ["my-agent*"] # globs; empty: the name
path = "/path/to/project"     # status file, cards, git`;

const WATCHED = "";

/** MAP-owned lanes grouped by project (the manager first), then the `[[lanes]]` blocks. */
function groupLanes(lanes: LaneInfo[]): [string, LaneInfo[]][] {
  const by = new Map<string, LaneInfo[]>();
  for (const l of lanes) {
    const key = l.project ?? WATCHED;
    by.set(key, [...(by.get(key) ?? []), l]);
  }
  const keys = [...by.keys()].filter((k) => k !== WATCHED).sort();
  if (by.has(WATCHED)) keys.push(WATCHED);
  return keys.map((k) => [k, (by.get(k) ?? []).sort((a, b) => (a.owned === "manager" ? -1 : b.owned === "manager" ? 1 : a.name.localeCompare(b.name)))]);
}

/** Lanes (`#lanes`, `#lanes/<name>` scrolls to one): tmux sessions someone else owns, which MAP watches, reports to
 * Telegram and, when allowed, nudges (`/clear`, the status prompt) while idle with nobody attached. */
export function LanesScreen() {
  const { route } = useApp();
  const [lanes, setLanes] = useState<LaneInfo[] | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [importOpen, setImportOpen] = useState(false);

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

  const replace = (l: LaneInfo) => setLanes((all) => all?.map((x) => (x.name === l.name ? l : x)) ?? null);

  return (
    <>
      <TopBar />
      <Main>
        <PageHeader
          kicker="Watch"
          title="Lanes"
          lead="Lanes are tmux sessions someone else owns, on tmux's default server or a named socket. MAP watches them and notifies Telegram. It only types into one (/clear, the status prompt) when that is allowed, the session is idle, and nobody is attached."
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
        ) : lanes.length === 0 ? (
          <Card className="flex max-w-[860px] flex-col gap-3.5 p-[18px]">
            <SectionTitle note="map.toml">No lanes yet</SectionTitle>
            <p className="text-[13px] leading-normal text-text-2">
              Add a <code>[[lanes]]</code> block to <code>map.toml</code> for each tmux session to watch, then restart MAP. Thresholds, the status prompt and
              Telegram go in the same file, or come from an agent-watcher config below.
            </p>
            <pre className="overflow-x-auto rounded-lg border border-line bg-inset px-3 py-2 font-mono text-[11.5px] whitespace-pre text-accent-soft">{TOML_EXAMPLE}</pre>
            <div className="border-t border-line pt-3.5">
              <ImportWatcher onDone={() => void load()} />
            </div>
          </Card>
        ) : (
          <div className="flex min-w-0 flex-col gap-6">
            {groupLanes(lanes).map(([group, list]) => (
              <section key={group} className="flex min-w-0 flex-col gap-3">
                <SectionTitle note={group === WATCHED ? "map.toml [[lanes]]" : "MAP's own: the manager and persistent lanes"}>{group === WATCHED ? "Watched lanes" : `Project ${group}`}</SectionTitle>
                <div className="grid min-w-0 gap-4 xl:grid-cols-2">
                  {list.map((l) => (
                    <LaneCard key={l.name} lane={l} onChange={replace} onReload={() => void load()} focused={focused === l.name} />
                  ))}
                </div>
              </section>
            ))}
          </div>
        )}
      </Main>
    </>
  );
}
