"use client";

/** Configure › Permissions (backend permissions.rs): tool rules × the CLIs MAP runs, written into each CLI's own
 * settings (Claude `permissions.allow/ask/deny`, agy's tool lists). Click a cell to cycle allow → ask → deny; a deny
 * anywhere wins, so a project can only make a machine rule stricter (yellow dot = this project's value). Codex has no
 * per-tool rules: its column only reports its approval policy and sandbox. Drafts until Save. */

import { useEffect, useState } from "react";

import { permsApi, type ConfigOp } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { PermChange } from "@/lib/generated/PermChange";
import type { PermValue } from "@/lib/generated/PermValue";
import type { PermissionsView } from "@/lib/generated/PermissionsView";

import { AskAgent } from "./ask-agent";
import { Note, TabHead, useConfigure } from "./ui";

const PV: Record<PermValue, [string, string, string]> = {
  allow: ["#7ee2a8", "rgba(74,222,128,.08)", "rgba(74,222,128,.3)"],
  ask: ["#F2C14E", "rgba(242,193,78,.08)", "rgba(242,193,78,.3)"],
  deny: ["#F26D6D", "rgba(242,109,109,.08)", "rgba(242,109,109,.3)"],
  unsupported: ["var(--muted)", "transparent", "var(--line)"],
};
const ORDER: PermValue[] = ["allow", "ask", "deny"];
const k = (rule: string, provider: string) => `${rule}\u0000${provider}`;

export function PermsTab() {
  const { slug, setPending } = useConfigure();
  const [data, setData] = useState<PermissionsView | null>(null);
  const [changes, setChanges] = useState<Record<string, PermValue>>({});
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    permsApi.get(slug).then(
      (d) => {
        if (!live) return;
        setData(d);
        setChanges({});
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [slug]);

  const list: PermChange[] = Object.entries(changes)
    .map(([key, value]) => {
      const [rule, provider] = key.split("\u0000");
      return { rule, provider, value };
    })
    .filter((c) => data?.rows.find((r) => r.rule === c.rule)?.cells.find((x) => x.provider === c.provider)?.value !== c.value);
  useEffect(() => {
    setPending(
      "perms",
      list.length
        ? {
            count: list.length,
            files: [...new Set((data?.files ?? []).filter((f) => f.writable && f.scope === (slug ? "project" : "machine") && list.some((c) => c.provider === f.provider)).map((f) => f.path))],
            save: async () => {
              const d = await permsApi.set(slug, list);
              setData(d);
              setChanges({});
            },
            discard: () => setChanges({}),
          }
        : null,
    );
  }, [JSON.stringify(list), slug]); // eslint-disable-line react-hooks/exhaustive-deps

  const applyOps = (ops: ConfigOp[]) => {
    let n = 0;
    const next = { ...changes };
    for (const o of ops) {
      const v = o.value as { rule?: string; provider?: string; value?: PermValue } | null;
      const m = /^perm:(.+):([^:]+)$/.exec(o.target);
      const rule = v?.rule ?? m?.[1];
      const provider = v?.provider ?? m?.[2];
      const value = v?.value ?? (typeof o.value === "string" ? (o.value as PermValue) : undefined);
      if (!rule || !provider || !value || !ORDER.includes(value)) continue;
      const cell = data?.rows.find((r) => r.rule === rule)?.cells.find((c) => c.provider === provider);
      if (!cell || cell.value === "unsupported") continue;
      next[k(rule, provider)] = value;
      n++;
    }
    setChanges(next);
    return n;
  };

  const cols = data?.providers ?? [];
  const grid = `minmax(180px,1.4fr) repeat(${Math.max(1, cols.length)},minmax(96px,1fr))`;
  return (
    <>
      <TabHead
        title="Permissions"
        desc="What each CLI may do without asking. Ask pauses the session until someone answers in the command centre."
        savesTo="each CLI’s settings (.claude/settings.local.json, .gemini/settings.json; machine: their home folders)"
      />
      <AskAgent tab="perms" context={data && { providers: cols.map((c) => c.name), rows: data.rows.map((r) => ({ rule: r.rule, cells: r.cells.map((c) => ({ provider: c.provider, value: changes[k(r.rule, c.provider)] ?? c.value })) })) }} onApply={applyOps} />
      {error && <Note tone="bad">Could not read the permissions: {error}</Note>}
      {!data && !error && <Note>Reading each CLI&apos;s settings…</Note>}
      {data && (
        <>
          <div className="overflow-auto rounded-[10px] border border-line">
            <div className="min-w-[720px]">
              <div className="grid border-b border-line" style={{ gridTemplateColumns: grid }}>
                <span className="px-3 py-[11px] font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase">tool</span>
                {cols.map((c) => (
                  <span key={c.name} className="flex flex-col gap-1 px-2 py-[11px]" title={c.note ?? undefined}>
                    <span className="flex items-center gap-1.5 font-mono text-[11.5px] leading-none font-semibold">
                      <span className="size-1.5 rounded-full" style={{ background: providerColor(c.name) }} />
                      {c.name}
                    </span>
                    {c.note && <span className="line-clamp-2 font-mono text-[9.5px] leading-[1.3] text-muted">{c.note}</span>}
                  </span>
                ))}
              </div>
              {data.rows.map((r) => (
                <div key={r.rule} className="grid border-b border-line/60 last:border-b-0" style={{ gridTemplateColumns: grid }}>
                  <span className="flex flex-col gap-1 px-3 py-2.5">
                    <span className="font-mono text-[12px] leading-none font-medium">{r.rule}</span>
                    <span className="text-[10.5px] leading-[1.2] text-muted">{r.description}</span>
                  </span>
                  {r.cells.map((c) => {
                    const v = changes[k(r.rule, c.provider)] ?? c.value;
                    const [color, bg, border] = PV[v];
                    const ov = c.source === "override" || c.source === "project" || k(r.rule, c.provider) in changes;
                    return (
                      <span key={c.provider} className="flex items-center p-2">
                        <button
                          disabled={v === "unsupported"}
                          onClick={() => setChanges({ ...changes, [k(r.rule, c.provider)]: ORDER[(ORDER.indexOf(v) + 1) % 3] })}
                          title={[c.note, v === "unsupported" ? null : "click to change", c.source ? `from ${c.source}` : "CLI default"].filter(Boolean).join(" · ")}
                          className="relative h-[26px] min-w-16 cursor-pointer rounded-[5px] border px-2 font-mono text-[10.5px] leading-none font-semibold tracking-[0.06em] uppercase disabled:cursor-not-allowed"
                          style={{ color, background: bg, borderColor: border, opacity: c.source === null && !(k(r.rule, c.provider) in changes) && v !== "unsupported" ? 0.7 : 1 }}
                        >
                          {v === "unsupported" ? "n/a" : v}
                          {ov && v !== "unsupported" && <span className="absolute -top-[3px] -right-[3px] size-[7px] rounded-full bg-warn" />}
                        </button>
                      </span>
                    );
                  })}
                </div>
              ))}
            </div>
          </div>
          <span className="font-mono text-[10.5px] leading-normal text-muted">
            click a cell to cycle allow → ask → deny · yellow dot = this project&apos;s value · faded = the CLI&apos;s default · a deny anywhere wins, so a project can only tighten a machine rule
          </span>
          <div className="flex flex-col gap-1 font-mono text-[10.5px] leading-[1.45] text-muted">
            {data.files.filter((f) => f.scope === (slug ? "project" : "machine")).map((f) => (
              <span key={`${f.provider}:${f.path}`} style={{ color: f.error ? "var(--bad)" : undefined }}>
                {f.provider} · {f.path}
                {f.exists ? "" : " · not there yet"}
                {f.writable ? "" : " · read only"}
                {f.error ? ` · ${f.error}` : ""}
              </span>
            ))}
          </div>
        </>
      )}
    </>
  );
}
