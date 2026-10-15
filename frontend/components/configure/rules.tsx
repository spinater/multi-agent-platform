"use client";

/** Configure › Rules (backend rules.rs): the context files each agent reads at the start of every session. A file
 * list beside an editor with "read by" chips: Claude and agy read a file through an `@path` import line in their
 * root file (CLAUDE.md, GEMINI.md), which the chips add or remove; a CLI's own root file is fixed. Body edits,
 * reader changes, new files and deletes are drafts until Save. */

import { useEffect, useState } from "react";

import { rulesApi, type ConfigOp } from "@/lib/api";
import type { RulesOverview } from "@/lib/generated/RulesOverview";

import { AskAgent } from "./ask-agent";
import { Chip, Note, SourceBadge, TabHead, dangerCls, useConfigure } from "./ui";

/** A file's unsaved state: its new body, its readers, created here, or deleted. */
type Draft = { body?: string; readers?: string[]; created?: boolean; deleted?: boolean };

export function RulesTab() {
  const { scope, slug, setPending, flash } = useConfigure();
  const [data, setData] = useState<RulesOverview | null>(null);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [sel, setSel] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    rulesApi.list(slug).then(
      (d) => {
        if (!live) return;
        setData(d);
        setDrafts({});
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [slug, tick]);

  const files = [
    ...(data?.files ?? []).filter((f) => scope === "project" || f.source === "machine"),
    ...Object.entries(drafts)
      .filter(([p, d]) => d.created && !data?.files.some((f) => f.path === p))
      .map(([path]) => ({ path, abs_path: path, source: "project" as const, exists: false, lines: 0, bytes: 0, readers: [], reader_options: [], root_of: [], body: "", truncated: false })),
  ].filter((f) => !drafts[f.path]?.deleted);
  const cur = files.find((f) => f.path === sel) ?? files[0] ?? null;
  const editable = (f: (typeof files)[number]) => scope === "machine" || f.source !== "machine";

  const changed = Object.entries(drafts).filter(([path, d]) => {
    const f = data?.files.find((x) => x.path === path);
    return d.deleted || d.created || (d.body !== undefined && d.body !== f?.body) || (d.readers !== undefined && JSON.stringify([...d.readers].sort()) !== JSON.stringify([...(f?.readers ?? [])].sort()));
  });
  useEffect(() => {
    setPending(
      "rules",
      changed.length
        ? {
            count: changed.length,
            files: changed.map(([p]) => p),
            save: async () => {
              for (const [path, d] of changed) {
                if (d.deleted) {
                  if (data?.files.some((f) => f.path === path && f.exists)) await rulesApi.remove(slug, path);
                  continue;
                }
                if (d.body !== undefined || d.created) await rulesApi.write(slug, path, d.body ?? "");
                if (d.readers) await rulesApi.readers(slug, path, d.readers);
              }
              setTick((t) => t + 1);
            },
            discard: () => setDrafts({}),
          }
        : null,
    );
  }, [JSON.stringify(changed), slug]); // eslint-disable-line react-hooks/exhaustive-deps

  const patch = (path: string, p: Draft) => setDrafts((d) => ({ ...d, [path]: { ...d[path], ...p } }));
  const add = () => {
    const name = window.prompt("New rule file, relative to the project folder", "docs/rules/new.md");
    if (!name) return;
    if (!/^[\w./-]+\.md$/.test(name) || name.includes("..") || name.startsWith("/")) {
      flash("A path inside the project, ending in .md", "bad");
      return;
    }
    patch(name, { created: true, body: "# Rules\n\n- \n" });
    setSel(name);
  };
  const applyOps = (ops: ConfigOp[]) => {
    let n = 0;
    const bodies: Record<string, string> = {};
    for (const o of ops) {
      const path = o.target.replace(/^rule:/, "");
      const v = (o.value ?? {}) as { append?: string; find?: string; replace?: string };
      const f = files.find((x) => x.path === path);
      const body = bodies[path] ?? drafts[path]?.body ?? f?.body ?? "";
      let next: string | null = null;
      if (o.op === "add" && v.append) next = `${body.replace(/\n*$/, "\n")}${v.append.replace(/\n*$/, "")}\n`;
      else if (o.op === "change" && v.find && body.includes(v.find)) next = body.replace(v.find, v.replace ?? "");
      else if (o.op === "remove" && v.find && body.includes(v.find)) next = body.replace(v.find, "");
      if (next === null) continue;
      bodies[path] = next;
      patch(path, { body: next, ...(f ? {} : { created: true }) });
      n++;
    }
    return n;
  };

  const body = cur ? (drafts[cur.path]?.body ?? cur.body) : "";
  const readers = cur ? (drafts[cur.path]?.readers ?? cur.readers) : [];
  const imports = [...body.matchAll(/^@(\S+)\s*$/gm)].map((m) => m[1]);

  return (
    <>
      <TabHead
        title="Rules"
        desc="Context files each agent reads at the start of every session. CLAUDE.md and GEMINI.md import AGENTS.md so all agents follow the same rules."
        savesTo={data ? `the files themselves · ${scope === "machine" ? "the CLIs' home folders" : data.folder}` : "the files themselves"}
        add={scope === "project" ? { label: "+ New rule file", onClick: add } : undefined}
      />
      <AskAgent tab="rules" context={{ files: files.map((f) => ({ path: f.path, readers: f.readers, body: (drafts[f.path]?.body ?? f.body).slice(0, 6000) })) }} onApply={applyOps} />
      {error && <Note tone="bad">Could not read the rule files: {error}</Note>}
      {!data && !error && <Note>Reading the rule files…</Note>}
      {data && (
        <div className="flex flex-wrap items-start gap-3.5">
          <div className="flex max-w-[320px] flex-[1_1_240px] flex-col gap-1.5">
            {files.map((f) => {
              const on = cur?.path === f.path;
              const d = drafts[f.path];
              const b = d?.body ?? f.body;
              return (
                <button
                  key={f.path}
                  onClick={() => setSel(f.path)}
                  className="flex cursor-pointer flex-col gap-1.5 rounded-[9px] border px-3 py-[11px] text-left text-text"
                  style={{ borderColor: on ? "var(--accent)" : "var(--line)", background: on ? "color-mix(in srgb, var(--accent) 6%, transparent)" : "var(--surface)" }}
                >
                  <span className="flex w-full items-center gap-2">
                    <span className="truncate font-mono text-[12.5px] leading-none font-semibold">{f.path}</span>
                    {d && <span className="size-1.5 flex-none rounded-full bg-accent" title="unsaved" />}
                    <span className="ml-auto">
                      <SourceBadge source={f.source} />
                    </span>
                  </span>
                  <span className="font-mono text-[10.5px] leading-[1.3] text-muted">
                    {f.exists || d ? `${b.split("\n").filter(Boolean).length} lines` : "not there yet"} · read by {(d?.readers ?? f.readers).join(", ") || "no CLI"}
                  </span>
                </button>
              );
            })}
          </div>
          {cur && (
            <div className="flex min-w-0 flex-[999_1_460px] flex-col gap-3 rounded-xl border border-line bg-surface p-4">
              <div className="flex flex-wrap items-center gap-2.5">
                <span className="font-mono text-[15px] leading-none font-semibold [overflow-wrap:anywhere]">{cur.path}</span>
                <span className="ml-auto font-mono text-[11px] leading-none text-muted">{body.split("\n").length} lines</span>
                {editable(cur) && (
                  <button onClick={() => patch(cur.path, { deleted: true })} className={dangerCls}>
                    Delete
                  </button>
                )}
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="mr-1 font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase">read by</span>
                {cur.reader_options.length
                  ? cur.reader_options.map((r) => {
                      const on = readers.includes(r.provider);
                      return (
                        <Chip
                          key={r.provider}
                          on={on}
                          disabled={r.fixed || !editable(cur)}
                          title={[r.how, r.note].filter(Boolean).join(" · ")}
                          onClick={() => patch(cur.path, { readers: on ? readers.filter((x) => x !== r.provider) : [...readers, r.provider] })}
                        >
                          {r.provider}
                        </Chip>
                      );
                    })
                  : <span className="font-mono text-[11px] text-muted">saved first, then readers can be chosen</span>}
              </div>
              <textarea
                value={body}
                readOnly={!editable(cur)}
                onChange={(e) => patch(cur.path, { body: e.target.value })}
                rows={16}
                className="resize-y rounded-lg border border-line bg-inset p-3 font-mono text-[12.5px] leading-[1.6] text-text outline-none focus:border-accent"
                spellCheck={false}
              />
              <span className="font-mono text-[10.5px] leading-[1.4] text-muted">
                {cur.truncated
                  ? `shown up to ${Math.round((data.max_bytes ?? 0) / 1024)} KB; edit the file itself`
                  : imports.length && body.trim().split("\n").length <= imports.length + 1
                    ? `imports ${imports.join(", ")} · edit that file to change these rules`
                    : cur.root_of.length
                      ? `root file of ${cur.root_of.join(", ")}`
                      : cur.source === "machine"
                        ? "machine file: applies in every project on this machine"
                        : "committed with the repo"}
              </span>
            </div>
          )}
        </div>
      )}
    </>
  );
}
