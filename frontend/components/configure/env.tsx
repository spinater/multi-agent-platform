"use client";

/** Configure › Env & secrets (backend env_vars.rs): variables passed to every session, hook and verify command of a
 * project. Machine variables apply everywhere; a project variable with the same key overrides one. Secret values
 * are write-only: the server never sends them back, so the row shows "set" and a new value replaces it. */

import { useEffect, useState } from "react";

import { envApi } from "@/lib/api";
import type { ConfigSource } from "@/lib/generated/ConfigSource";
import type { EnvList } from "@/lib/generated/EnvList";

import { Note, SourceBadge, TabHead, inputCls, useConfigure } from "./ui";

type Row = { id: number; key: string; value: string; secret: boolean; set: boolean; source: ConfigSource; touched: boolean };
let seq = 0;

function rowsOf(l: EnvList): Row[] {
  return l.vars.map((v) => ({ id: ++seq, key: v.key, value: v.value ?? "", secret: v.secret, set: v.set, source: v.source, touched: false }));
}

export function EnvTab() {
  const { scope, slug, setPending } = useConfigure();
  const [list, setList] = useState<EnvList | null>(null);
  const [rows, setRows] = useState<Row[] | null>(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    envApi.list(slug).then(
      (l) => {
        if (!live) return;
        setList(l);
        setRows(rowsOf(l));
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [slug, tick]);

  const mine = (r: Row) => (scope === "machine" ? r.source === "machine" : r.source !== "machine");
  const base = list ? rowsOf(list) : [];
  const sig = (rs: Row[]) => JSON.stringify(rs.filter(mine).map((r) => [r.key, r.secret, r.touched ? r.value : null]));
  const changed = rows && list ? sig(rows) !== sig(base) : false;
  const count = rows ? Math.max(1, rows.filter((r) => r.touched).length + Math.abs(rows.filter(mine).length - base.filter(mine).length)) : 0;

  useEffect(() => {
    if (!rows || !changed) {
      setPending("env", null);
      return;
    }
    setPending("env", {
      count,
      files: [scope === "machine" ? "machine variables · secrets file" : "project variables · secrets file"],
      save: async () => {
        const vars = rows.filter(mine).map((r) => ({ key: r.key, secret: r.secret, value: r.secret && !r.touched ? null : r.value }));
        const bad = vars.find((v) => !/^[A-Z_][A-Z0-9_]*$/.test(v.key));
        if (bad) throw new Error(`${bad.key || "(empty)"}: keys are A–Z, 0–9 and _, not starting with a digit`);
        const l = await envApi.save(slug, vars);
        setList(l);
        setRows(rowsOf(l));
      },
      discard: () => setTick((t) => t + 1),
    });
  }, [rows && sig(rows), changed, slug]); // eslint-disable-line react-hooks/exhaustive-deps

  const upd = (id: number, p: Partial<Row>) =>
    setRows((rs) =>
      (rs ?? []).map((r) => {
        if (r.id !== id) return r;
        // Editing a machine variable in project scope makes this project's own copy.
        const source: ConfigSource = scope === "project" && r.source === "machine" ? "override" : r.source;
        return { ...r, ...p, source };
      }),
    );

  return (
    <>
      <TabHead
        title="Env & secrets"
        desc="Variables passed to every session and hook in this project."
        savesTo={
          list
            ? `${scope === "machine" ? "machine settings" : "project settings"} (values) · ${list.secrets_at} (secrets, 0600)`
            : scope === "machine"
              ? "machine settings · <data_dir>/secrets/machine.json"
              : "project settings · <data_dir>/secrets/projects/<slug>.json"
        }
        add={{
          label: "+ Add variable",
          onClick: () => setRows([...(rows ?? []), { id: ++seq, key: "NEW_VAR", value: "", secret: false, set: false, source: scope === "machine" ? "machine" : "project", touched: true }]),
          disabled: !rows,
        }}
      />
      {error && <Note tone="bad">Could not read the variables: {error}</Note>}
      {!rows && !error && <Note>Reading the variables…</Note>}
      {rows && rows.length === 0 && <Note>No variables yet. + Add variable; mark it secret to keep its value out of the dashboard, the database and the logs.</Note>}
      <div className="flex flex-col gap-1.5">
        {(rows ?? []).map((r) => {
          const masked = r.secret && !r.touched && r.set;
          return (
            <div key={r.id} className="flex flex-wrap items-center gap-x-2.5 gap-y-2 rounded-[9px] border border-line bg-surface px-3 py-[9px]">
              <input
                value={r.key}
                onChange={(e) => upd(r.id, { key: e.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, "_"), touched: true })}
                className={`${inputCls} h-[30px] min-w-[140px] flex-[0_1_230px] font-semibold`}
                aria-label="Key"
              />
              <span className="text-muted/80">=</span>
              <input
                value={masked ? "" : r.value}
                placeholder={masked ? "•••••••••••• set · type to replace" : r.secret ? "secret value" : ""}
                type={r.secret ? "password" : "text"}
                autoComplete="off"
                onChange={(e) => upd(r.id, { value: e.target.value, touched: true })}
                className={`${inputCls} h-[30px] flex-[1_1_220px]`}
                aria-label={`Value of ${r.key}`}
              />
              <button
                onClick={() => upd(r.id, { secret: !r.secret, touched: r.secret ? true : r.touched, value: r.secret ? "" : r.value })}
                className="h-[26px] cursor-pointer rounded-[5px] border px-2 font-mono text-[10.5px] font-medium"
                style={{ borderColor: r.secret ? "color-mix(in srgb, var(--warn) 40%, transparent)" : "var(--line-3)", color: r.secret ? "var(--warn)" : "var(--text-2)" }}
                title={r.secret ? "Make it a plain variable (needs a new value)" : "Keep the value secret"}
              >
                {r.secret ? "secret" : "plain"}
              </button>
              <SourceBadge source={r.source} />
              {mine(r) ? (
                <button onClick={() => setRows((rs) => (rs ?? []).filter((x) => x.id !== r.id))} className="size-[26px] cursor-pointer rounded-[5px] text-muted hover:text-bad" aria-label={`Remove ${r.key}`}>
                  ×
                </button>
              ) : (
                <span className="w-[26px]" />
              )}
            </div>
          );
        })}
      </div>
      <span className="mt-1 font-mono text-[10.5px] leading-normal text-muted">
        secrets live in a file only the server can read (0600) and reach sessions through a one-time file the launch script reads and deletes; they never go into map.toml, the database or logs, and
        this page never shows them again
      </span>
    </>
  );
}
