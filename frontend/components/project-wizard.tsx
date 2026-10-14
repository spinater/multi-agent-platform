"use client";

import clsx from "clsx";
import { Hexagon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { CheckLine } from "@/lib/generated/CheckLine";
import type { SourceInspection } from "@/lib/generated/SourceInspection";
import type { SourceKind } from "@/lib/generated/SourceKind";

import { useApp } from "./app-context";
import { FolderPicker } from "./folder-picker";
import { GitAccessDialog } from "./git-access";
import { useGitAccess } from "./settings/git-ssh";
import {
  Btn,
  Check,
  ChecksEditor,
  Field,
  Inset,
  ProvidersEditor,
  Segmented,
  TextInput,
  TomlView,
  errText,
  formFromSettings,
  settingsFromForm,
  suggestVerify,
  tomlLines,
  trustLine,
  useDefaults,
  type SettingsForm,
} from "./project-ui";

const STEPS: [string, string][] = [
  ["Folder", "existing · new · clone"],
  ["Identity", "name · prefix"],
  ["Providers", "order · parallel"],
  ["Checks", "verify · audit"],
  ["Review", "settings"],
];

/** Same rule as the server's `slugify`. */
export function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/, "");
}

const base = (p: string) => p.trim().replace(/\/+$/, "").split("/").pop() ?? "";

type Source = { kind: SourceKind; location: string; url: string; branch: string };

const failedInspection: SourceInspection = { ok: false, error: null, default_branch: null, branches: [], files: [], commits: null, hint: null, folder: null, root: null };

/** The 5-step "New project" wizard (Projects.dc.html). A project is a folder on this machine that agents work in
 * directly, an existing one or a new empty one; the platform never copies or hosts the code. The settings are stored
 * in the platform database. `onCreated` gets the server's check lines. */
export function ProjectWizard({ onClose, onCreated, initialStep = 0 }: { onClose: () => void; onCreated: (slug: string, checks: CheckLine[]) => void; initialStep?: number }) {
  const { providers, server, openAuth, orchestrator } = useApp();
  const defaults = useDefaults();
  const [step, setStep] = useState(initialStep);
  const [source, setSource] = useState<Source>({ kind: "local", location: "", url: "", branch: "" });
  const [folderTouched, setFolderTouched] = useState(false);
  const [remoteInspection, setRemoteInspection] = useState<SourceInspection | null>(null);
  const [remoteChecking, setRemoteChecking] = useState(false);
  const [gitDialog, setGitDialog] = useState(false);
  // Bumped when the Git access dialog closes, so the repo is looked at again with a new key.
  const [recheck, setRecheck] = useState(0);
  const gitAccess = useGitAccess();
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [slug, setSlug] = useState("");
  const [description, setDescription] = useState("");
  const [fallback, setFallback] = useState("api");
  const [form, setForm] = useState<SettingsForm | null>(null);
  const [inspection, setInspection] = useState<SourceInspection | null>(null);
  const [inspecting, setInspecting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  // The provider rows wait for map.toml's order, then keep the user's edits.
  const rows = form ?? (providers.length ? formFromSettings(null, providers, defaults) : null);
  const patch = (p: Partial<SettingsForm>) => rows && setForm({ ...rows, ...p });

  useEffect(() => {
    // Escape closes the Git access dialog first, when it is open on top.
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !gitDialog && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, gitDialog]);

  // A repo to clone: `git ls-remote` as the URL is typed (default branch, branches, or why the host refused).
  const cloning = source.kind === "clone";
  const url = source.url.trim();
  const branch = source.branch.trim();
  useEffect(() => {
    if (!cloning || !url) return;
    let stale = false;
    const t = window.setTimeout(() => {
      setRemoteChecking(true);
      api
        .inspectSource({ kind: "clone", location: url, branch: branch || undefined })
        .then((r) => !stale && setRemoteInspection(r))
        .catch((e) => !stale && setRemoteInspection({ ...failedInspection, error: errText(e) }))
        .finally(() => !stale && setRemoteChecking(false));
    }, 700);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [cloning, url, branch, recheck]);
  const remoteSeen = cloning && url ? remoteInspection : null;

  // Look at the folder as it is typed (git branch, commits, top-level files; or that a new one is free). A clone
  // goes into `<project folder>/<repo name>` unless another folder is typed.
  const location = (cloning && !folderTouched ? (remoteSeen?.folder ?? "") : source.location).trim();
  const ready = /^(\/|~\/)./.test(location);
  const folderKind: SourceKind = cloning ? "empty" : source.kind;
  useEffect(() => {
    if (!ready) return;
    let stale = false;
    const t = window.setTimeout(() => {
      setInspecting(true);
      api
        .inspectSource({ kind: folderKind, location })
        .then((r) => !stale && setInspection(r))
        .catch((e) => !stale && setInspection({ ...failedInspection, error: errText(e) }))
        .finally(() => !stale && setInspecting(false));
    }, 600);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [ready, folderKind, location]);
  const seen = ready ? inspection : null;

  const shownName = nameTouched ? name : base(location);
  const shownSlug = slug || slugify(shownName);
  const prefix = server?.session_prefix ?? defaults?.session_prefix ?? "map-";
  const sourceValue = cloning
    ? { kind: source.kind, location: location || null, branch: branch || null, remote: url || null }
    : { kind: source.kind, location: location || null, branch: null, remote: null };
  const settings = rows ? settingsFromForm(rows, defaults, sourceValue) : null;
  const toml = settings ? tomlLines({ name: shownName, slug: shownSlug, description, fallback, model: "" }, settings, defaults) : [];

  const canNext =
    step === 0
      ? ready && seen?.ok === true && (!cloning || remoteSeen?.ok === true)
      : step === 1
        ? shownName.trim().length > 0 && /^[a-z0-9][a-z0-9-]{0,38}[a-z0-9]$/.test(shownSlug)
        : step === 2
          ? !!rows?.providers.some((p) => p.enabled)
          : true;

  const create = async () => {
    if (!settings) return;
    setBusy(true);
    setError("");
    try {
      const r = await api.createProject({ name: shownName.trim(), slug: shownSlug, description, fallback, settings });
      onCreated(r.project.project.slug, r.checks);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const next = () => {
    if (!canNext || busy) return;
    if (step === 4) void create();
    else setStep(step + 1);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-[rgba(3,4,6,.72)] p-3 backdrop-blur-sm sm:items-center sm:p-5" onClick={onClose}>
      <div
        role="dialog"
        aria-label="New project"
        onClick={(e) => e.stopPropagation()}
        className="flex w-full max-w-[860px] flex-wrap overflow-hidden rounded-2xl border border-white/10 bg-menu shadow-[0_30px_80px_rgba(0,0,0,.6)] sm:max-h-[calc(100vh-40px)]"
      >
        <aside className="flex max-w-full flex-[1_1_200px] flex-col gap-4 border-b border-line bg-white/[.015] px-4 py-5 sm:border-r sm:border-b-0">
          <span className="flex items-center gap-2 font-mono text-[11px] font-semibold tracking-[0.14em] whitespace-nowrap uppercase">
            <Hexagon className="size-3 text-accent" strokeWidth={2.5} />
            New project
          </span>
          <div className="flex flex-row flex-wrap gap-0.5 sm:flex-col">
            {STEPS.map(([label, sub], i) => (
              <button
                key={label}
                type="button"
                onClick={() => i <= step && setStep(i)}
                className={clsx("flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left", i === step && "bg-accent/10", i <= step ? "text-text" : "text-muted")}
              >
                <span
                  className={clsx(
                    "flex size-5 shrink-0 items-center justify-center rounded-full border font-mono text-[10px] font-semibold",
                    i <= step ? "border-accent" : "border-white/20",
                    i < step ? "bg-accent text-accent-text" : i === step ? "text-accent-soft" : "text-muted",
                  )}
                >
                  {i < step ? "✓" : i + 1}
                </span>
                <span className="flex min-w-0 flex-col gap-0.5">
                  <span className="text-[13px] leading-tight font-medium">{label}</span>
                  <span className="hidden font-mono text-[10px] text-muted sm:block">{sub}</span>
                </span>
              </button>
            ))}
          </div>
        </aside>

        <div className="flex min-w-0 flex-[3_1_420px] flex-col sm:max-h-[calc(100vh-40px)]">
          <div className="flex flex-1 flex-col gap-[18px] overflow-auto px-5 py-6 sm:px-[26px]">
            {step === 0 && (
              <>
                <Heading title="Which folder do agents work in?">
                  A folder on this machine. Agents work in it directly: the platform never copies the code, never hosts it in a git repo of its own and
                  adds no files to it. It can clone a repo into a new folder for you, with its own SSH key.
                </Heading>
                <div className="flex max-w-full overflow-x-auto">
                  <Segmented<SourceKind>
                    value={source.kind}
                    options={[
                      ["local", "Existing"],
                      ["empty", "New folder"],
                      ["clone", "Clone from git"],
                    ]}
                    onChange={(kind) => {
                      setSource({ ...source, kind });
                      setInspection(null);
                    }}
                  />
                </div>
                {cloning && (
                  <>
                    <div className="grid gap-[18px] sm:grid-cols-[minmax(0,1fr)_170px]">
                      <Field label="Repo URL" hint="An ssh URL uses MAP's key; https works for public repos only (MAP takes no tokens).">
                        <TextInput value={source.url} onChange={(e) => setSource({ ...source, url: e.target.value })} placeholder="git@github.com:you/app.git" autoFocus />
                      </Field>
                      <Field label="Branch" hint="Empty: the default.">
                        <TextInput value={source.branch} onChange={(e) => setSource({ ...source, branch: e.target.value })} placeholder={remoteSeen?.default_branch ?? "main"} />
                      </Field>
                    </div>
                    {url && (
                      <Inset>
                        <span className="break-all text-muted">{remoteChecking ? `git ls-remote ${url}…` : `git ls-remote ${url}`}</span>
                        {remoteSeen?.ok && (
                          <span className="text-ok">
                            ✓ reachable · default branch {remoteSeen.default_branch ?? "(none: an empty repo)"} · {remoteSeen.branches.length} branch
                            {remoteSeen.branches.length === 1 ? "" : "es"}
                            {remoteSeen.branches.length > 0 && remoteSeen.branches.length <= 8 && `: ${remoteSeen.branches.join(", ")}`}
                          </span>
                        )}
                        {remoteSeen?.error && <span className="break-words whitespace-pre-wrap text-bad">✗ {remoteSeen.error}</span>}
                        {remoteSeen?.hint && <span className="text-warn">! {remoteSeen.hint}</span>}
                      </Inset>
                    )}
                    <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 font-mono text-[11px] text-muted">
                      <span className="break-all">
                        {gitAccess.access?.key ? `MAP's key ${gitAccess.access.key.fingerprint}` : gitAccess.access ? "MAP has no SSH key yet: private repos over ssh need one" : "MAP's key…"}
                      </span>
                      <button type="button" onClick={() => setGitDialog(true)} className="text-link hover:text-link-hover">
                        Git access →
                      </button>
                    </div>
                  </>
                )}
                <div className="flex flex-col gap-1.5">
                  <span className="font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">
                    {source.kind === "local" ? "Folder" : cloning ? "Clone into" : "New folder"}
                  </span>
                  <span className="text-[11px] text-muted">
                    {source.kind === "local"
                      ? "A sub-folder of the project folder set in map.toml ([projects] path)."
                      : cloning
                        ? "A new or empty sub-folder of the project folder; by default the repo's name."
                        : "Name a new sub-folder of the project folder; it is created empty."}
                  </span>
                  <FolderPicker
                    mode={source.kind === "local" ? "existing" : "new"}
                    value={cloning && !folderTouched ? location : source.location}
                    suggestedName={cloning ? base(remoteSeen?.folder ?? "") : ""}
                    onChange={(path) => {
                      setSource({ ...source, location: path });
                      setFolderTouched(true);
                    }}
                  />
                </div>
                {ready && (
                  <Inset>
                    <span className="text-muted">{inspecting ? `looking at ${location}…` : location}</span>
                    {seen?.error && <span className="text-bad">✗ {seen.error}</span>}
                    {seen?.ok && source.kind === "empty" && <span className="text-ok">✓ free: the folder will be created empty</span>}
                    {seen?.ok && cloning && <span className="text-ok">✓ free: git clone makes it a normal folder project, used in place</span>}
                    {seen?.ok && source.kind === "local" && (
                      <>
                        {seen.default_branch ? (
                          <span className="text-ok">
                            ✓ git repo · on {seen.default_branch} · {seen.branches.length} branch{seen.branches.length === 1 ? "" : "es"}
                            {seen.commits !== null && ` · ${seen.commits} commits`}
                          </span>
                        ) : (
                          <span className="text-warn">! not a git repo: agents still work here, but there is no history to review their changes against</span>
                        )}
                        {seen.files.length > 0 ? (
                          <span className="text-ok">✓ {seen.files.filter((f) => !f.startsWith(".")).slice(0, 10).join(" · ")}</span>
                        ) : (
                          <span className="text-muted">the folder is empty</span>
                        )}
                      </>
                    )}
                  </Inset>
                )}
                {defaults && <p className="font-mono text-[11px] leading-[1.5] text-muted">{trustLine(defaults.trust_folders, defaults.trust_providers)}</p>}
              </>
            )}

            {step === 1 && (
              <>
                <Heading title="Name it">
                  The platform only manages tmux sessions that start with the prefix. Anything else on the machine is never listed, typed into or closed.
                </Heading>
                <Field label="Project name">
                  <TextInput
                    mono={false}
                    value={shownName}
                    onChange={(e) => {
                      setName(e.target.value);
                      setNameTouched(true);
                    }}
                    placeholder="my-repo"
                    autoFocus
                  />
                </Field>
                <div className="grid gap-[18px] sm:grid-cols-2">
                  <Field label="Slug" hint="The project's id in URLs and the API.">
                    <TextInput value={shownSlug} onChange={(e) => setSlug(e.target.value)} placeholder="from the name" />
                  </Field>
                  <Field label="Session prefix" hint="Set in map.toml (workspace.session_prefix); shared by every project.">
                    <TextInput value={prefix} readOnly disabled />
                  </Field>
                </div>
                <div className="font-mono text-[11.5px] leading-relaxed text-muted">
                  sessions will be named <span className="text-accent-soft">{prefix}task-12-1</span> · <span className="text-accent-soft">{prefix}claude-142233</span>
                </div>
                <Field label="Description">
                  <TextInput mono={false} value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What this project is for" />
                </Field>
                <Field label="When Reflex is unsure, ask" hint="The project's decision engine answers locally first; this answers the rest.">
                  <select value={fallback} onChange={(e) => setFallback(e.target.value)} className="h-[38px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[13px] tracking-normal text-text normal-case">
                    <option value="api">{server?.api_fallback ?? "Anthropic API"}</option>
                    {providers.map((p) => (
                      <option key={p.name} value={p.name} disabled={!p.available}>
                        {p.name} (headless){p.available ? "" : " · not installed"}
                      </option>
                    ))}
                    <option value="none">None: unanswered questions stay unanswered</option>
                  </select>
                </Field>
              </>
            )}

            {step === 2 && rows && (
              <>
                <Heading title="Which agents can work here?">
                  The orchestrator tries providers top to bottom and skips any that are missing or cooling down after a quota or sign-in failure.
                </Heading>
                <ProvidersEditor rows={rows.providers} onChange={(providers) => patch({ providers })} providers={providers} onSignIn={openAuth} />
                <div className="flex flex-wrap items-center gap-x-7 gap-y-3">
                  <span className="font-mono text-[10.5px] tracking-[0.1em] text-muted uppercase">
                    tasks at once <span className="text-[15px] font-semibold tracking-normal text-text">{orchestrator?.max_parallel ?? defaults?.max_parallel ?? "?"}</span>{" "}
                    <span className="tracking-normal normal-case">for all projects · changed live in the Command centre; a project limit is in its settings</span>
                  </span>
                  <Check on={rows.paused} onClick={() => patch({ paused: !rows.paused })} label="Start paused" sub="tasks wait until you resume the project" />
                </div>
              </>
            )}

            {step === 3 && rows && (
              <>
                <Heading title="How is a task proven done?">
                  Every command must exit 0 after the agent reports done, or the task is retried with the failing output in its prompt.
                </Heading>
                <ChecksEditor form={rows} onChange={patch} defaults={defaults} providers={providers} suggestions={suggestVerify(seen?.files ?? [])} />
              </>
            )}

            {step === 4 && (
              <>
                <Heading title="Review the settings">
                  Shown as TOML for review. They are stored in the platform database, not written to a file: change them later in the project&apos;s settings.
                  map.toml itself is not touched.
                </Heading>
                <TomlView lines={toml} />
              </>
            )}
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2.5 border-t border-line px-5 py-3.5">
            <Btn tone="quiet" className="border-0 text-text-2" onClick={onClose}>
              Cancel
            </Btn>
            {error && <span className="order-last w-full text-[12px] text-bad sm:order-none sm:w-auto sm:flex-1">{error}</span>}
            <div className="flex gap-2">
              <Btn onClick={() => setStep(Math.max(0, step - 1))} disabled={step === 0}>
                Back
              </Btn>
              <Btn tone="primary" onClick={next} disabled={!canNext || busy}>
                {step === 4 ? (busy ? "Creating…" : "Create project") : "Continue"}
              </Btn>
            </div>
          </div>
        </div>
      </div>
      {gitDialog && (
        <div onClick={(e) => e.stopPropagation()}>
          <GitAccessDialog
            onClose={() => {
              setGitDialog(false);
              gitAccess.reload();
              setRecheck((r) => r + 1);
            }}
          />
        </div>
      )}
    </div>
  );
}

function Heading({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <h2 className="text-xl leading-tight font-semibold">{title}</h2>
      <p className="text-[13px] leading-normal text-text-2">{children}</p>
    </div>
  );
}
