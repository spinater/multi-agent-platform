"use client";

/** Settings → Reflex gates (backend gates.rs): the orchestrator gates' own Reflex, shared by every project. Its
 * corpus has one domain per answer; its threshold decides from which confidence an answer is acted on, and with
 * none it runs in shadow. */

import clsx from "clsx";
import { RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { api } from "@/lib/api";
import type { GatesInfo } from "@/lib/generated/GatesInfo";
import type { ReflexState } from "@/lib/generated/ReflexState";

import { useApp } from "../app-context";
import { Btn, TextInput, errText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";

/** Re-read the status this often (a restart or a project turning a gate on happens elsewhere). */
const REFRESH_MS = 20_000;

/** What each domain of the gates corpus answers. */
const DOMAINS: { id: string; question: string; text: string }[] = [
  { id: "unclear", question: "intake", text: "too unclear to start: asks you first" },
  { id: "plan_first", question: "intake", text: "big enough to plan first: waits for you" },
  { id: "light", question: "intake · tier", text: "a small task: the light model" },
  { id: "heavy", question: "intake · tier", text: "a big task: the heavy model" },
  { id: "shared", question: "shared files", text: "changes files other tasks share: runs alone" },
  { id: "own", question: "shared files", text: "keeps to its own files" },
  { id: "audit", question: "audit", text: "the change needs the LLM audit" },
  { id: "no_audit", question: "audit", text: "docs, copy, tests only: no audit call" },
];

const STATE_TONE: Record<ReflexState, string> = { running: "text-ok", starting: "text-info", stopped: "text-muted", no_corpus: "text-warn", error: "text-bad" };

/** Reflex confidences are small numbers (1e-3 … 5e-2), always shown in scientific notation. */
const fmt = (c: number | null) => (c === null ? "none" : c === 0 ? "0" : c.toExponential(2));

/** Parses "2e-3", "0.002", "2.5E-2"; `undefined` for anything else. */
function parseThreshold(raw: string): number | undefined {
  const v = raw.trim();
  if (!/^[0-9]*\.?[0-9]+(e[-+]?[0-9]+)?$/i.test(v)) return undefined;
  const n = Number(v);
  return Number.isFinite(n) && n >= 0 ? n : undefined;
}

export function ReflexGatesSettings() {
  const { projects, go, setProject, setScope, toast } = useApp();
  const [info, setInfo] = useState<GatesInfo | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState("");
  const [draft, setDraft] = useState<string | null>(null);
  const [file, setFile] = useState<{ path: string; content: string; saved: string } | null>(null);
  const [fileError, setFileError] = useState("");

  const load = useCallback(() => {
    api.gates().then(
      (g) => {
        setInfo(g);
        setError("");
      },
      (e) => setError(errText(e)),
    );
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const run = async (label: string, f: () => Promise<GatesInfo>, ok: (g: GatesInfo) => string) => {
    setBusy(label);
    try {
      const g = await f();
      setInfo(g);
      toast(ok(g));
      return true;
    } catch (e) {
      toast(errText(e), "bad");
      return false;
    } finally {
      setBusy("");
    }
  };

  const text = draft ?? (info?.threshold == null ? "" : fmt(info.threshold));
  const parsed = parseThreshold(text);
  const saveThreshold = () => {
    if (parsed === undefined) return;
    void run("threshold", () => api.setGatesThreshold(parsed), (g) => `Gates threshold ${fmt(g.threshold)} · answers at or above it are acted on`).then((ok) => ok && setDraft(null));
  };
  const reset = () =>
    void run(
      "threshold",
      () => api.setGatesThreshold(null),
      (g) => (g.threshold === null ? "Gates back to shadow (map.toml has no gates_threshold)" : `Gates threshold back to map.toml's ${fmt(g.threshold)}`),
    ).then((ok) => ok && setDraft(null));
  const restart = () => void run("restart", () => api.restartGates(), (g) => `Gates Reflex: ${g.status.state.replace("_", " ")}`);

  const open = async (path: string) => {
    if (file && file.content !== file.saved && !confirm(`Discard your changes to ${file.path}?`)) return;
    setFileError("");
    try {
      const f = await api.gatesFile(path);
      setFile({ path: f.path, content: f.content, saved: f.content });
    } catch (e) {
      setFileError(errText(e));
    }
  };
  const saveFile = () => {
    if (!file) return;
    const { path, content } = file;
    void run("file", () => api.saveGatesFile(path, content), () => `Saved ${path} · the gates Reflex restarted`).then((ok) => ok && setFile({ path, content, saved: content }));
  };

  const byDomain = useMemo(() => {
    const map = new Map<string, { path: string; size: number }[]>();
    for (const f of info?.files ?? []) {
      const d = f.path.split("/")[0];
      map.set(d, [...(map.get(d) ?? []), f]);
    }
    return map;
  }, [info]);

  const openProject = (slug: string) => {
    setProject(slug);
    setScope("project");
    go("projects", slug);
  };

  if (!info) {
    return <Card className="p-[18px]">{error ? <p className="text-[12px] text-bad">{error}</p> : <p className="font-mono text-[11.5px] text-muted">loading…</p>}</Card>;
  }

  const shadow = info.threshold === null;
  const s = info.status;
  const name = (slug: string) => projects.find((p) => p.project.slug === slug)?.project.name ?? slug;
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {error && <p className="text-[12px] text-bad">{error}</p>}
      <Card className="flex flex-col gap-3.5 p-[18px]">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <SectionTitle note={`port ${info.port}`}>Gates Reflex</SectionTitle>
          <Btn size="sm" onClick={restart} disabled={!!busy}>
            <RefreshCw className={clsx("size-3", busy === "restart" && "animate-spin")} />
            {busy === "restart" ? "Restarting…" : "Restart"}
          </Btn>
        </div>
        <p className="font-mono text-[12px] leading-snug">
          <span className={STATE_TONE[s.state]}>{s.state.replace("_", " ")}</span>
          <span className="text-muted">
            {" "}
            · {s.domains.length} domain{s.domains.length === 1 ? "" : "s"} · threshold {shadow ? "none (shadow)" : fmt(info.threshold)}
          </span>
          {s.message && <span className="block break-words text-muted">{s.message.split("\n")[0]}</span>}
        </p>
        <p className="text-[13px] leading-normal text-text-2">
          The orchestrator&apos;s gates (intake, audit, model tier) ask their own Reflex, shared by every project, before any LLM. Its corpus has one domain per answer:
          a choice is the domain whose notes the task reads most like.
        </p>
        <ul className="grid gap-x-6 gap-y-1 font-mono text-[11.5px] leading-snug sm:grid-cols-2">
          {DOMAINS.map((d) => (
            <li key={d.id} className="flex min-w-0 gap-2">
              <span className="w-[76px] shrink-0 text-text">{d.id}</span>
              <span className="min-w-0 text-muted">{d.text}</span>
            </li>
          ))}
        </ul>
      </Card>

      <Card className={clsx("flex flex-col gap-3 p-[18px]", shadow && "border-warn/45")}>
        <SectionTitle note={info.config_threshold === null ? "map.toml: none (shadow)" : `map.toml: ${fmt(info.config_threshold)}`}>Threshold</SectionTitle>
        {shadow ? (
          <div className="rounded-lg border border-warn/40 bg-warn/[0.07] px-3.5 py-3 text-[13px] leading-normal">
            <span className="font-semibold text-warn">Shadow mode.</span>{" "}
            <span className="text-text-2">
              Answers are logged on each project&apos;s Reflex screen but none is acted on; set a threshold once the logged answers look right.
            </span>
          </div>
        ) : (
          <p className="text-[12.5px] leading-normal text-text-2">
            An answer at or above {fmt(info.threshold)} is acted on; below it, or unsure, the orchestrator does what it does without the gate.
          </p>
        )}
        <div className="flex flex-wrap items-end gap-2">
          <label className="flex flex-col gap-1.5 font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">
            confidence from
            <TextInput
              value={text}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveThreshold();
              }}
              placeholder="e.g. 2e-3"
              aria-label="Gates threshold"
              className={clsx("w-[160px]", text && parsed === undefined && "border-bad")}
            />
          </label>
          <Btn tone="primary" onClick={saveThreshold} disabled={!!busy || parsed === undefined || draft === null}>
            {busy === "threshold" ? "Saving…" : "Set threshold"}
          </Btn>
          {(info.threshold !== info.config_threshold || draft !== null) && (
            <Btn onClick={reset} disabled={!!busy}>
              Reset to map.toml
            </Btn>
          )}
        </div>
        <p className="font-mono text-[11px] leading-snug text-muted">
          Reflex confidences are small numbers, about 1e-3 to 5e-2; write them like 2e-3. Reset goes back to map.toml&apos;s [reflex] gates_threshold (unset there: shadow).
        </p>
      </Card>

      <Card className="flex flex-col gap-3 p-[18px]">
        <SectionTitle note={`${info.files.length} file${info.files.length === 1 ? "" : "s"}`}>Corpus</SectionTitle>
        <p className="text-[12.5px] leading-normal text-text-2">Each domain&apos;s notes, in English. Saving a file restarts the gates Reflex.</p>
        {info.files.length === 0 && <p className="font-mono text-[11.5px] text-muted">No files yet: they are seeded when a project turns a gate on.</p>}
        <div className="grid min-w-0 gap-3 lg:grid-cols-[220px_minmax(0,1fr)]">
          <div className="flex min-w-0 flex-col gap-2">
            {[...byDomain.entries()].map(([d, files]) => (
              <div key={d} className="flex min-w-0 flex-col gap-0.5">
                <span className="font-mono text-[10px] tracking-[0.1em] text-muted uppercase">{d}</span>
                {files.map((f) => (
                  <button
                    key={f.path}
                    type="button"
                    onClick={() => void open(f.path)}
                    aria-pressed={file?.path === f.path}
                    className={clsx(
                      "truncate rounded-md border px-2 py-1 text-left font-mono text-[12px]",
                      file?.path === f.path ? "border-accent/45 bg-accent/10 text-text" : "border-transparent text-text-2 hover:border-line-2",
                    )}
                  >
                    {f.path.slice(d.length + 1) || f.path}
                    <span className="text-muted"> · {f.size} B</span>
                  </button>
                ))}
              </div>
            ))}
          </div>
          <div className="flex min-w-0 flex-col gap-2">
            {fileError && <p className="text-[12px] text-bad">{fileError}</p>}
            {file ? (
              <>
                <span className="font-mono text-[11.5px] text-text-2">{file.path}</span>
                <textarea
                  rows={16}
                  value={file.content}
                  onChange={(e) => setFile({ ...file, content: e.target.value })}
                  aria-label={`Contents of ${file.path}`}
                  className="w-full resize-y rounded-[7px] border border-line-2 bg-inset px-3 py-2.5 font-mono text-[12px] leading-[1.6] text-text outline-none focus:border-accent"
                />
                <div className="flex flex-wrap items-center gap-2">
                  <Btn tone="primary" onClick={saveFile} disabled={!!busy || file.content === file.saved || !file.content.trim()}>
                    {busy === "file" ? "Saving…" : "Save and restart"}
                  </Btn>
                  <Btn onClick={() => setFile({ ...file, content: file.saved })} disabled={file.content === file.saved}>
                    Undo changes
                  </Btn>
                </div>
              </>
            ) : (
              info.files.length > 0 && <p className="font-mono text-[11.5px] text-muted">Pick a file to read or edit it.</p>
            )}
          </div>
        </div>
      </Card>

      <Card className="flex flex-col gap-2.5 p-[18px]">
        <SectionTitle note="project settings → checks">Projects using the gates</SectionTitle>
        {info.projects.length === 0 ? (
          <p className="font-mono text-[11.5px] text-muted">None yet. Turn a gate on in a project&apos;s settings (Checks → Reflex gates).</p>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {info.projects.map((slug) => (
              <button
                key={slug}
                type="button"
                onClick={() => openProject(slug)}
                className="h-7 rounded-md border border-line-2 px-2.5 font-mono text-[11.5px] text-text-2 hover:border-line-3 hover:text-text"
              >
                {name(slug)}
              </button>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
