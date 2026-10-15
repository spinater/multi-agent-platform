"use client";

/** Configure › MCP servers (backend mcp.rs): one list across the CLIs' own files (Claude `.mcp.json` /
 * `.claude.json`, Codex `config.toml`, agy `settings.json`). Each card: transport, command or URL, last test status,
 * which CLIs have it, Test connection, Edit, Remove. Saving a server writes it for the chosen CLIs and removes it
 * from the others in this scope; env and header values are write-only. Saved as you go (one server at a time). */

import { useEffect, useState } from "react";

import { mcpApi } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { McpServer } from "@/lib/generated/McpServer";
import type { McpTransport } from "@/lib/generated/McpTransport";
import type { McpView } from "@/lib/generated/McpView";

import { Box, Chip, Lbl, Note, Seg, SourceBadge, TabHead, btnCls, dangerCls, inputCls, primaryCls, useConfigure } from "./ui";

export type McpServerRow = { name: string; providers: string[]; ok: boolean };

/** The MCP servers in scope, for the Agents tab's relation map. */
export function useMcpServers(slug: string | null): { servers: McpServerRow[] | null } {
  const [servers, setServers] = useState<McpServerRow[] | null>(null);
  useEffect(() => {
    let live = true;
    mcpApi.list(slug).then(
      (v) => live && setServers(v.servers.map((s) => ({ name: s.name, providers: s.providers, ok: s.status.state !== "failed" }))),
      () => live && setServers([]),
    );
    return () => {
      live = false;
    };
  }, [slug]);
  return { servers };
}

type Form = { name: string; transport: McpTransport; command: string; args: string; url: string; env: { key: string; value: string; set: boolean }[]; providers: string[]; editing: string | null };

const blank = (providers: string[]): Form => ({ name: "", transport: "stdio", command: "npx", args: "-y <package>", url: "", env: [], providers, editing: null });

/** Splits an args line on spaces, keeping "quoted parts" together. */
function splitArgs(s: string): string[] {
  return [...s.matchAll(/"([^"]*)"|'([^']*)'|(\S+)/g)].map((m) => m[1] ?? m[2] ?? m[3]);
}

export function McpTab() {
  const { scope, slug, flash } = useConfigure();
  const [view, setView] = useState<McpView | null>(null);
  const [form, setForm] = useState<Form | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    mcpApi.list(slug).then(
      (v) => {
        if (!live) return;
        setView(v);
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [slug]);

  const providers = (view?.providers ?? []).map((p) => p.name);
  const servers = (view?.servers ?? []).filter((s) => scope === "project" || s.source === "machine");

  const edit = (s: McpServer) =>
    setForm({
      name: s.name,
      transport: s.transport,
      command: s.command ?? "",
      args: s.args.map((a) => (/\s/.test(a) ? `"${a}"` : a)).join(" "),
      url: s.url ?? "",
      env: s.env_keys.map((k) => ({ key: k.key, value: "", set: k.set })),
      providers: s.providers,
      editing: s.name,
    });

  const save = async () => {
    if (!form) return;
    if (!/^[A-Za-z0-9_.-]+$/.test(form.name)) {
      flash("Name: letters, digits, . _ and - only", "bad");
      return;
    }
    if (!form.providers.length) {
      flash("Pick at least one CLI", "bad");
      return;
    }
    setBusy("save");
    try {
      const v = await mcpApi.save(
        slug,
        {
          name: form.name,
          transport: form.transport,
          command: form.transport === "stdio" ? form.command.trim() || null : null,
          args: form.transport === "stdio" ? splitArgs(form.args) : [],
          url: form.transport === "http" ? form.url.trim() || null : null,
          env: form.env.filter((e) => e.key.trim()).map((e) => ({ key: e.key.trim(), value: e.value === "" && e.set ? null : e.value })),
          headers: [],
        },
        form.providers,
      );
      setView(v);
      setForm(null);
      flash(`Saved ${form.name} for ${form.providers.join(", ")}`);
    } catch (e) {
      flash((e as Error).message, "bad");
    } finally {
      setBusy(null);
    }
  };
  const remove = async (s: McpServer) => {
    if (!window.confirm(`Remove ${s.name} from ${s.providers.join(", ")} in this ${scope === "machine" ? "machine" : "project"}?`)) return;
    setBusy(s.name);
    try {
      setView(await mcpApi.remove(slug, s.name));
      flash(`Removed ${s.name}`, "warn");
    } catch (e) {
      flash((e as Error).message, "bad");
    } finally {
      setBusy(null);
    }
  };
  const test = async (s: McpServer) => {
    setBusy(`test:${s.name}`);
    try {
      const r = await mcpApi.test(slug, s.name);
      flash(r.ok ? `${s.name}: connected · ${r.tools} tools` : `${s.name}: ${r.error ?? "failed"}`, r.ok ? "ok" : "bad");
      setView(await mcpApi.list(slug));
    } catch (e) {
      flash((e as Error).message, "bad");
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <TabHead
        title="MCP servers"
        desc="Tool servers agents can call. The CLIs that have a server can use it; agents get the servers of their target CLIs."
        savesTo={scope === "machine" ? "each CLI's home: .claude.json, codex config.toml, agy settings.json · saved per server" : ".mcp.json · .codex/config.toml · .gemini/settings.json · saved per server"}
        add={{ label: "+ Add server", onClick: () => setForm(blank(providers.filter((p) => p !== "openrouter"))), disabled: !view }}
      />
      {error && <Note tone="bad">Could not read the MCP servers: {error}</Note>}
      {!view && !error && <Note>Reading each CLI&apos;s MCP settings…</Note>}
      {form && (
        <Box className="border-accent/40">
          <span className="text-[14px] leading-none font-semibold">{form.editing ? `Edit ${form.editing}` : "New MCP server"}</span>
          <div className="flex flex-wrap gap-3">
            <label className="flex flex-col gap-1.5">
              <Lbl>name</Lbl>
              <input value={form.name} readOnly={!!form.editing} onChange={(e) => setForm({ ...form, name: e.target.value })} className={inputCls} placeholder="github" />
            </label>
            <div className="flex flex-col gap-1.5">
              <Lbl>transport</Lbl>
              <Seg value={form.transport} options={["stdio", "http"]} onChange={(t) => setForm({ ...form, transport: t })} size="md" />
            </div>
          </div>
          {form.transport === "stdio" ? (
            <div className="flex flex-wrap gap-2">
              <label className="flex flex-[0_1_180px] flex-col gap-1.5">
                <Lbl>command</Lbl>
                <input value={form.command} onChange={(e) => setForm({ ...form, command: e.target.value })} className={inputCls} />
              </label>
              <label className="flex flex-[1_1_300px] flex-col gap-1.5">
                <Lbl>args</Lbl>
                <input value={form.args} onChange={(e) => setForm({ ...form, args: e.target.value })} className={inputCls} />
              </label>
            </div>
          ) : (
            <label className="flex flex-col gap-1.5">
              <Lbl>url</Lbl>
              <input value={form.url} onChange={(e) => setForm({ ...form, url: e.target.value })} className={inputCls} placeholder="https://mcp.example.com/mcp" />
            </label>
          )}
          <div className="flex flex-col gap-1.5">
            <Lbl>env · values are write-only</Lbl>
            {form.env.map((e, i) => (
              <div key={i} className="flex flex-wrap gap-2">
                <input value={e.key} onChange={(x) => setForm({ ...form, env: form.env.map((y, j) => (j === i ? { ...y, key: x.target.value.toUpperCase().replace(/[^A-Z0-9_]/g, "_") } : y)) })} className={`${inputCls} w-[220px]`} />
                <input
                  type="password"
                  autoComplete="off"
                  value={e.value}
                  placeholder={e.set ? "•••••• set · type to replace" : "value"}
                  onChange={(x) => setForm({ ...form, env: form.env.map((y, j) => (j === i ? { ...y, value: x.target.value } : y)) })}
                  className={`${inputCls} flex-1`}
                />
                <button onClick={() => setForm({ ...form, env: form.env.filter((_, j) => j !== i) })} className="size-8 cursor-pointer text-muted hover:text-bad" aria-label="Remove">
                  ×
                </button>
              </div>
            ))}
            <button onClick={() => setForm({ ...form, env: [...form.env, { key: "", value: "", set: false }] })} className="h-7 cursor-pointer self-start rounded-md border border-dashed border-line-3 px-2.5 text-[11.5px] font-medium text-nav">
              + Add variable
            </button>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Lbl className="mr-1">for</Lbl>
            {providers.map((p) => (
              <Chip key={p} on={form.providers.includes(p)} onClick={() => setForm({ ...form, providers: form.providers.includes(p) ? form.providers.filter((x) => x !== p) : [...form.providers, p] })}>
                {p}
              </Chip>
            ))}
          </div>
          <div className="flex gap-2">
            <button onClick={save} disabled={busy === "save"} className={primaryCls}>
              {busy === "save" ? "Saving…" : "Save server"}
            </button>
            <button onClick={() => setForm(null)} className={btnCls}>
              Cancel
            </button>
          </div>
        </Box>
      )}
      {view && servers.length === 0 && !form && <Note>No MCP servers in {scope === "machine" ? "the CLIs' home folders" : "this project or the machine"}. + Add server writes one for the CLIs you pick.</Note>}
      <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(320px,100%),1fr))]">
        {servers.map((s) => {
          const st = s.status.state;
          const dot = st === "connected" ? "#4ade80" : st === "failed" ? "#F26D6D" : "#5d6673";
          const editable = scope === "machine" || s.source !== "machine";
          return (
            <div key={s.name} className="flex flex-col gap-2.5 rounded-[11px] border border-line bg-surface p-3.5">
              <div className="flex items-center gap-2">
                <span className="size-2 rounded-full" style={{ background: dot }} />
                <span className="font-mono text-[14px] leading-none font-semibold">{s.name}</span>
                <span className="font-mono text-[10.5px] leading-none text-muted">{s.transport}</span>
                <span className="ml-auto">
                  <SourceBadge source={s.source} />
                </span>
              </div>
              <code className="rounded-[7px] border border-line bg-inset px-2.5 py-2 font-mono text-[11.5px] leading-[1.4] [overflow-wrap:anywhere] text-nav">
                {s.transport === "http" ? s.url : [s.command, ...s.args].filter(Boolean).join(" ")}
              </code>
              <span className="font-mono text-[11px] leading-[1.4]" style={{ color: st === "connected" ? "#7ee2a8" : st === "failed" ? "var(--bad)" : "var(--muted)" }}>
                {s.status.label}
              </span>
              <span className="flex flex-wrap items-center gap-1.5 font-mono text-[10.5px] leading-[1.3] text-muted">
                in
                {s.providers.map((p) => (
                  <span key={p} className="flex items-center gap-1">
                    <span className="size-1.5 rounded-full" style={{ background: providerColor(p) }} />
                    {p}
                  </span>
                ))}
                {s.env_keys.length > 0 && <span>· env {s.env_keys.map((k) => `${k.key}${k.set ? "" : " (empty)"}`).join(", ")}</span>}
              </span>
              {s.notes.map((n) => (
                <span key={n} className="font-mono text-[10.5px] leading-[1.35] text-muted">
                  {n}
                </span>
              ))}
              <div className="flex gap-1.5">
                <button onClick={() => test(s)} disabled={busy === `test:${s.name}`} className={`${btnCls} h-7 text-[11.5px]`}>
                  {busy === `test:${s.name}` ? "Testing…" : "Test connection"}
                </button>
                {editable && (
                  <>
                    <button onClick={() => edit(s)} className={`${btnCls} h-7 text-[11.5px]`}>
                      Edit
                    </button>
                    <button onClick={() => remove(s)} disabled={busy === s.name} className={`${dangerCls} h-7 text-[11.5px]`}>
                      Remove
                    </button>
                  </>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {view && (
        <div className="flex flex-col gap-1 font-mono text-[10.5px] leading-[1.45] text-muted">
          {view.files.filter((f) => f.scope === (slug ? "project" : "machine")).map((f) => (
            <span key={`${f.provider}:${f.path}`} style={{ color: f.error ? "var(--bad)" : undefined }}>
              {f.provider} · {f.path}
              {f.exists ? "" : " · not there yet"}
              {f.error ? ` · ${f.error}` : ""}
            </span>
          ))}
        </div>
      )}
    </>
  );
}
