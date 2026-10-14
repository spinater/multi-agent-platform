"use client";

import clsx from "clsx";
import { ArrowLeft } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { CheckLine } from "@/lib/generated/CheckLine";
import type { InitPlan } from "@/lib/generated/InitPlan";
import type { InitStarted } from "@/lib/generated/InitStarted";
import type { PlannedFile } from "@/lib/generated/PlannedFile";
import type { TemplateInfo } from "@/lib/generated/TemplateInfo";

import { useApp } from "./app-context";
import { Btn, Check, Field, Inset, errText, signInText } from "./project-ui";
import { Card, Main, PageHeader, SectionTitle } from "./shell/page";

const LEVEL_TEXT: Record<CheckLine["level"], string> = { success: "text-ok", info: "text-info", warning: "text-warn", error: "text-bad" };
const LEVEL_MARK: Record<CheckLine["level"], string> = { success: "✓", info: "·", warning: "!", error: "✗" };
const TONE_TEXT = { ok: "text-ok", warn: "text-warn", bad: "text-bad", muted: "text-muted" };
const ACTION: Record<PlannedFile["action"], { text: string; className: string }> = {
  create: { text: "create", className: "border-ok/40 text-ok" },
  skip: { text: "skip", className: "border-line-2 text-muted" },
  append: { text: "append", className: "border-info/40 text-info" },
};

type Row = { depth: number; name: string; file: PlannedFile | null };

/** The plan as a tree: a folder row the first time a folder appears, then its files (paths come sorted). */
function treeRows(files: PlannedFile[]): Row[] {
  const rows: Row[] = [];
  const seen = new Set<string>();
  for (const f of files) {
    const parts = f.path.split("/");
    for (let i = 0; i < parts.length - 1; i++) {
      const dir = parts.slice(0, i + 1).join("/");
      if (!seen.has(dir)) {
        seen.add(dir);
        rows.push({ depth: i, name: `${parts[i]}/`, file: null });
      }
    }
    rows.push({ depth: parts.length - 1, name: parts[parts.length - 1], file: f });
  }
  return rows;
}

/** "Init project" (`#projects/init/<slug>`): pick a template, see what it does to each file of the project's folder
 * (create, skip because it exists, append missing lines), describe the project, and queue the init agent that fills
 * in the project-specific parts. Existing files are never overwritten. */
export function ProjectInit({ slug }: { slug: string }) {
  const { projects, providers, orchestrator, connection, go, setProject, setScope, openAuth, toast } = useApp();
  const info = projects.find((p) => p.project.slug === slug) ?? null;
  const [templates, setTemplates] = useState<TemplateInfo[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [chosen, setChosen] = useState("");
  const [plan, setPlan] = useState<InitPlan | null>(null);
  const [planError, setPlanError] = useState("");
  const [description, setDescription] = useState("");
  const [picked, setPicked] = useState("");
  const [model, setModel] = useState("");
  const [setVerify, setSetVerify] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<InitStarted | null>(null);
  const [shown, setShown] = useState(0);

  useEffect(() => {
    let stale = false;
    api.templates().then(
      (t) => !stale && setTemplates(t),
      (e) => !stale && setLoadError(errText(e)),
    );
    return () => {
      stale = true;
    };
  }, []);
  const template = templates?.find((t) => t.name === chosen) ?? templates?.[0] ?? null;
  const templateName = template?.name ?? null;
  const known = info !== null;

  // The plan follows the chosen template until a start, whose own plan then shows what was written.
  useEffect(() => {
    if (!templateName || !known || result) return;
    let stale = false;
    api.initPlan(slug, templateName).then(
      (p) => {
        if (stale) return;
        setPlan(p);
        setPlanError("");
      },
      (e) => !stale && setPlanError(errText(e)),
    );
    return () => {
      stale = true;
    };
  }, [slug, templateName, known, result]);

  // The project's own provider order first, then the rest; the first one that is installed is preselected.
  const order = useMemo(() => {
    const own = info?.project.settings.providers.map((p) => p.name) ?? [];
    return [...own.filter((n) => providers.some((p) => p.name === n)), ...providers.map((p) => p.name).filter((n) => !own.includes(n))];
  }, [info, providers]);
  const fallback = order.find((n) => providers.find((p) => p.name === n)?.available && providers.find((p) => p.name === n)?.signed_in !== false) ?? order.find((n) => providers.find((p) => p.name === n)?.available) ?? "";
  const provider = picked || fallback;
  const pinfo = providers.find((p) => p.name === provider);
  const projectModel = info?.project.settings.providers.find((p) => p.name === provider)?.model ?? null;

  useEffect(() => {
    if (!result || shown >= result.checks.length) return;
    const t = window.setTimeout(() => setShown((n) => n + 1), 300);
    return () => window.clearTimeout(t);
  }, [result, shown]);

  if (!info) {
    return (
      <Main>
        <BackLink slug={slug} />
        <p className="text-sm text-text-2">{connection === "connecting" ? "Loading…" : `No project ${slug}.`}</p>
      </Main>
    );
  }

  const canStart = !!template && description.trim().length > 0 && !!pinfo?.available && !busy && !result;
  const start = async () => {
    if (!template || !canStart) return;
    setBusy(true);
    setError("");
    try {
      const r = await api.initProject(slug, { template: template.name, description, provider, model: model || undefined, set_verify: setVerify });
      setResult(r);
      setPlan(r.plan);
      setShown(0);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const openTask = (id: number) => {
    setProject(slug);
    setScope("project");
    go("command", `tasks/${id}`);
  };
  const runOrchestrator = () =>
    api
      .setOrchestrator(true)
      .then(() => toast("Orchestrator running · the init task can start", "ok"))
      .catch((e) => toast(errText(e), "bad"));

  const counts = plan ? (["create", "append", "skip"] as const).map((a) => [a, plan.files.filter((f) => f.action === a).length] as const) : [];
  const fills = plan?.files.filter((f) => f.action !== "skip" && f.placeholders.length > 0) ?? [];

  return (
    <Main>
      <BackLink slug={slug} />
      <PageHeader
        kicker={`Project · ${slug} · init`}
        title="Init project"
        lead={
          <>
            Lays a development process into <span className="font-mono break-all text-text">{info.workspace}</span>: task cards, a verify gate, docs
            folders and agents for every CLI. Files that already exist are never overwritten. Then an init agent, queued as a task in this project,
            fills in the project-specific parts from your description and writes the first task cards.
          </>
        }
      />

      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1.1fr)_minmax(0,1fr)]">
        <div className="flex min-w-0 flex-col gap-4">
          <Card className="flex flex-col gap-3.5 p-[18px]">
            <SectionTitle note={templates ? `${templates.length} available` : undefined}>Template</SectionTitle>
            {loadError && <p className="text-[12px] text-bad">{loadError}</p>}
            {!templates && !loadError && <p className="font-mono text-[11px] text-muted">Loading…</p>}
            <div className="flex flex-col gap-2">
              {templates?.map((t) => {
                const on = template?.name === t.name;
                return (
                  <button
                    key={t.name}
                    type="button"
                    disabled={!!result}
                    onClick={() => setChosen(t.name)}
                    className={clsx("flex flex-col gap-1 rounded-[10px] border px-3.5 py-3 text-left disabled:cursor-default", on ? "border-accent/60 bg-accent-tint" : "border-line hover:border-line-3")}
                  >
                    <span className="flex flex-wrap items-center gap-2">
                      <span className={clsx("size-2 rounded-full border", on ? "border-accent bg-accent" : "border-line-3")} />
                      <span className="text-[13.5px] font-semibold">{t.title}</span>
                      <span className="font-mono text-[10.5px] text-muted">
                        {t.name} · {t.source === "builtin" ? "built in" : "yours"} · {t.files.length} files
                      </span>
                    </span>
                    {t.description && <span className="text-[12.5px] leading-snug text-text-2">{t.description}</span>}
                    {t.dir && <span className="font-mono text-[10.5px] break-all text-muted">{t.dir}</span>}
                  </button>
                );
              })}
            </div>
          </Card>

          <Card className="flex flex-col gap-3.5 p-[18px]">
            <SectionTitle note="required">Describe the project</SectionTitle>
            <Field label="What it does, language/stack, anything the agents must know">
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                disabled={!!result}
                rows={9}
                placeholder={"A booking API for small gyms: members book classes, staff see the day's roster.\nRust (axum) + Postgres, deployed with Docker on one VPS.\nMoney: payments go through Stripe; never store card numbers.\nStart with: sign-up, class schedule, booking with a waiting list."}
                className="min-h-44 w-full min-w-0 resize-y rounded-[7px] border border-line-2 bg-inset px-3 py-2.5 font-sans text-sm leading-relaxed tracking-normal text-text normal-case outline-none placeholder:text-muted/70 focus:border-accent disabled:opacity-60"
              />
            </Field>
            <p className="font-mono text-[11px] leading-snug text-muted">
              The init agent fills each placeholder from this text and the code already in the folder, makes scripts/verify.sh run the stack&apos;s real
              checks, and writes the first cards in tasks/todo/. It sets up a skeleton only in an empty folder, and only when you ask for one here.
            </p>
          </Card>

          <Card className="flex flex-col gap-3.5 p-[18px]">
            <SectionTitle note="runs as the task “Init project”">Init agent</SectionTitle>
            <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Provider">
              {order.map((name) => {
                const p = providers.find((x) => x.name === name);
                const state = signInText(p);
                const on = provider === name;
                return (
                  <button
                    key={name}
                    type="button"
                    role="radio"
                    aria-checked={on}
                    disabled={!p?.available || !!result}
                    onClick={() => {
                      setPicked(name);
                      setModel("");
                    }}
                    className={clsx(
                      "flex min-w-36 flex-col gap-1 rounded-[10px] border px-3 py-2.5 text-left disabled:cursor-not-allowed",
                      on ? "border-accent/60 bg-accent-tint" : "border-line hover:border-line-3",
                      !p?.available && "opacity-55",
                    )}
                  >
                    <span className="flex items-center gap-2">
                      <span className="size-[7px] rounded-full" style={{ background: providerColor(name) }} />
                      <span className="text-[13px] font-semibold">{name}</span>
                    </span>
                    <span className={clsx("font-mono text-[10.5px]", TONE_TEXT[state.tone])}>{state.text}</span>
                  </button>
                );
              })}
            </div>
            {pinfo?.available && pinfo.signed_in === false && (
              <p className="flex flex-wrap items-center gap-2 font-mono text-[11px] text-warn">
                {provider} is signed out: the task waits until it signs in.
                <Btn size="sm" tone="quiet" onClick={() => openAuth(provider)}>
                  Sign in
                </Btn>
              </p>
            )}
            <Field label="Model" hint="Optional. Default: the project's model for this provider, else the provider's default.">
              <select
                value={model}
                onChange={(e) => setModel(e.target.value)}
                disabled={!pinfo || !!result}
                className="h-[38px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[13px] tracking-normal text-text normal-case"
              >
                <option value="">default ({projectModel ?? pinfo?.default_model ?? "none"})</option>
                {pinfo?.models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </Field>
            {template && (
              <Check
                on={setVerify}
                onClick={() => !result && setSetVerify(!setVerify)}
                label="Use the template's verify commands for this project"
                sub={
                  template.verify.length
                    ? `${template.verify.join(" · ")} — replaces ${info.project.settings.verify ? "the project's own" : "the map.toml"} verify commands; they also check the init agent's work`
                    : "this template suggests no verify commands"
                }
              />
            )}
          </Card>

          {!result && (
            <div className="flex flex-wrap items-center justify-end gap-3">
              {error && <span className="min-w-0 flex-1 text-[12px] text-bad">{error}</span>}
              {!description.trim() && <span className="font-mono text-[11px] text-muted">a description is required</span>}
              <Btn tone="primary" onClick={start} disabled={!canStart}>
                {busy ? "Starting…" : "Start init"}
              </Btn>
            </div>
          )}

          {result && (
            <Card className="flex flex-col gap-3.5 p-[18px]">
              <SectionTitle note={`task #${result.task.id}`}>Started</SectionTitle>
              <Inset className="gap-0.5">
                <span className="text-muted">$ init {slug} --template {result.plan.template}</span>
                {result.checks.slice(0, shown).map((l, i) => (
                  <span key={i} className={clsx("break-words", LEVEL_TEXT[l.level])}>
                    {LEVEL_MARK[l.level]} {l.text}
                  </span>
                ))}
              </Inset>
              <div className="flex flex-wrap items-center gap-2">
                <Btn tone="primary" onClick={() => openTask(result.task.id)}>
                  Open task #{result.task.id} in the command centre →
                </Btn>
                {orchestrator && !orchestrator.running && <Btn onClick={runOrchestrator}>Run the orchestrator</Btn>}
                <Btn tone="quiet" onClick={() => go("projects", slug)}>
                  Back to {info.project.name}
                </Btn>
              </div>
            </Card>
          )}
        </div>

        <Card className="flex min-w-0 flex-col gap-3.5 p-[18px]">
          <SectionTitle note={plan ? counts.map(([a, n]) => `${n} ${a}`).join(" · ") : undefined}>{result ? "Written" : "Files"}</SectionTitle>
          {planError && <p className="text-[12px] text-bad">{planError}</p>}
          {!plan && !planError && <p className="font-mono text-[11px] text-muted">Loading…</p>}
          {plan && (
            <>
              <p className="font-mono text-[11px] leading-snug text-muted">
                {plan.folder_empty ? "The folder is empty: " : "The folder already has files: "}
                existing files are skipped, never overwritten; .gitignore only gets its missing lines.
              </p>
              <ul className="flex flex-col overflow-x-auto rounded-lg border border-line bg-inset py-2 font-mono text-[11.5px] leading-[1.75]">
                {treeRows(plan.files).map((r, i) => (
                  <li key={i} className="flex min-w-0 items-center gap-2 px-3 whitespace-nowrap" style={{ paddingLeft: 12 + r.depth * 16 }}>
                    <span className={clsx("min-w-0", r.file ? (r.file.action === "skip" ? "text-muted" : "text-text") : "text-text-2")}>{r.name}</span>
                    {r.file && (
                      <>
                        <span className={clsx("rounded border px-1.5 text-[9.5px] leading-4 font-semibold tracking-[0.08em] uppercase", ACTION[r.file.action].className)}>
                          {ACTION[r.file.action].text}
                          {r.file.action === "append" && ` +${r.file.lines.length}`}
                        </span>
                        {r.file.placeholders.length > 0 && (
                          <span className="text-[10.5px] text-warn" title={r.file.placeholders.join(", ")}>
                            fill {r.file.placeholders.length}
                          </span>
                        )}
                        {r.file.note && r.file.action !== "create" && <span className="text-[10.5px] text-muted">{r.file.note}</span>}
                      </>
                    )}
                  </li>
                ))}
              </ul>
              {fills.length > 0 && (
                <div className="flex flex-col gap-1.5">
                  <span className="label">Placeholders the init agent fills</span>
                  {fills.map((f) => (
                    <p key={f.path} className="font-mono text-[11px] leading-snug break-words">
                      <span className="text-text">{f.path}</span> <span className="text-muted">·</span> <span className="text-warn">{f.placeholders.join(", ")}</span>
                    </p>
                  ))}
                </div>
              )}
            </>
          )}
        </Card>
      </div>
    </Main>
  );
}

function BackLink({ slug }: { slug: string }) {
  const { go } = useApp();
  return (
    <a
      href={`#projects/${slug}`}
      onClick={(e) => {
        e.preventDefault();
        go("projects", slug);
      }}
      className="flex items-center gap-1.5 self-start font-mono text-[11px] text-muted hover:text-text"
    >
      <ArrowLeft className="size-3" /> {slug}
    </a>
  );
}
