"use client";

/** The Knowledge screen's sections: health checks with fix tasks, byte budgets, search, browse, and the searches
 * agents ran. Each takes the loaded overview and opens documents through `onOpen`. */

import clsx from "clsx";
import { Search } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { KnowledgeCheck } from "@/lib/generated/KnowledgeCheck";
import type { KnowledgeDoc } from "@/lib/generated/KnowledgeDoc";
import type { KnowledgeIssue } from "@/lib/generated/KnowledgeIssue";
import type { KnowledgeKind } from "@/lib/generated/KnowledgeKind";
import type { KnowledgeOverview } from "@/lib/generated/KnowledgeOverview";
import type { KnowledgeSearch } from "@/lib/generated/KnowledgeSearch";

import { useApp } from "../app-context";
import { fmtBytes } from "../branches";
import { LEVEL_TONE, TONE_TEXT } from "../command-ui";
import { Btn, ago, errText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";
import { Badge, Dot } from "../ui";
import { Snippet } from "./markdown";
import { checkText, fmtKB, KIND_LABEL, KIND_TONE, KINDS, knowledgeHash, plural } from "./shared";

type Open = (path: string) => void;

/** Issues shown per check before "Show all". */
const ISSUE_LIMIT = 6;
/** Documents shown per kind before "Show all". */
const DOC_LIMIT = 12;
/** Wait this long after the last keystroke before searching. */
const SEARCH_DEBOUNCE_MS = 300;

export function Box({ title, note, glyph, actions, children, className }: { title: ReactNode; note?: ReactNode; glyph?: string; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <Card className={clsx("flex flex-col gap-3.5 p-[18px]", className)}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <SectionTitle glyph={glyph} note={note}>
          {title}
        </SectionTitle>
        {actions}
      </div>
      {children}
    </Card>
  );
}

export function Chip({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={clsx(
        "flex h-7 items-center gap-1.5 rounded-md border px-2.5 font-mono text-[11.5px] font-medium whitespace-nowrap",
        active ? "border-accent bg-accent/10 text-text" : "border-line-2 text-text-2 hover:border-line-3",
      )}
    >
      {children}
    </button>
  );
}

/** A path that opens the viewer when it is an indexed document, plain text otherwise. */
function PathLink({ slug, path, known, onOpen }: { slug: string; path: string; known: Set<string>; onOpen: Open }) {
  if (!path) return null;
  if (!known.has(path)) return <span className="font-mono text-[12px] break-all text-text">{path}</span>;
  return (
    <a
      href={knowledgeHash(slug, path)}
      onClick={(e) => {
        e.preventDefault();
        onOpen(path);
      }}
      className="font-mono text-[12px] break-all text-link hover:text-link-hover"
    >
      {path}
    </a>
  );
}

// ---- health

export function Health({ data, projectName, known, onOpen }: { data: KnowledgeOverview; projectName: string; known: Set<string>; onOpen: Open }) {
  const { toast } = useApp();
  const [expanded, setExpanded] = useState<Set<KnowledgeCheck>>(new Set());
  const [busy, setBusy] = useState<string | null>(null);

  // Issues come most serious first, so the groups keep that order.
  const groups = useMemo(() => {
    const m = new Map<KnowledgeCheck, KnowledgeIssue[]>();
    for (const i of data.issues) m.set(i.check, [...(m.get(i.check) ?? []), i]);
    return [...m.entries()];
  }, [data.issues]);
  const count = (level: KnowledgeIssue["level"]) => data.issues.filter((i) => i.level === level).length;

  const queue = async (key: string, title: string, prompt: string, what: string) => {
    if (!confirm(`Queue a task in ${projectName}?\n\n${title}\n\n${what}`)) return;
    setBusy(key);
    try {
      const task = await api.createTask({ title, prompt, depends_on: [], project: data.slug });
      toast(`Queued task #${task.id} "${task.title}" in ${projectName}`);
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(null);
    }
  };

  const stale = data.issues.filter((i) => i.check === "stale" && i.fix_prompt);
  const fixStale = () => {
    const prompt = [
      `These ${stale.length} knowledge cards are stale: files listed in their \`sources:\` changed after the card was last updated. Fix each one in turn, then make sure ${data.settings.index} still describes them.`,
      ...stale.map((i, n) => `## ${n + 1}. ${i.path}\n\n${i.detail}\n\n${i.fix_prompt}`),
    ].join("\n\n");
    void queue("stale-all", `Refresh ${plural(stale.length, "stale knowledge card")}`, prompt, stale.map((i) => `· ${i.path}`).join("\n"));
  };

  return (
    <Box
      title="Health"
      glyph="✚"
      note={data.issues.length ? "most serious first" : undefined}
      actions={
        <div className="flex flex-wrap items-center gap-1.5">
          {data.issues.length === 0 ? (
            <Badge tone="ok">all clear</Badge>
          ) : (
            <>
              <Badge tone="bad">{plural(count("error"), "error")}</Badge>
              <Badge tone="warn">{plural(count("warning"), "warning")}</Badge>
              <Badge tone="info">{count("info")} info</Badge>
            </>
          )}
        </div>
      }
    >
      {groups.length === 0 && <p className="text-[13px] text-text-2">No problems found: every card is fresh, under its cap and in the index, and the budgets hold.</p>}
      {groups.length > 0 && (
        <div className="flex flex-col gap-3">
          {groups.map(([check, issues]) => {
            const { label, why } = checkText(check, data.settings);
            const open = expanded.has(check);
            const shown = open ? issues : issues.slice(0, ISSUE_LIMIT);
            return (
              <div key={check} className="flex flex-col gap-2 border-t border-line pt-3 first:border-0 first:pt-0">
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <div className="min-w-0">
                    <h3 className="text-[13.5px] font-semibold text-text">
                      {label} <span className="font-mono text-[11px] font-normal text-muted">· {issues.length}</span>
                    </h3>
                    <p className="font-mono text-[11px] leading-snug text-muted">{why}</p>
                  </div>
                  {check === "stale" && stale.length > 1 && (
                    <Btn size="sm" disabled={busy !== null} onClick={fixStale} title="One task that refreshes every stale card">
                      Fix all stale cards
                    </Btn>
                  )}
                </div>
                <ul className="flex flex-col gap-1.5">
                  {shown.map((i, n) => {
                    const key = `${check}-${i.path}-${n}`;
                    const tone = LEVEL_TONE[i.level];
                    return (
                      <li key={key} className="flex flex-wrap items-start gap-x-3 gap-y-1 rounded-lg border border-line bg-inset px-3 py-2">
                        <span className="mt-[7px]">
                          <Dot tone={tone} />
                        </span>
                        <div className="flex min-w-0 flex-[1_1_260px] flex-col gap-0.5">
                          <PathLink slug={data.slug} path={i.path} known={known} onOpen={onOpen} />
                          <span className={clsx("text-[12.5px] leading-snug", i.level === "error" ? TONE_TEXT[tone] : "text-text-2")}>{i.detail}</span>
                        </div>
                        {i.fix_title && i.fix_prompt ? (
                          <Btn size="sm" disabled={busy !== null} onClick={() => void queue(key, i.fix_title!, i.fix_prompt!, i.detail)} title={i.fix_prompt}>
                            {busy === key ? "Queuing…" : "Create fix task"}
                          </Btn>
                        ) : (
                          <span className="self-center font-mono text-[10.5px] text-muted">needs your decision</span>
                        )}
                      </li>
                    );
                  })}
                </ul>
                {issues.length > ISSUE_LIMIT && (
                  <button
                    type="button"
                    onClick={() => setExpanded((s) => (s.has(check) ? new Set([...s].filter((c) => c !== check)) : new Set([...s, check])))}
                    className="self-start font-mono text-[11px] text-link hover:text-link-hover"
                  >
                    {open ? "Show fewer" : `Show all ${issues.length}`}
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </Box>
  );
}

// ---- budgets

/** Bytes against a warn line and a cap: green under warn, amber from warn, red over the cap. */
function BudgetBar({ value, warn, cap }: { value: number; warn: number; cap: number }) {
  const scale = Math.max(cap, value, 1);
  const tone = value > cap ? "bg-bad" : value >= warn ? "bg-warn" : "bg-ok";
  return (
    <div className="relative h-1.5 overflow-visible rounded-[3px] bg-white/5">
      <div className={clsx("h-full rounded-[3px]", tone)} style={{ width: `${Math.max(1, (100 * value) / scale)}%` }} />
      <span className="absolute -top-0.5 h-2.5 w-px bg-warn/70" style={{ left: `${(100 * warn) / scale}%` }} title="warn" />
      <span className="absolute -top-0.5 h-2.5 w-px bg-bad/80" style={{ left: `calc(${(100 * cap) / scale}% - 1px)` }} title="cap" />
    </div>
  );
}

export function Budgets({ data, known, onOpen }: { data: KnowledgeOverview; known: Set<string>; onOpen: Open }) {
  const s = data.settings;
  return (
    <Box title="Budgets" glyph="▥" note="loaded on every request · scratch">
      <div className="flex flex-col gap-3">
        <span className="label">rulebooks</span>
        {data.rulebooks.length === 0 && <p className="font-mono text-[11.5px] text-muted">None of {s.rulebooks.join(", ") || "the configured rulebooks"} exist in the project folder.</p>}
        {data.rulebooks.map((r) => {
          const tone = r.bytes > r.cap ? "text-bad" : r.bytes >= r.warn ? "text-warn" : "text-ok";
          return (
            <div key={r.path} className="flex flex-col gap-1.5">
              <div className="flex flex-wrap justify-between gap-2 font-mono text-[11.5px]">
                <PathLink slug={data.slug} path={r.path} known={known} onOpen={onOpen} />
                <span className={tone} title={`${r.bytes.toLocaleString()} bytes`}>
                  {fmtKB(r.bytes)} of {fmtKB(r.cap)} · warn at {fmtKB(r.warn)}
                  {r.bytes > r.cap ? " · over" : ""}
                </span>
              </div>
              <BudgetBar value={r.bytes} warn={r.warn} cap={r.cap} />
            </div>
          );
        })}
      </div>
      <div className="flex flex-col gap-3 border-t border-line pt-3">
        <span className="label">scratch folders</span>
        {data.scratch.length === 0 && <p className="font-mono text-[11.5px] text-muted">No scratch folders ({s.scratch.join(", ") || "none configured"}).</p>}
        {data.scratch.map((x) => (
          <div key={x.path} className="flex flex-col gap-1.5">
            <div className="flex flex-wrap justify-between gap-2 font-mono text-[11.5px]">
              <span className="break-all text-text">{x.path}</span>
              <span className={x.bytes >= s.scratch_byte_warn ? "text-warn" : "text-text-2"}>
                {fmtBytes(x.bytes)} · warn at {fmtBytes(s.scratch_byte_warn)}
              </span>
            </div>
            <BudgetBar value={x.bytes} warn={s.scratch_byte_warn} cap={s.scratch_byte_warn * 1.5} />
            {x.copies.length > 0 && (
              <div className="flex flex-col gap-0.5 font-mono text-[11px] text-warn">
                <span>{plural(x.copies.length, "repo copy", "repo copies")} inside, which grep finds next to the real files:</span>
                {x.copies.map((c) => (
                  <span key={c} className="break-all text-text-2">
                    · {c}
                  </span>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
    </Box>
  );
}

// ---- search

export function SearchBox({ data, onOpen }: { data: KnowledgeOverview; onOpen: Open }) {
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<KnowledgeKind | null>(null);
  const [result, setResult] = useState<{ key: string; search: KnowledgeSearch | null; error: string } | null>(null);
  const q = query.trim();
  const key = JSON.stringify([data.slug, q, kind]);

  useEffect(() => {
    if (!q) return;
    let stale = false;
    const t = window.setTimeout(() => {
      api.searchKnowledge(data.slug, q, kind).then(
        (search) => !stale && setResult({ key, search, error: "" }),
        (e) => !stale && setResult({ key, search: null, error: errText(e) }),
      );
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [data.slug, q, kind, key]);

  const shown = q && result?.key === key ? result : null;
  const count = (k: KnowledgeKind) => data.counts.find((c) => c.kind === k)?.count ?? 0;

  return (
    <Box title="Search" glyph="⌕" note={shown?.search ? `${plural(shown.search.hits.length, "hit")} · ${shown.search.took_ms} ms` : "what an agent finds for the same words"}>
      <label className="flex h-9 items-center gap-2 rounded-lg border border-line-2 bg-inset px-3 focus-within:border-accent">
        <Search className="size-3.5 shrink-0 text-muted" />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="search the project's knowledge (Thai or English)"
          aria-label="Search knowledge"
          className="min-w-0 flex-1 bg-transparent font-mono text-[12.5px] text-text outline-none placeholder:text-muted/80"
        />
        {q && !shown && <span className="font-mono text-[10.5px] text-muted">searching…</span>}
      </label>
      <div className="flex flex-wrap gap-1.5">
        <Chip active={kind === null} onClick={() => setKind(null)}>
          all <span className="text-muted">{data.docs.length}</span>
        </Chip>
        {KINDS.map((k) => (
          <Chip key={k} active={kind === k} onClick={() => setKind(kind === k ? null : k)}>
            {KIND_LABEL[k]} <span className="text-muted">{count(k)}</span>
          </Chip>
        ))}
      </div>
      {shown?.error && <p className="text-[12.5px] text-bad">Search failed: {shown.error}</p>}
      {shown?.search && shown.search.hits.length === 0 && <p className="py-3 text-center text-[13px] text-muted">Nothing matches “{q}”{kind ? ` in ${KIND_LABEL[kind]}` : ""}.</p>}
      {shown?.search && shown.search.hits.length > 0 && (
        <ul className="flex flex-col gap-1.5">
          {shown.search.hits.map((h, i) => (
            <li key={`${h.path}-${i}`}>
              <button type="button" onClick={() => onOpen(h.path)} className="flex w-full flex-col gap-1 rounded-lg border border-line bg-inset px-3 py-2 text-left hover:border-line-3">
                <span className="flex flex-wrap items-center gap-2">
                  <Badge tone={KIND_TONE[h.kind]}>{KIND_LABEL[h.kind]}</Badge>
                  <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-text">{h.title || h.path}</span>
                  <span className="font-mono text-[10.5px] text-muted" title="score: higher is better">
                    {h.score.toFixed(2)}
                  </span>
                </span>
                <span className="font-mono text-[11px] break-all text-muted">{h.path}</span>
                <span className="text-[12.5px] leading-snug break-words text-text-2">
                  <Snippet text={h.snippet} />
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </Box>
  );
}

// ---- browse

type Sort = "name" | "lines" | "modified";

export function Browse({ data, now, onOpen }: { data: KnowledgeOverview; now: number; onOpen: Open }) {
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<Sort>("name");
  const [expanded, setExpanded] = useState<Set<KnowledgeKind>>(new Set());
  const s = data.settings;
  const f = filter.trim().toLowerCase();

  const groups = useMemo(() => {
    const cmp = (a: KnowledgeDoc, b: KnowledgeDoc) =>
      sort === "lines" ? b.lines - a.lines : sort === "modified" ? (b.modified ?? "").localeCompare(a.modified ?? "") : (a.title || a.path).localeCompare(b.title || b.path);
    const docs = data.docs.filter((d) => !f || d.path.toLowerCase().includes(f) || d.title.toLowerCase().includes(f));
    return KINDS.map((k) => [k, docs.filter((d) => d.kind === k).sort(cmp)] as const).filter(([, list]) => list.length > 0);
  }, [data.docs, f, sort]);

  return (
    <Box
      title="Browse"
      glyph="▤"
      note={plural(data.docs.length, "document")}
      actions={
        <div className="flex flex-wrap items-center gap-2">
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter by name or path"
            aria-label="Filter documents"
            className="h-7 w-[190px] max-w-full rounded-md border border-line-2 bg-inset px-2.5 font-mono text-[11.5px] text-text outline-none placeholder:text-muted/80 focus:border-accent"
          />
          <div className="flex overflow-hidden rounded-md border border-line-2" role="group" aria-label="Sort by">
            {(["name", "lines", "modified"] as const).map((v) => (
              <button
                key={v}
                type="button"
                aria-pressed={sort === v}
                onClick={() => setSort(v)}
                className={clsx("h-7 px-2.5 font-mono text-[11px]", sort === v ? "bg-accent text-accent-text" : "text-nav hover:text-text")}
              >
                {v}
              </button>
            ))}
          </div>
        </div>
      }
    >
      {groups.length === 0 && <p className="py-3 text-center text-[13px] text-muted">{data.docs.length ? "No documents match." : "Nothing indexed yet."}</p>}
      {groups.map(([kind, list]) => {
        const open = expanded.has(kind);
        return (
          <div key={kind} className="flex flex-col gap-1.5">
            <h3 className="flex items-center gap-2">
              <Badge tone={KIND_TONE[kind]}>{KIND_LABEL[kind]}</Badge>
              <span className="font-mono text-[11px] text-muted">{list.length}</span>
            </h3>
            <ul className="flex flex-col">
              {(open ? list : list.slice(0, DOC_LIMIT)).map((d) => {
                const lineTone = kind !== "card" ? "text-muted" : d.lines > s.card_line_cap ? "text-bad" : d.lines > s.card_line_warn ? "text-warn" : "text-muted";
                return (
                  <li key={d.path}>
                    <button type="button" onClick={() => onOpen(d.path)} className="flex w-full flex-wrap items-baseline gap-x-3 gap-y-0.5 rounded-md px-2 py-1.5 text-left hover:bg-white/[0.03]">
                      <span className="min-w-0 flex-[1_1_220px]">
                        <span className="block truncate text-[13px] text-text">{d.title || d.path.split("/").pop()}</span>
                        <span className="block truncate font-mono text-[10.5px] text-muted">{d.path}</span>
                      </span>
                      <span className="flex flex-wrap items-center gap-x-3 font-mono text-[10.5px]">
                        <span className={lineTone}>
                          {d.lines}
                          {kind === "card" ? ` / ${s.card_line_cap}` : ""} lines
                        </span>
                        {kind === "card" && <span className="text-muted">{plural(d.sources.length, "source")}</span>}
                        {kind === "card" && !d.in_index && <span className="text-warn">not in index</span>}
                        {d.modified && <span className="text-muted">{ago(new Date(d.modified).getTime(), now)}</span>}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
            {list.length > DOC_LIMIT && (
              <button
                type="button"
                onClick={() => setExpanded((x) => (x.has(kind) ? new Set([...x].filter((k) => k !== kind)) : new Set([...x, kind])))}
                className="self-start px-2 font-mono text-[11px] text-link hover:text-link-hover"
              >
                {open ? "Show fewer" : `Show all ${list.length}`}
              </button>
            )}
          </div>
        );
      })}
    </Box>
  );
}

// ---- agent searches

export function Queries({ data, now }: { data: KnowledgeOverview; now: number }) {
  const [by, setBy] = useState<"all" | "agent" | "owner">("all");
  const list = data.recent_queries.filter((q) => by === "all" || q.by === by);
  const agents = data.recent_queries.filter((q) => q.by === "agent").length;
  return (
    <Box title="Agent searches" glyph="◎" note={`${agents} by agents · newest first`}>
      <div className="flex flex-wrap gap-1.5">
        {(["all", "agent", "owner"] as const).map((v) => (
          <Chip key={v} active={by === v} onClick={() => setBy(v)}>
            {v === "all" ? "everyone" : v === "agent" ? "agents" : "you"}
          </Chip>
        ))}
      </div>
      {list.length === 0 ? (
        <p className="text-[12.5px] leading-snug text-muted">
          {data.recent_queries.length === 0
            ? "No searches yet. Agents search this project's knowledge through MAP with their project token; what they looked for, and how much they found, shows here."
            : "No searches by them yet."}
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {list.map((q, i) => (
            <li key={`${q.at}-${i}`} className="flex items-baseline gap-2.5 font-mono text-[11.5px]">
              <span className={clsx("w-11 shrink-0 text-[10px] font-semibold tracking-[0.1em] uppercase", q.by === "agent" ? "text-accent" : "text-muted")}>{q.by === "agent" ? "agent" : "you"}</span>
              <span className="min-w-0 flex-1 truncate text-text" title={q.query}>
                {q.query}
              </span>
              <span className={clsx("shrink-0", q.hits === 0 ? "text-warn" : "text-text-2")} title={q.hits === 0 ? "found nothing: a gap in the knowledge?" : undefined}>
                {plural(q.hits, "hit")}
              </span>
              <span className="w-16 shrink-0 text-right text-muted">{ago(new Date(q.at).getTime(), now)}</span>
            </li>
          ))}
        </ul>
      )}
    </Box>
  );
}
