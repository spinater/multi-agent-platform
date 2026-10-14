"use client";

/** Knowledge (`#knowledge`, `#knowledge/<slug>`, `#knowledge/<slug>/doc/<path>`): a project's docs (knowledge cards
 * and their index, requirements, task sheets, rulebooks) plus what MAP remembers of finished tasks and approved plans.
 * Health checks with ready fix tasks, byte budgets, search, browse, a doc viewer, and what agents searched for. */

import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { api } from "@/lib/api";
import type { KnowledgeOverview } from "@/lib/generated/KnowledgeOverview";

import { useApp } from "../app-context";
import { DocViewer } from "../knowledge/doc-viewer";
import { Box, Browse, Budgets, Health, Queries, SearchBox } from "../knowledge/sections";
import { KIND_LABEL, knowledgeHash } from "../knowledge/shared";
import { Suggestion } from "../knowledge/suggestion";
import { Btn, Inset, ago, errText } from "../project-ui";
import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";
import { Empty } from "../ui";

/** Agents' searches arrive without a push; read the overview again this often. */
const REFRESH_MS = 60_000;

type Loaded = { slug: string; data: KnowledgeOverview } | { slug: string; error: string };

/** `<slug>` or `<slug>/doc/<path>` (the path may itself contain `/`, or be `map://…`). */
function parseArg(arg: string | null): { slug: string | null; doc: string | null } {
  if (!arg) return { slug: null, doc: null };
  const [slug, sub, ...rest] = arg.split("/");
  return { slug: slug || null, doc: sub === "doc" && rest.length ? rest.join("/") : null };
}

export function KnowledgeScreen() {
  const { route, go, projects, project, setProject, setScope, connection, now, toast } = useApp();
  const arg = parseArg(route.arg);
  // The route's project, else the current one, else the first: knowledge belongs to a project folder.
  const slug = arg.slug ?? (projects.some((p) => p.project.slug === project) ? project : (projects[0]?.project.slug ?? null));
  const info = projects.find((p) => p.project.slug === slug) ?? null;
  const projectName = info?.project.name ?? slug ?? "";

  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!slug) return;
    let live = true;
    api.knowledge(slug).then(
      (data) => live && setLoaded({ slug, data }),
      (e) => live && setLoaded((l) => (l && l.slug === slug && "data" in l ? l : { slug, error: errText(e) })),
    );
    const timer = window.setInterval(() => setTick((t) => t + 1), REFRESH_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [slug, tick]);

  const ours = loaded && loaded.slug === slug ? loaded : null;
  const data = ours && "data" in ours ? ours.data : null;
  const known = useMemo(() => new Set(data?.docs.map((d) => d.path) ?? []), [data]);

  // Opening a doc changes only the hash (no scroll to top), so the page stays where it was under the modal.
  const openDoc = useCallback(
    (path: string) => {
      if (slug) location.hash = knowledgeHash(slug, path);
    },
    [slug],
  );
  const closeDoc = useCallback(() => {
    if (slug) location.hash = knowledgeHash(slug);
  }, [slug]);

  const pick = (s: string) => {
    setProject(s);
    setScope("project");
    go("knowledge", s);
  };

  const reindex = async () => {
    if (!slug) return;
    setBusy(true);
    try {
      const next = await api.reindexKnowledge(slug);
      setLoaded({ slug, data: next });
      toast(`${projectName}: ${next.docs.length} documents indexed`);
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  if (!slug) {
    return (
      <>
        <TopBar />
        <Main>
          <PageHeader kicker="Knowledge" title="Knowledge" lead="Each project's docs, their health, and what agents look up." />
          {connection === "connecting" ? (
            <Empty>Loading projects…</Empty>
          ) : (
            <Box title="No projects yet">
              <p className="text-[13px] text-text-2">Knowledge belongs to a project&apos;s folder. Create a project, then come back to see its cards, rulebooks and health.</p>
              <div>
                <Btn tone="primary" onClick={() => go("projects", "new")}>
                  + New project
                </Btn>
              </div>
            </Box>
          )}
        </Main>
      </>
    );
  }

  const s = data?.settings;
  const cardDir = s ? s.index.replace(/\/[^/]*$/, "") || "." : "";

  return (
    <>
      <TopBar />
      <Main>
        <PageHeader
          kicker={`Project · ${projectName}`}
          title="Knowledge"
          lead="The docs in the project folder (knowledge cards and their index, requirements, task sheets, rulebooks) and what MAP remembers of finished tasks and approved plans."
          actions={
            <>
              <label className="flex items-center gap-2 font-mono text-[11px] text-muted">
                project
                <select
                  value={slug}
                  onChange={(e) => pick(e.target.value)}
                  aria-label="Project"
                  className="h-[34px] max-w-[220px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[12.5px] text-text"
                >
                  {!info && <option value={slug}>{slug}</option>}
                  {projects.map((p) => (
                    <option key={p.project.slug} value={p.project.slug}>
                      {p.project.name}
                    </option>
                  ))}
                </select>
              </label>
              <Btn onClick={() => go("projects", slug)}>Project page</Btn>
              <Btn tone="primary" onClick={() => void reindex()} disabled={busy}>
                <RefreshCw className={busy ? "size-3.5 animate-spin" : "size-3.5"} />
                {busy ? "Reindexing…" : "Reindex"}
              </Btn>
            </>
          }
        >
          {data && (
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 font-mono text-[11px] text-muted">
              <span className="break-all text-text-2" title="project folder">
                {data.root}
              </span>
              <span>
                index{" "}
                {data.index_path ? (
                  known.has(data.index_path) ? (
                    <a
                      href={knowledgeHash(slug, data.index_path)}
                      onClick={(e) => {
                        e.preventDefault();
                        openDoc(data.index_path!);
                      }}
                      className="text-link hover:text-link-hover"
                    >
                      {data.index_path}
                    </a>
                  ) : (
                    <span className="text-text-2">{data.index_path}</span>
                  )
                ) : (
                  <span className="text-warn">none</span>
                )}
              </span>
              <span>{data.indexed_at ? `indexed ${ago(new Date(data.indexed_at).getTime(), now)}` : "not indexed yet"}</span>
              <span>{data.counts.filter((c) => c.count > 0).map((c) => `${c.count} ${KIND_LABEL[c.kind]}`).join(" · ") || "no documents"}</span>
            </div>
          )}
        </PageHeader>

        {ours && "error" in ours && <p className="rounded-lg border border-bad/40 bg-bad/10 px-3 py-2 text-sm text-bad">Could not read {projectName}&apos;s knowledge: {ours.error}</p>}
        {!ours && <Empty>Indexing {projectName}…</Empty>}

        {data && s && (
          <>
            <Suggestion key={slug} slug={slug} data={data} onChange={(next) => (next ? setLoaded({ slug, data: next }) : setTick((t) => t + 1))} />
            {data.docs.length === 0 && (
              <Box title="Nothing to know yet" glyph="◇">
                <p className="text-[13px] leading-normal text-text-2">
                  MAP found no documents in {s.dirs.length ? s.dirs.join(", ") : "the indexed folders"} and none of {s.rulebooks.join(", ") || "the rulebooks"} exist. Agents work best
                  with a few short, current documents they can search before they read code:
                </p>
                <ul className="flex flex-col gap-1 font-mono text-[12px] leading-snug text-text-2">
                  <li>· {s.rulebooks[0] ?? "CLAUDE.md"}: the rules every request loads (keep it under {Math.round(s.rulebook_byte_cap / 1000)} KB).</li>
                  <li>· {s.index}: one line per card, so an agent reads the index first and opens only the card it needs.</li>
                  <li>· {cardDir}/&lt;topic&gt;.md: one card per topic, under {s.card_line_cap} lines, with sources: in its frontmatter listing the files it summarises.</li>
                  <li>· requirements, specs and task sheets in {s.dirs.join(", ") || "an indexed folder"}.</li>
                </ul>
                <p className="text-[12px] text-muted">Finished tasks and approved plans are added here as MAP memory on their own. Folders and limits are in the project&apos;s settings (Knowledge).</p>
              </Box>
            )}

            {!data.index_path && data.docs.length > 0 && (
              <Inset className="gap-1.5 border-warn/40">
                <span className="text-[12.5px] font-semibold text-warn">No index at {s.index}</span>
                <span className="text-text-2">
                  The convention: one card per topic in {cardDir}/, each with frontmatter sources: listing the files it summarises, and {s.index.split("/").pop()} listing every card with a
                  one-line summary. Agents read the index first, so a card it does not list is a card they never find. Health below can queue a task that writes it.
                </span>
              </Inset>
            )}

            <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
              <Health data={data} projectName={projectName} known={known} onOpen={openDoc} />
              <Budgets data={data} known={known} onOpen={openDoc} />
            </div>
            <SearchBox data={data} onOpen={openDoc} />
            <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
              <Browse data={data} now={now} onOpen={openDoc} />
              <Queries data={data} now={now} />
            </div>
          </>
        )}
      </Main>
      {arg.doc && data && s && (
        <DocViewer key={arg.doc} slug={slug} path={arg.doc} known={known} settings={s} now={now} onOpen={openDoc} onClose={closeDoc} />
      )}
    </>
  );
}
