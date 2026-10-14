"use client";

import clsx from "clsx";
import { ArrowLeft, Plus, RotateCw } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { api, cloneUrl, fmtConf, type RepoName } from "@/lib/api";
import type { FolderTrust } from "@/lib/generated/FolderTrust";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { RepoListing } from "@/lib/generated/RepoListing";

import { useApp } from "./app-context";
import { ProjectGit } from "./git-access";
import {
  Btn,
  Check,
  ChecksEditor,
  Field,
  Inset,
  ProviderChip,
  ProvidersEditor,
  StatusPill,
  TaskBar,
  TextInput,
  TomlView,
  ago,
  cliName,
  errText,
  figures,
  formFromSettings,
  projectProviders,
  settingsFromForm,
  tomlLines,
  useDefaults,
  type SettingsForm,
} from "./project-ui";
import { Card, Main, PageHeader, SectionTitle } from "./shell/page";
import { SessionBadge } from "./ui";

const TABS = ["overview", "corpus", "settings"] as const;
type Tab = (typeof TABS)[number];

const reflexText: Record<string, string> = { running: "text-ok", starting: "text-info", no_corpus: "text-warn", error: "text-bad", stopped: "text-muted" };

function Box({ title, note, children, className }: { title: ReactNode; note?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <Card className={clsx("flex flex-col gap-3.5 p-[18px]", className)}>
      <SectionTitle note={note}>{title}</SectionTitle>
      {children}
    </Card>
  );
}

/** A project's page (`#projects/<slug>`): its folder, activity, Reflex, the corpus editor, settings. */
export function ProjectDetail({ slug }: { slug: string }) {
  const { projects, tasks, sessions, orchestrator, go, setProject, setScope, scope, project, toast, connection, now } = useApp();
  const [tab, setTab] = useState<Tab>("overview");
  const info = projects.find((p) => p.project.slug === slug) ?? null;

  if (!info) {
    return (
      <Main>
        <BackLink />
        <p className="text-sm text-text-2">{connection === "connecting" ? "Loading…" : `No project ${slug}.`}</p>
      </Main>
    );
  }
  const p = info.project;
  const f = figures(slug, info, tasks, sessions, orchestrator);
  const current = scope === "project" && project === slug;
  const open = (screen: "command" | "reflex") => {
    setProject(slug);
    setScope("project");
    go(screen);
  };
  const togglePause = () =>
    api
      .updateProject(slug, { paused: !p.settings.paused })
      .then(() => toast(p.settings.paused ? `${p.name} resumed · its pending tasks can start` : `${p.name} paused · its pending tasks wait`, p.settings.paused ? "ok" : "warn"))
      .catch((e) => toast(errText(e), "bad"));

  return (
    <Main>
      <BackLink />
      <PageHeader
        kicker={`Project · ${slug}`}
        title={p.name}
        lead={p.description || undefined}
        actions={
          <>
            <Btn onClick={() => go("projects", `init/${slug}`)} title="Lay a development process (task cards, verify gate, docs, agents) into the folder">
              Init
            </Btn>
            <Btn onClick={togglePause}>{p.settings.paused ? "Resume" : "Pause"}</Btn>
            <Btn onClick={() => open("reflex")}>Reflex</Btn>
            <Btn tone="primary" onClick={() => open("command")}>
              Open command centre →
            </Btn>
          </>
        }
      >
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5 font-mono text-[11px] text-muted">
          <StatusPill status={f.status} />
          {current && <span className="text-[9px] font-semibold tracking-[0.14em] text-accent">CURRENT</span>}
          <span className="break-all text-text-2">{info.workspace}</span>
          <span>
            {f.live} live · {ago(f.last, now)}
            {f.attention > 0 && <span className="text-warn"> · {f.attention} need you</span>}
          </span>
        </div>
      </PageHeader>
      <nav className="flex gap-1 overflow-x-auto border-b border-line" aria-label="Project sections">
        {TABS.map((t) => (
          <button key={t} onClick={() => setTab(t)} className={clsx("label -mb-px border-b-2 px-3 py-2.5 whitespace-nowrap hover:text-text", tab === t ? "border-accent text-text" : "border-transparent")}>
            {t}
          </button>
        ))}
      </nav>
      {tab === "overview" && <Overview info={info} />}
      {tab === "corpus" && <RepoEditor key={`${slug}-corpus`} slug={slug} repo="corpus" />}
      {tab === "settings" && <Settings key={slug} info={info} />}
    </Main>
  );
}

function BackLink() {
  const { go } = useApp();
  return (
    <a
      href="#projects"
      onClick={(e) => {
        e.preventDefault();
        go("projects");
      }}
      className="flex items-center gap-1.5 self-start font-mono text-[11px] text-muted hover:text-text"
    >
      <ArrowLeft className="size-3" /> All projects
    </a>
  );
}

/** Whether each installed CLI trusts the project's folder, read from the CLI's own config, with "Trust folder" when
 * one does not (the setting was off, or the CLI rewrote its config). */
function FolderTrustRows({ slug }: { slug: string }) {
  const { toast } = useApp();
  const [trust, setTrust] = useState<FolderTrust | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let stale = false;
    api.projectTrust(slug).then(
      (t) => !stale && setTrust(t),
      () => {},
    );
    return () => {
      stale = true;
    };
  }, [slug]);
  if (!trust) return null;
  const missing = trust.providers.filter((x) => x.file && x.trusted !== true);
  const run = () => {
    setBusy(true);
    api
      .trustProject(slug)
      .then(
        (r) => {
          setTrust(r.trust);
          const bad = r.checks.find((c) => c.level === "warning" || c.level === "error");
          toast(bad ? bad.text : (r.checks[0]?.text ?? "trusted"), bad ? "warn" : "ok");
        },
        (e: Error) => toast(e.message, "bad"),
      )
      .finally(() => setBusy(false));
  };
  return (
    <div className="flex flex-col gap-1.5">
      <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11.5px]">
        {trust.providers.map((x) => (
          <div key={x.provider} className="contents">
            <dt className="text-muted">trust · {cliName(x.provider)}</dt>
            <dd className={clsx("break-words", x.trusted === true ? "text-ok" : x.file ? "text-warn" : "text-muted")} title={x.file ?? undefined}>
              {x.trusted === true ? "✓ " : x.file ? "! " : ""}
              {x.note}
            </dd>
          </div>
        ))}
      </dl>
      {missing.length > 0 && (
        <div className="flex items-center gap-2">
          <Btn size="sm" tone="primary" disabled={busy || !trust.enabled} onClick={run}>
            {busy ? "Trusting…" : "Trust folder"}
          </Btn>
          {!trust.enabled && <span className="font-mono text-[11px] text-muted">off: workspace.trust_folders = false in map.toml</span>}
        </div>
      )}
    </div>
  );
}

function Overview({ info }: { info: ProjectInfo }) {
  const { tasks, sessions, orchestrator, providers, go, setProject, setScope, openAuth } = useApp();
  const defaults = useDefaults();
  const { project: p, reflex, stats } = info;
  const f = figures(p.slug, info, tasks, sessions, orchestrator);
  const live = sessions.filter((s) => s.project === p.slug && s.state !== "exited");
  const [log, setLog] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const src = p.settings.source;
  const s = p.settings;
  const verify = s.verify ?? defaults?.verify ?? [];
  const audit = s.audit ? s.audit.enabled : (defaults?.audit_enabled ?? false);

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Box title="Folder" note={src.kind === "empty" ? "created new" : src.kind === "local" ? "existing · in place" : `legacy · ${src.kind}`}>
        <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 font-mono text-[11.5px]">
          <dt className="text-muted">agents work in</dt>
          <dd className="break-all">{info.workspace}</dd>
        </dl>
        {info.outside_roots && (
          <p className="rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 font-mono text-[11.5px] leading-normal text-warn">
            ! outside the configured project folder ([projects] path in map.toml). Agents still work here, but a project could not be created in this folder now.
          </p>
        )}
        <p className="font-mono text-[11px] text-muted">The platform never copies the code or hosts it in a git repo of its own; agents change this folder directly.</p>
        <FolderTrustRows slug={p.slug} />
        <Inset className="gap-1 overflow-x-auto">
          <span className="text-muted"># corpus: one folder per Reflex domain; a push to main reloads Reflex</span>
          <span className="whitespace-nowrap">git clone {cloneUrl(p.slug, "corpus")}</span>
        </Inset>
        <p className="font-mono text-[11px] text-muted">With server.api_token set, git asks for a password: use the token.</p>
      </Box>

      <ProjectGit info={info} />

      <Box title="Activity" note={`${f.live} live sessions`}>
        <TaskBar f={f} />
        {live.length > 0 ? (
          <ul className="flex flex-col divide-y divide-line">
            {live.map((x) => (
              <li key={x.name} className="flex items-center gap-2 py-1.5 font-mono text-[11.5px]">
                <a
                  href={`#command/sessions/${x.name}`}
                  onClick={(e) => {
                    e.preventDefault();
                    setProject(p.slug);
                    setScope("project");
                    go("command", `sessions/${x.name}`);
                  }}
                  className="min-w-0 flex-1 truncate"
                >
                  {x.name}
                </a>
                <span className="text-muted">{x.provider}</span>
                <SessionBadge state={x.state} />
              </li>
            ))}
          </ul>
        ) : (
          <p className="font-mono text-[11px] text-muted">No live sessions.</p>
        )}
      </Box>

      <Box
        title="Reflex"
        note={`port ${p.port}`}
        className="lg:order-none"
      >
        <p className="flex flex-wrap items-center gap-2 font-mono text-[11.5px]">
          <span className={reflexText[reflex.state]}>{reflex.state.replace("_", " ")}</span>
          <span className="text-muted">· domains {reflex.domains.length ? reflex.domains.join(", ") : "none"}</span>
        </p>
        {reflex.message && <Inset className="whitespace-pre-wrap text-text-2">{reflex.message}</Inset>}
        <div className="grid grid-cols-4 gap-3">
          {(
            [
              ["decisions", stats.total],
              ["reflex", stats.reflex],
              ["llm", stats.llm],
              ["abstain", stats.abstain],
            ] as const
          ).map(([k, v]) => (
            <div key={k}>
              <p className="font-mono text-lg font-semibold tabular-nums">{v}</p>
              <p className="label">{k}</p>
            </div>
          ))}
        </div>
        <p className="font-mono text-[11px] text-muted">
          threshold {fmtConf(p.threshold)} · fallback {p.fallback}
          {p.model ? ` ${p.model}` : ""}
        </p>
        <div className="flex flex-wrap gap-2">
          <Btn
            size="sm"
            tone="quiet"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              await api.restartReflex(p.slug).finally(() => setBusy(false));
            }}
          >
            <RotateCw className={clsx("size-3", busy && "animate-spin")} /> Restart
          </Btn>
          <Btn size="sm" tone="quiet" onClick={async () => setLog((await api.reflexLog(p.slug)).log || "(empty)")}>
            Log
          </Btn>
          <Btn
            size="sm"
            tone="quiet"
            onClick={() => {
              setProject(p.slug);
              setScope("project");
              go("reflex");
            }}
          >
            Playground, calibrate and decisions →
          </Btn>
        </div>
        {log !== null && <pre className="max-h-60 overflow-auto rounded-lg bg-term-bg p-2.5 font-mono text-[11px] text-term-text">{log}</pre>}
      </Box>

      <Box title="Providers & checks" note={s.paused ? "paused" : undefined}>
        <div className="flex flex-wrap gap-2">
          {projectProviders(info, defaults, providers).map((name) => (
            <ProviderChip key={name} name={name} info={providers.find((x) => x.name === name)} onSignIn={openAuth} />
          ))}
        </div>
        <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 font-mono text-[11.5px]">
          <dt className="text-muted">order</dt>
          <dd>{s.providers.length ? "this project's" : "map.toml provider_order"}</dd>
          <dt className="text-muted">verify</dt>
          <dd className="break-words">
            {verify.length ? verify.join(" · ") : "none"} <span className="text-muted">({s.verify ? "project" : "map.toml"})</span>
          </dd>
          <dt className="text-muted">audit</dt>
          <dd>
            {audit ? `on · ${s.audit?.provider ?? defaults?.audit_chain.join(" → ") ?? ""}` : "off"} <span className="text-muted">({s.audit ? "project" : "map.toml"})</span>
          </dd>
        </dl>
      </Box>
    </div>
  );
}

function RepoEditor({ slug, repo }: { slug: string; repo: RepoName }) {
  const [listing, setListing] = useState<RepoListing | null>(null);
  const [path, setPath] = useState("");
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  // Bumped after each commit to reload the listing.
  const [version, setVersion] = useState(0);

  useEffect(() => {
    let stale = false;
    api.repo(slug, repo).then(
      (l) => !stale && setListing(l),
      (e) => !stale && setError(errText(e)),
    );
    return () => {
      stale = true;
    };
  }, [slug, repo, version]);

  const open = async (p: string) => {
    setError("");
    setStatus("");
    try {
      const r = await api.readFile(slug, repo, p);
      setPath(p);
      setContent(r.content);
      setOriginal(r.content);
    } catch (e) {
      setError(errText(e));
    }
  };

  const save = async (remove = false) => {
    setBusy(true);
    setError("");
    try {
      const r = await api.writeFile(slug, repo, { path, content: remove ? null : content, message });
      setStatus(`committed ${r.commit.slice(0, 8)} · Reflex restarted`);
      setMessage("");
      if (remove) {
        setPath("");
        setContent("");
        setOriginal(null);
      } else setOriginal(content);
      setVersion((v) => v + 1);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const dirty = original === null ? path !== "" : content !== original;

  return (
    <div className="flex flex-col gap-4">
      <p className="max-w-[760px] text-[13px] leading-normal text-text-2">
        Each top-level folder is one Reflex domain (1 to 8). Write each <code>.md</code> as prose describing that domain&apos;s situations and how they are handled, like a
        runbook. English works best.
      </p>
      <div className="grid gap-4 lg:grid-cols-[260px_minmax(0,1fr)]">
        <Box
          title="Files"
          note={
            <button
              className="flex items-center gap-1 text-link hover:text-link-hover"
              onClick={() => {
                setPath("domain/notes.md");
                setContent("");
                setOriginal(null);
                setStatus("");
              }}
            >
              <Plus className="size-3" /> new
            </button>
          }
        >
          <ul className="-mx-2 max-h-[420px] overflow-auto">
            {listing?.files.map((f) => (
              <li key={f.path}>
                <button
                  onClick={() => open(f.path)}
                  className={clsx("w-full truncate rounded-md px-2 py-1 text-left font-mono text-[12px] hover:bg-white/5", f.path === path && "bg-accent-tint text-accent-soft")}
                >
                  {f.path}
                </button>
              </li>
            ))}
            {listing && listing.files.length === 0 && <li className="px-2 py-1 font-mono text-[12px] text-muted">empty</li>}
          </ul>
        </Box>
        <Box title={original === null && path ? "new file" : path || "editor"}>
          <TextInput value={path} onChange={(e) => setPath(e.target.value)} readOnly={original !== null} placeholder="path/in/repo.md" />
          <textarea
            rows={16}
            value={content}
            onChange={(e) => setContent(e.target.value)}
            className="w-full rounded-[7px] border border-line-2 bg-inset p-3 font-mono text-[12.5px] leading-5 text-text outline-none focus:border-accent"
          />
          <div className="flex flex-wrap gap-2">
            <TextInput className="h-[34px] min-w-48 flex-1" value={message} onChange={(e) => setMessage(e.target.value)} placeholder="commit message (optional)" />
            <Btn tone="primary" disabled={busy || !path || !dirty} onClick={() => save()}>
              {busy ? "Committing…" : "Commit"}
            </Btn>
            {original !== null && (
              <Btn tone="danger" disabled={busy} onClick={() => confirm(`Delete ${path}?`) && save(true)}>
                Delete
              </Btn>
            )}
          </div>
          {status && <p className="font-mono text-[11px] text-muted">{status}</p>}
          {error && <p className="text-[12px] text-bad">{error}</p>}
        </Box>
      </div>
      <Box title="History" note={`${listing?.commits.length ?? 0} recent`}>
        <ul className="flex flex-col divide-y divide-line">
          {listing?.commits.map((c) => (
            <li key={c.sha} className="flex flex-wrap gap-x-3 py-1.5 font-mono text-[11.5px]">
              <span className="text-muted">{c.sha.slice(0, 8)}</span>
              <span className="min-w-0 flex-1 truncate">{c.subject}</span>
              <span className="shrink-0 text-muted">
                {c.author} · {new Date(c.at).toLocaleString()}
              </span>
            </li>
          ))}
        </ul>
      </Box>
    </div>
  );
}

function FallbackSelect({ value, onChange, providers }: { value: string; onChange: (v: string) => void; providers: ProviderInfo[] }) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} className="h-[38px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[13px] tracking-normal text-text normal-case">
      <option value="api">Anthropic API</option>
      {providers.map((p) => (
        <option key={p.name} value={p.name} disabled={!p.available}>
          {p.name} (headless){p.available ? "" : " · not installed"}
        </option>
      ))}
      <option value="none">None: abstain stays unanswered</option>
    </select>
  );
}

function Settings({ info }: { info: ProjectInfo }) {
  const { providers, openAuth, go, toast } = useApp();
  const defaults = useDefaults();
  const p = info.project;
  const [name, setName] = useState(p.name);
  const [description, setDescription] = useState(p.description);
  const [fallback, setFallback] = useState(p.fallback);
  const [model, setModel] = useState(p.model ?? "");
  const [threshold, setThreshold] = useState(p.threshold === null ? "" : String(p.threshold));
  const [edited, setEdited] = useState<SettingsForm | null>(null);
  const [confirmSlug, setConfirmSlug] = useState("");
  const [error, setError] = useState("");
  const form = edited ?? (providers.length ? formFromSettings(p.settings, providers, defaults) : null);
  const patch = (x: Partial<SettingsForm>) => form && setEdited({ ...form, ...x });
  const provider = providers.find((x) => x.name === fallback);
  const settings = form ? settingsFromForm(form, defaults, p.settings.source) : p.settings;

  const saveIdentity = async () => {
    setError("");
    try {
      const t = threshold.trim() === "" ? null : Number(threshold);
      if (t !== null && !Number.isFinite(t)) throw new Error("threshold must be a number, or empty");
      await api.updateProject(p.slug, { name, description, fallback, model: model.trim() || null, threshold: t });
      toast("project saved");
    } catch (e) {
      setError(errText(e));
    }
  };
  const saveSettings = async () => {
    setError("");
    try {
      const r = await api.updateProject(p.slug, { settings });
      setEdited(null);
      const warn = r.checks.some((c) => c.level === "warning" || c.level === "error");
      toast(["orchestrator settings saved", ...r.checks.map((c) => c.text)].join(" · "), warn ? "warn" : "ok");
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <div className="flex flex-col gap-4">
        <Box title="Project">
          <Field label="Name">
            <TextInput mono={false} value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
          <Field label="Description">
            <TextInput mono={false} value={description} onChange={(e) => setDescription(e.target.value)} />
          </Field>
          <Field label="When Reflex is unsure, ask">
            <FallbackSelect value={fallback} onChange={setFallback} providers={providers} />
          </Field>
          <Field label={provider ? `Fallback model (${provider.models.join(", ") || "any"})` : "Fallback model (empty: the server's default)"}>
            {provider && provider.models.length > 0 ? (
              <select value={model} onChange={(e) => setModel(e.target.value)} className="h-[38px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[13px] tracking-normal text-text normal-case">
                <option value="">default ({provider.default_model})</option>
                {provider.models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            ) : (
              <TextInput value={model} onChange={(e) => setModel(e.target.value)} placeholder="claude-haiku-5-5" />
            )}
          </Field>
          <Field label="Reflex confidence threshold" hint="Empty: every question goes to the fallback. Prefer fitting it on the Reflex screen.">
            <TextInput value={threshold} onChange={(e) => setThreshold(e.target.value)} placeholder="e.g. 1.2e-5" />
          </Field>
          <div className="flex items-center gap-2">
            <Btn tone="primary" onClick={saveIdentity}>
              Save
            </Btn>
            <span className="font-mono text-[11px] text-muted">session prefix {defaults?.session_prefix} · set in map.toml</span>
          </div>
        </Box>
        {form && (
          <Box title="Providers" note="tried top to bottom">
            <ProvidersEditor rows={form.providers} onChange={(rows) => patch({ providers: rows })} providers={providers} onSignIn={openAuth} />
            <Check on={form.paused} onClick={() => patch({ paused: !form.paused })} label="Paused" sub="the orchestrator starts none of this project's pending tasks" />
          </Box>
        )}
        {form && (
          <Box title="Checks">
            <ChecksEditor form={form} onChange={patch} defaults={defaults} providers={providers} domains={info.reflex.domains.length || info.reflex.state === "no_corpus" ? info.reflex.domains : undefined} />
            <div className="flex items-center gap-2">
              <Btn tone="primary" onClick={saveSettings} disabled={!edited}>
                Save orchestrator settings
              </Btn>
              {edited && (
                <Btn tone="quiet" onClick={() => setEdited(null)}>
                  Discard
                </Btn>
              )}
            </div>
          </Box>
        )}
        {error && <p className="text-[12px] text-bad">{error}</p>}
      </div>
      <div className="flex flex-col gap-4">
        <Box title="As TOML" note="stored in the platform database, not a file">
          <TomlView lines={tomlLines({ name, slug: p.slug, description, fallback, model }, settings, defaults)} className="max-h-[520px]" />
        </Box>
        <Box title="Delete project">
          <p className="text-[13px] leading-normal text-text-2">
            Stops its Reflex and permanently deletes both repos, the agent workspace and the decision log. Running tasks must be cancelled first. Type <code>{p.slug}</code> to
            confirm.
          </p>
          <TextInput value={confirmSlug} onChange={(e) => setConfirmSlug(e.target.value)} aria-label="Type the slug to confirm" />
          <div>
            <Btn
              tone="danger"
              disabled={confirmSlug !== p.slug}
              onClick={() =>
                api
                  .deleteProject(p.slug)
                  .then(() => {
                    toast(`${p.slug} deleted`, "warn");
                    go("projects");
                  })
                  .catch((e) => setError(errText(e)))
              }
            >
              Delete project
            </Btn>
          </div>
        </Box>
      </div>
    </div>
  );
}
