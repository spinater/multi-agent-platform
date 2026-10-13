"use client";

import clsx from "clsx";
import { Hexagon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import type { CheckLine } from "@/lib/generated/CheckLine";
import type { SourceInspection } from "@/lib/generated/SourceInspection";
import type { SourceKind } from "@/lib/generated/SourceKind";

import { useApp } from "./app-context";
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
  useDefaults,
  type SettingsForm,
} from "./project-ui";

const STEPS: [string, string][] = [
  ["Source", "new · local · git"],
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

const base = (p: string) =>
  p
    .trim()
    .replace(/\/+$/, "")
    .split(/[/:]/)
    .pop()
    ?.replace(/\.git$/, "") ?? "";

type Source = { kind: SourceKind; location: string; branch: string };

/** The 5-step "New project" wizard (Projects.dc.html). It creates a real project: the code repo is new or imported
 * with `git clone --bare`, and the settings are stored in the platform database. `onCreated` gets the server's check
 * lines. */
export function ProjectWizard({ onClose, onCreated, initialStep = 0 }: { onClose: () => void; onCreated: (slug: string, checks: CheckLine[]) => void; initialStep?: number }) {
  const { providers, server, openAuth } = useApp();
  const defaults = useDefaults();
  const [step, setStep] = useState(initialStep);
  const [source, setSource] = useState<Source>({ kind: "empty", location: "", branch: "" });
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
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Look at the source as it is typed (branches, commits, top-level files).
  const location = source.location.trim();
  const ready = source.kind === "git" ? location.length > 6 : source.kind === "local" ? /^(\/|~\/)./.test(location) : false;
  useEffect(() => {
    if (!ready) return;
    let stale = false;
    const t = window.setTimeout(() => {
      setInspecting(true);
      api
        .inspectSource({ kind: source.kind, location, branch: source.branch.trim() || undefined })
        .then((r) => !stale && setInspection(r))
        .catch((e) => !stale && setInspection({ ok: false, error: errText(e), default_branch: null, branches: [], files: [], commits: null }))
        .finally(() => !stale && setInspecting(false));
    }, 600);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [ready, source.kind, location, source.branch]);
  const seen = ready ? inspection : null;

  const shownName = nameTouched ? name : base(source.location);
  const shownSlug = slug || slugify(shownName);
  const prefix = server?.session_prefix ?? defaults?.session_prefix ?? "map-";
  const sourceValue = { kind: source.kind, location: source.kind === "empty" ? null : location || null, branch: source.kind === "empty" ? null : source.branch.trim() || null };
  const settings = rows ? settingsFromForm(rows, defaults, sourceValue) : null;
  const toml = settings ? tomlLines({ name: shownName, slug: shownSlug, description, fallback, model: "" }, settings, defaults) : [];

  const canNext =
    step === 0
      ? source.kind === "empty" || (ready && seen?.ok !== false)
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
                <Heading title="Where does the code live?">
                  Agents work in a checkout of the project&apos;s code repo, served at <code>/git/&lt;slug&gt;/code.git</code>. Start it empty, or import a git
                  repo with <code>git clone --bare</code>.
                </Heading>
                <Segmented<SourceKind>
                  value={source.kind}
                  options={[
                    ["empty", "New repo"],
                    ["local", "Local repo"],
                    ["git", "Git URL"],
                  ]}
                  onChange={(kind) => {
                    setSource({ ...source, kind });
                    setInspection(null);
                  }}
                />
                {source.kind === "empty" && (
                  <p className="text-[13px] leading-normal text-text-2">A new repo with a README and a CLAUDE.md that points agents at the project&apos;s decision endpoint.</p>
                )}
                {source.kind !== "empty" && (
                  <>
                    <Field label={source.kind === "git" ? "Repository URL" : "Repo on this machine"}>
                      <TextInput
                        value={source.location}
                        onChange={(e) => setSource({ ...source, location: e.target.value })}
                        placeholder={source.kind === "git" ? "https://github.com/you/your-repo.git" : "~/code/my-repo"}
                        autoFocus
                      />
                    </Field>
                    <Field label="Branch" hint="Imported as the platform repo's main. Empty: the source's default branch.">
                      <TextInput value={source.branch} onChange={(e) => setSource({ ...source, branch: e.target.value })} placeholder={seen?.default_branch ?? "main"} list="wizard-branches" />
                      <datalist id="wizard-branches">
                        {seen?.branches.map((b) => (
                          <option key={b} value={b} />
                        ))}
                      </datalist>
                    </Field>
                    {ready && (
                      <Inset>
                        <span className="text-muted">{inspecting ? `looking at ${location}…` : `found at ${location}`}</span>
                        {seen?.error && <span className="text-bad">✗ {seen.error}</span>}
                        {seen?.ok && (
                          <>
                            <span className="text-ok">
                              ✓ git repo · default branch {seen.default_branch ?? "?"} · {seen.branches.length} branch{seen.branches.length === 1 ? "" : "es"}
                            </span>
                            {seen.commits !== null && (
                              <span className="text-ok">
                                ✓ {seen.commits} commits on {source.branch.trim() || seen.default_branch}
                              </span>
                            )}
                            {seen.files.length > 0 && <span className="text-ok">✓ {seen.files.filter((f) => !f.startsWith(".")).slice(0, 10).join(" · ")}</span>}
                            {source.branch.trim() && !seen.branches.includes(source.branch.trim()) && <span className="text-warn">! no branch {source.branch.trim()} in this repo</span>}
                          </>
                        )}
                      </Inset>
                    )}
                  </>
                )}
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
                  <Field label="Slug" hint={`/git/${shownSlug || "…"}/code.git`}>
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
                    <option value="api">Anthropic API</option>
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
                    max_parallel <span className="text-[15px] font-semibold tracking-normal text-text">{defaults?.max_parallel ?? "?"}</span>{" "}
                    <span className="tracking-normal normal-case">for all projects, in map.toml</span>
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
                {step === 4 ? (busy ? (source.kind === "empty" ? "Creating…" : "Importing…") : "Create project") : "Continue"}
              </Btn>
            </div>
          </div>
        </div>
      </div>
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
