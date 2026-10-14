"use client";

/** Shared pieces of the Projects screen (design_handoff_map_dashboard/Projects.dc.html): form controls in the design's
 * style, the provider and checks editors used by both the wizard and a project's settings, the settings-as-TOML view,
 * and the per-project figures shown on cards. */

import clsx from "clsx";
import { ArrowDown, ArrowUp, Minus, Plus, X } from "lucide-react";
import { useEffect, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { OrchestratorStatus } from "@/lib/generated/OrchestratorStatus";
import type { ProjectDefaults } from "@/lib/generated/ProjectDefaults";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ProjectSettings } from "@/lib/generated/ProjectSettings";
import type { ProjectSource } from "@/lib/generated/ProjectSource";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { ReflexGates } from "@/lib/generated/ReflexGates";
import type { Session } from "@/lib/generated/Session";
import type { Task } from "@/lib/generated/Task";
import type { TaskStatus } from "@/lib/generated/TaskStatus";

export const errText = (e: unknown) => (e as Error).message;

// ---- controls

export function Field({ label, children, hint }: { label: ReactNode; children: ReactNode; hint?: ReactNode }) {
  return (
    <label className="flex flex-col gap-1.5 font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">
      {label}
      {children}
      {hint && <span className="text-[11px] font-normal tracking-normal normal-case">{hint}</span>}
    </label>
  );
}

export function TextInput({ mono = true, className, ...props }: InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }) {
  return (
    <input
      {...props}
      className={clsx(
        "h-[38px] w-full min-w-0 rounded-[7px] border border-line-2 bg-inset px-3 text-[13px] tracking-normal text-text normal-case outline-none placeholder:text-muted/70 focus:border-accent disabled:opacity-60",
        mono ? "font-mono" : "font-sans text-sm",
        className,
      )}
    />
  );
}

type Btn = ButtonHTMLAttributes<HTMLButtonElement> & { tone?: "primary" | "ghost" | "quiet" | "danger"; size?: "sm" | "md" };

export function Btn({ tone = "ghost", size = "md", className, ...props }: Btn) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex items-center justify-center gap-1.5 font-semibold whitespace-nowrap transition-colors disabled:cursor-not-allowed disabled:opacity-45",
        size === "md" ? "h-[34px] rounded-[7px] px-3.5 text-[12.5px]" : "h-7 rounded-md px-2.5 text-[11.5px] font-medium",
        tone === "primary" && "border border-accent bg-accent text-accent-text hover:opacity-90",
        tone === "ghost" && "border border-line-3 text-text hover:border-muted",
        tone === "quiet" && "border border-line-2 text-nav hover:text-text",
        tone === "danger" && "border border-bad/50 text-bad hover:border-bad",
        className,
      )}
    />
  );
}

/** A segmented control: the selected option is filled with the accent. */
export function Segmented<T extends string>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div className="flex self-start overflow-hidden rounded-lg border border-line-2">
      {options.map(([v, label]) => (
        <button
          key={v}
          type="button"
          onClick={() => onChange(v)}
          className={clsx("h-8 px-3.5 text-xs font-semibold whitespace-nowrap", v === value ? "bg-accent text-accent-text" : "text-nav hover:text-text")}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

export function Toggle({ on, onClick, disabled, label }: { on: boolean; onClick: () => void; disabled?: boolean; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className={clsx("relative h-5 w-[34px] shrink-0 rounded-full border p-0 disabled:cursor-not-allowed disabled:opacity-40", on ? "border-accent bg-accent" : "border-line-3 bg-white/[.08]")}
    >
      <span className={clsx("absolute top-[2px] size-3.5 rounded-full transition-[left]", on ? "left-[17px] bg-accent-text" : "left-[2px] bg-muted")} />
    </button>
  );
}

export function Check({ on, onClick, label, sub }: { on: boolean; onClick: () => void; label: ReactNode; sub?: ReactNode }) {
  return (
    <button type="button" onClick={onClick} className="flex items-start gap-2.5 p-0 text-left text-text">
      <span
        className={clsx(
          "mt-px flex size-4 shrink-0 items-center justify-center rounded border text-[11px] font-bold text-accent-text",
          on ? "border-accent bg-accent" : "border-white/25",
        )}
      >
        {on ? "✓" : ""}
      </span>
      <span className="flex flex-col gap-1">
        <span className="text-[13px] leading-tight font-medium">{label}</span>
        {sub && <span className="font-mono text-[11px] leading-snug text-muted">{sub}</span>}
      </span>
    </button>
  );
}

export function Inset({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={clsx("flex flex-col rounded-lg border border-line bg-inset px-3.5 py-3 font-mono text-[11.5px] leading-[1.6]", className)}>{children}</div>;
}

/** A provider's sign-in state as words, with the tone it is shown in. */
export function signInText(p: ProviderInfo | undefined): { text: string; tone: "ok" | "warn" | "bad" | "muted" } {
  if (!p) return { text: "not configured", tone: "bad" };
  if (!p.available) return { text: "not found on PATH", tone: "bad" };
  if (p.signed_in === true) return { text: "found · signed in", tone: "ok" };
  if (p.signed_in === false) return { text: "found · signed out", tone: "warn" };
  return { text: "found · sign-in unknown", tone: "muted" };
}

const toneText = { ok: "text-ok", warn: "text-warn", bad: "text-bad", muted: "text-muted" };

/** A provider chip: red with "· sign in" when signed out, which opens the sign-in modal. */
export function ProviderChip({ name, info, onSignIn }: { name: string; info: ProviderInfo | undefined; onSignIn: (name: string) => void }) {
  const out = info?.available && info.signed_in === false;
  const missing = !info?.available;
  return (
    <button
      type="button"
      onClick={() => out && onSignIn(name)}
      title={missing ? `${name} is not installed` : out ? `${name} is signed out` : info?.signed_in ? `${name} signed in` : `${name}: sign-in state unknown`}
      className={clsx(
        "flex h-[22px] items-center gap-1.5 rounded-[5px] border px-2 font-mono text-[10.5px] font-medium whitespace-nowrap",
        out ? "cursor-pointer border-bad/45 text-bad" : missing ? "cursor-default border-line text-muted" : "cursor-default border-line text-nav",
      )}
    >
      <span className="size-1.5 rounded-full" style={{ background: providerColor(name) }} />
      {name}
      {out ? " · sign in" : missing ? " · missing" : ""}
    </button>
  );
}

// ---- defaults from map.toml

let defaultsCache: ProjectDefaults | null = null;

/** The `[orchestrator]` values projects inherit; fetched once per page. */
export function useDefaults(): ProjectDefaults | null {
  const [d, setD] = useState<ProjectDefaults | null>(defaultsCache);
  useEffect(() => {
    if (defaultsCache) return;
    let stale = false;
    api.projectDefaults().then(
      (x) => {
        defaultsCache = x;
        if (!stale) setD(x);
      },
      () => {},
    );
    return () => {
      stale = true;
    };
  }, []);
  return d;
}

// ---- settings form

export type ProviderRow = { name: string; enabled: boolean; model: string; max: number | null };
export type AuditMode = "global" | "on" | "off";
export type SettingsForm = {
  providers: ProviderRow[];
  paused: boolean;
  /** `null`: inherit `orchestrator.verify`. */
  verify: string[] | null;
  audit: AuditMode;
  /** "" is the global audit chain. */
  auditor: string;
  gates: ReflexGates;
};

export function formFromSettings(s: ProjectSettings | null, providers: ProviderInfo[], defaults: ProjectDefaults | null): SettingsForm {
  const order = s && s.providers.length ? s.providers.map((p) => p.name) : (defaults?.provider_order ?? providers.map((p) => p.name));
  const rows: ProviderRow[] = order.map((name) => {
    const own = s?.providers.find((p) => p.name === name);
    return { name, enabled: true, model: own?.model ?? "", max: own?.max_parallel ?? null };
  });
  for (const p of providers) if (!order.includes(p.name)) rows.push({ name: p.name, enabled: false, model: "", max: null });
  return {
    providers: rows,
    paused: s?.paused ?? false,
    verify: s?.verify ?? null,
    audit: !s?.audit ? "global" : s.audit.enabled ? "on" : "off",
    auditor: s?.audit?.provider ?? "",
    gates: s?.reflex_gates ?? { audit: false, tier: false },
  };
}

/** An enabled list identical to `provider_order` with nothing customised is stored as "inherit". */
export function settingsFromForm(f: SettingsForm, defaults: ProjectDefaults | null, source: ProjectSource): ProjectSettings {
  const on = f.providers.filter((p) => p.enabled);
  const plain = on.every((p) => !p.model && p.max === null);
  const same = defaults !== null && plain && on.length === defaults.provider_order.length && on.every((p, i) => p.name === defaults.provider_order[i]);
  return {
    source,
    providers: same ? [] : on.map((p) => ({ name: p.name, model: p.model || null, max_parallel: p.max })),
    verify: f.verify,
    audit: f.audit === "global" ? null : { enabled: f.audit === "on", provider: f.audit === "on" && f.auditor ? f.auditor : null },
    paused: f.paused,
    reflex_gates: f.gates,
  };
}

export function ProvidersEditor({
  rows,
  onChange,
  providers,
  onSignIn,
}: {
  rows: ProviderRow[];
  onChange: (rows: ProviderRow[]) => void;
  providers: ProviderInfo[];
  onSignIn: (name: string) => void;
}) {
  const set = (i: number, patch: Partial<ProviderRow>) => onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  const move = (i: number, d: number) => {
    const j = i + d;
    if (j < 0 || j >= rows.length) return;
    const next = rows.slice();
    [next[i], next[j]] = [next[j], next[i]];
    onChange(next);
  };
  const enabled = rows.filter((r) => r.enabled).map((r) => r.name);
  return (
    <div className="flex flex-col overflow-hidden rounded-[10px] border border-line">
      {rows.map((r, i) => {
        const info = providers.find((p) => p.name === r.name);
        const state = signInText(info);
        const rank = enabled.indexOf(r.name);
        return (
          <div key={r.name} className={clsx("grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-2 border-b border-line px-3.5 py-3 last:border-b-0 sm:grid-cols-[auto_minmax(0,1fr)_auto]", !r.enabled && "opacity-55")}>
            <Toggle on={r.enabled} disabled={!info?.available && !r.enabled} onClick={() => set(i, { enabled: !r.enabled })} label={`Use ${r.name}`} />
            <span className="flex min-w-0 flex-col gap-1">
              <span className="flex flex-wrap items-center gap-2">
                <span className="size-[7px] rounded-full" style={{ background: providerColor(r.name) }} />
                <span className="text-[13.5px] font-semibold">{r.name}</span>
                <span className={clsx("font-mono text-[10.5px]", toneText[state.tone])}>{state.text}</span>
              </span>
              <span className="flex flex-wrap items-center gap-2 font-mono text-[10.5px] text-muted">
                model
                <select
                  value={r.model}
                  onChange={(e) => set(i, { model: e.target.value })}
                  disabled={!r.enabled}
                  className="h-6 max-w-full min-w-0 rounded border border-line-2 bg-inset px-1.5 font-mono text-[11px] text-text"
                  aria-label={`${r.name} model`}
                >
                  <option value="">default ({info?.default_model || "none"})</option>
                  {info?.models.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
                <span className="flex items-center gap-1 whitespace-nowrap">
                  <span className="mr-1">max sessions</span>
                  <button type="button" disabled={!r.enabled} className="flex size-6 items-center justify-center rounded border border-line-2 text-nav" onClick={() => set(i, { max: r.max === null ? null : r.max <= 1 ? null : r.max - 1 })} aria-label={`Fewer ${r.name} sessions`}>
                    <Minus className="size-3" />
                  </button>
                  <span className="min-w-12 text-center text-[11px] text-text">{r.max === null ? "no limit" : r.max}</span>
                  <button type="button" disabled={!r.enabled} className="flex size-6 items-center justify-center rounded border border-line-2 text-nav" onClick={() => set(i, { max: Math.min(16, (r.max ?? 0) + 1) })} aria-label={`More ${r.name} sessions`}>
                    <Plus className="size-3" />
                  </button>
                </span>
              </span>
            </span>
            <div className="col-span-2 flex items-center justify-end gap-1 sm:col-span-1">
              {info?.available && info.signed_in === false && (
                <button type="button" onClick={() => onSignIn(r.name)} className="mr-1 h-6 rounded-[5px] border border-accent bg-accent-tint px-2.5 text-[11px] font-semibold text-accent-soft">
                  Sign in
                </button>
              )}
              <span className="min-w-[22px] text-right font-mono text-[10.5px] font-medium text-accent-soft">{r.enabled ? `#${rank + 1}` : ""}</span>
              <button type="button" onClick={() => move(i, -1)} className="flex size-6 items-center justify-center rounded-[5px] border border-line-2 text-nav" aria-label={`Move ${r.name} up`}>
                <ArrowUp className="size-3" />
              </button>
              <button type="button" onClick={() => move(i, 1)} className="flex size-6 items-center justify-center rounded-[5px] border border-line-2 text-nav" aria-label={`Move ${r.name} down`}>
                <ArrowDown className="size-3" />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** Verify commands suggested by the files found at the top of the repo. */
export function suggestVerify(files: string[]): string[] {
  const has = (f: string) => files.includes(f);
  const out: string[] = [];
  if (has("package.json")) {
    const pm = has("bun.lock") || has("bun.lockb") ? "bun" : has("pnpm-lock.yaml") ? "pnpm" : has("yarn.lock") ? "yarn" : "npm";
    out.push(`${pm} test`, `${pm} run lint`, `${pm} run build`);
  }
  if (has("Cargo.toml")) out.push("cargo test", "cargo clippy --all-targets");
  if (has("go.mod")) out.push("go test ./...", "go vet ./...");
  if (has("pyproject.toml") || has("setup.py")) out.push("pytest");
  if (has("Makefile")) out.push("make test");
  return out;
}

export function ChecksEditor({
  form,
  onChange,
  defaults,
  providers,
  suggestions = [],
  domains,
}: {
  form: SettingsForm;
  onChange: (patch: Partial<SettingsForm>) => void;
  defaults: ProjectDefaults | null;
  providers: ProviderInfo[];
  suggestions?: string[];
  /** The corpus's Reflex domains; `undefined` for a project not created yet. */
  domains?: string[];
}) {
  const [draft, setDraft] = useState("");
  const own = form.verify !== null;
  const list = form.verify ?? defaults?.verify ?? [];
  const add = (cmd: string) => {
    if (!cmd.trim()) return;
    onChange({ verify: [...(form.verify ?? []), cmd.trim()] });
    setDraft("");
  };
  const chain = defaults?.audit_chain.join(" → ") ?? "";
  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-col gap-2">
        <span className="font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">verify</span>
        <Segmented
          value={own ? "own" : "global"}
          options={[
            ["global", `map.toml (${defaults?.verify.length ?? 0})`],
            ["own", "This project's"],
          ]}
          onChange={(v) => onChange({ verify: v === "own" ? [...(defaults?.verify ?? [])] : null })}
        />
        {list.map((c, i) => (
          <div key={`${i}-${c}`} className="flex h-[34px] items-center gap-2 rounded-[7px] border border-line bg-inset pr-1.5 pl-3">
            <span className="font-mono text-[10.5px] text-muted/70">{i + 1}</span>
            <span className="min-w-0 flex-1 truncate font-mono text-[12.5px]">{c}</span>
            {own && (
              <button type="button" onClick={() => onChange({ verify: list.filter((_, j) => j !== i) })} className="flex size-6 items-center justify-center text-muted hover:text-text" aria-label={`Remove ${c}`}>
                <X className="size-3.5" />
              </button>
            )}
          </div>
        ))}
        {!own && list.length === 0 && <p className="font-mono text-[11px] text-muted">map.toml has no verify commands: a task passes when the agent reports done.</p>}
        {own && (
          <>
            <input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  add(draft);
                }
              }}
              placeholder="add a command, Enter to add"
              className="h-[34px] rounded-[7px] border border-dashed border-line-3 bg-inset px-3 font-mono text-[12.5px] outline-none placeholder:text-muted/70 focus:border-accent"
            />
            <div className="flex flex-wrap gap-1.5">
              {suggestions
                .filter((s) => !list.includes(s))
                .map((s) => (
                  <button key={s} type="button" onClick={() => add(s)} className="h-6 rounded-[5px] border border-line-2 px-2 font-mono text-[11px] text-text-2 hover:text-text">
                    + {s}
                  </button>
                ))}
            </div>
          </>
        )}
      </div>
      <div className="flex flex-col gap-2.5">
        <span className="font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">audit</span>
        <Segmented
          value={form.audit}
          options={[
            ["global", `map.toml (${defaults?.audit_enabled ? "on" : "off"})`],
            ["on", "On"],
            ["off", "Off"],
          ]}
          onChange={(audit) => onChange({ audit })}
        />
        <p className="font-mono text-[11px] leading-snug text-muted">
          A second agent reviews the change and must answer VERDICT: PASS.{" "}
          {form.audit === "global" ? `map.toml: ${defaults?.audit_enabled ? `on, chain ${chain}` : "off"}.` : ""}
        </p>
        {form.audit === "on" && (
          <Field label="Auditor">
            <select value={form.auditor} onChange={(e) => onChange({ auditor: e.target.value })} className="h-[34px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[12.5px] tracking-normal text-text normal-case">
              <option value="">map.toml chain ({chain || "none"})</option>
              {providers.map((p) => (
                <option key={p.name} value={p.name} disabled={!p.available}>
                  {p.name}
                  {p.available ? "" : " (not installed)"}
                </option>
              ))}
            </select>
          </Field>
        )}
      </div>
      <GatesEditor gates={form.gates} onChange={(gates) => onChange({ gates })} defaults={defaults} domains={domains} />
      <p className="font-mono text-[11px] leading-snug text-muted">
        post_done hooks run for every project and are set in map.toml{defaults?.post_done.length ? `: ${defaults.post_done.join(" · ")}` : " (none)"}.
      </p>
    </div>
  );
}

/** The corpus domain the Reflex gates read; the server seeds `orchestration/gates.md` when a gate is turned on. */
export const GATES_DOMAIN = "orchestration";

/** The two Reflex gates: decisions the orchestrator asks the project's Reflex first, never an LLM. */
function GatesEditor({ gates, onChange, defaults, domains }: { gates: ReflexGates; onChange: (g: ReflexGates) => void; defaults: ProjectDefaults | null; domains?: string[] }) {
  const tiers = defaults?.tiers ?? [];
  const tierText = tiers.length
    ? tiers.map((t) => `${t.provider} ${[t.light, t.standard, t.heavy].map((m) => m ?? "–").join(" / ")}`).join(" · ")
    : "no provider has [orchestrator.tiers] in map.toml";
  const has = domains?.includes(GATES_DOMAIN);
  const corpus =
    domains === undefined
      ? `Both need an ${GATES_DOMAIN}/ domain in the corpus: turning one on adds a seed ${GATES_DOMAIN}/gates.md.`
      : has
        ? `✓ Both read the corpus's ${GATES_DOMAIN}/ domain.`
        : `Both need an ${GATES_DOMAIN}/ domain in the corpus: saving with a gate turned on adds a seed ${GATES_DOMAIN}/gates.md (unless the corpus already has 8 domains).`;
  return (
    <div className="flex flex-col gap-2.5">
      <span className="font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">reflex gates</span>
      <Check
        on={gates.audit}
        onClick={() => onChange({ ...gates, audit: !gates.audit })}
        label="Audit gate"
        sub="with the audit on, Reflex first decides if the change needs it; a confident no (docs, copy, tests only…) skips the audit call"
      />
      <Check
        on={gates.tier}
        onClick={() => onChange({ ...gates, tier: !gates.tier })}
        label="Model tier"
        sub={`Reflex rates a task without a pinned model light, standard or heavy, and it starts on that tier's model: ${tierText}`}
      />
      <p className={clsx("font-mono text-[11px] leading-snug", (gates.audit || gates.tier) && domains !== undefined && !has ? "text-warn" : "text-muted")}>
        {corpus} Reflex answers only at or above the project&apos;s threshold; unsure, abstaining or stopped, the orchestrator does what it does without the gate.
      </p>
    </div>
  );
}

// ---- settings as TOML

export type TomlLine = { text: string; kind: "comment" | "section" | "kv" | "blank" };

const q = (s: string) => JSON.stringify(s);
const arr = (a: string[]) => `[${a.map(q).join(", ")}]`;

/** The project's settings written out as TOML for review. They live in the platform's database, not in a file. */
export function tomlLines(
  p: { name: string; slug: string; description: string; fallback: string; model: string },
  s: ProjectSettings,
  defaults: ProjectDefaults | null,
): TomlLine[] {
  const L: TomlLine[] = [];
  const c = (text: string) => L.push({ text: `# ${text}`, kind: "comment" });
  const sec = (text: string) => L.push({ text, kind: "section" });
  const kv = (k: string, v: string) => L.push({ text: `${k} = ${v}`, kind: "kv" });
  const blank = () => L.push({ text: " ", kind: "blank" });
  c(`${p.name || "(unnamed)"} · project settings, stored in the platform database (not a file)`);
  sec("[project]");
  kv("slug", q(p.slug || "(from the name)"));
  kv("name", q(p.name));
  if (p.description) kv("description", q(p.description));
  kv("fallback", q(p.fallback));
  if (p.model) kv("model", q(p.model));
  blank();
  sec("[project.source]");
  kv("kind", q(s.source.kind));
  if (s.source.location) kv("location", q(s.source.location));
  if (s.source.kind === "empty") c("a new, empty folder; agents work in it directly");
  else if (s.source.kind === "local") c("an existing folder, used in place; the platform never copies or hosts the code");
  blank();
  sec("[project.orchestrator]");
  kv("paused", String(s.paused));
  if (s.providers.length) kv("provider_order", arr(s.providers.map((x) => x.name)));
  else c(`provider_order inherits map.toml: ${arr(defaults?.provider_order ?? [])}`);
  if (s.verify) kv("verify", arr(s.verify));
  else c(`verify inherits map.toml: ${arr(defaults?.verify ?? [])}`);
  blank();
  sec("[project.orchestrator.audit]");
  if (s.audit) {
    kv("enabled", String(s.audit.enabled));
    if (s.audit.enabled) kv("chain", arr(s.audit.provider ? [s.audit.provider] : (defaults?.audit_chain ?? [])));
  } else c(`inherits map.toml: enabled = ${defaults?.audit_enabled ?? false}, chain = ${arr(defaults?.audit_chain ?? [])}`);
  blank();
  sec("[project.orchestrator.reflex_gates]");
  kv("audit", String(s.reflex_gates.audit));
  kv("tier", String(s.reflex_gates.tier));
  if (s.reflex_gates.audit || s.reflex_gates.tier) c(`answered from the corpus's ${GATES_DOMAIN}/ domain; tier models from map.toml [orchestrator.tiers]`);
  for (const x of s.providers.filter((x) => x.model || x.max_parallel !== null)) {
    blank();
    sec(`[project.providers.${x.name}]`);
    if (x.model) kv("model", q(x.model));
    if (x.max_parallel !== null) kv("max_parallel", String(x.max_parallel));
  }
  blank();
  c("global, from map.toml (read-only here)");
  c(`workspace.session_prefix = ${q(defaults?.session_prefix ?? "")}`);
  c(`orchestrator.max_parallel = ${defaults?.max_parallel ?? "?"}`);
  if (defaults?.post_done.length) c(`orchestrator.post_done = ${arr(defaults.post_done)}`);
  return L;
}

export function TomlView({ lines, className }: { lines: TomlLine[]; className?: string }) {
  return (
    <pre className={clsx("max-h-[380px] overflow-auto rounded-[10px] border border-line bg-inset px-4 py-3.5 font-mono text-xs leading-[1.65] whitespace-pre-wrap break-words", className)}>
      {lines.map((l, i) => {
        if (l.kind !== "kv")
          return (
            <span key={i} className={clsx("block", l.kind === "section" ? "text-text" : "text-muted")}>
              {l.text}
            </span>
          );
        const eq = l.text.indexOf(" = ");
        return (
          <span key={i} className="block">
            <span className="text-nav">{l.text.slice(0, eq)}</span>
            <span className="text-muted"> = </span>
            <span className="text-accent-soft">{l.text.slice(eq + 3)}</span>
          </span>
        );
      })}
    </pre>
  );
}

// ---- figures for a project card

export const SEGMENTS: [TaskStatus, string, string][] = [
  ["done", "var(--accent)", "done"],
  ["running", "var(--ok)", "running"],
  ["verifying", "var(--info)", "verifying"],
  ["auditing", "var(--area-8)", "auditing"],
  ["pending", "#5d6673", "pending"],
  ["failed", "var(--bad)", "failed"],
];

export type ProjectStatus = "running" | "paused" | "idle";

export type ProjectFigures = {
  counts: Partial<Record<TaskStatus, number>>;
  total: number;
  live: number;
  /** Sessions waiting for input, signed out, stalled or in error, plus failed tasks. */
  attention: number;
  last: number | null;
  status: ProjectStatus;
};

/** Figures for one project; `slug === null` is the default workspace. */
export function figures(slug: string | null, info: ProjectInfo | null, tasks: Task[], all: Session[], orchestrator: OrchestratorStatus | null): ProjectFigures {
  // Sign-in helpers (`<prefix>login-<provider>`) belong to no project's work.
  const sessions = all.filter((s) => !(s.role === "manual" && s.name.endsWith(`login-${s.provider}`)));
  const own = tasks.filter((t) => (t.project ?? null) === slug && t.status !== "cancelled");
  const live = sessions.filter((s) => (s.project ?? null) === slug && s.state !== "exited");
  const counts: Partial<Record<TaskStatus, number>> = {};
  for (const t of own) counts[t.status] = (counts[t.status] ?? 0) + 1;
  const attention = live.filter((s) => ["waiting_input", "needs_auth", "stalled", "error"].includes(s.state)).length + (counts.failed ?? 0);
  const times = [...own.map((t) => t.updated_at), ...sessions.filter((s) => (s.project ?? null) === slug).map((s) => s.last_activity_at)].map((x) => new Date(x).getTime());
  const busy = (counts.running ?? 0) + (counts.verifying ?? 0) + (counts.auditing ?? 0) > 0;
  const status: ProjectStatus = info?.project.settings.paused ? "paused" : orchestrator?.running && busy ? "running" : "idle";
  return { counts, total: own.length, live: live.length, attention, last: times.length ? Math.max(...times) : null, status };
}

export function ago(t: number | null, now: number): string {
  if (t === null) return "no activity yet";
  const s = Math.max(0, Math.round((now - t) / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
}

export const STATUS_TONE: Record<ProjectStatus | "initializing", { color: string; className: string }> = {
  running: { color: "var(--ok)", className: "text-ok border-ok/35 bg-ok/10" },
  paused: { color: "var(--warn)", className: "text-warn border-warn/35 bg-warn/10" },
  idle: { color: "#5d6673", className: "text-idle border-idle/35 bg-idle/10" },
  initializing: { color: "var(--info)", className: "text-info border-info/35 bg-info/10" },
};

export function StatusPill({ status }: { status: ProjectStatus | "initializing" }) {
  return (
    <span className={clsx("inline-flex h-5 shrink-0 items-center rounded border px-[7px] font-mono text-[9.5px] font-medium tracking-[0.08em] whitespace-nowrap uppercase", STATUS_TONE[status].className)}>
      {status}
    </span>
  );
}

export function TaskBar({ f }: { f: ProjectFigures }) {
  const segs = SEGMENTS.filter(([k]) => f.counts[k]).map(([k, color, label]) => ({ n: f.counts[k]!, color, label }));
  return (
    <div className="flex flex-col gap-2">
      <div className="flex justify-between gap-2.5 font-mono text-[10.5px] text-muted">
        <span>TASKS</span>
        <span className="text-nav">{f.total ? `${f.counts.done ?? 0} / ${f.total} done` : "no tasks yet"}</span>
      </div>
      <div className="flex h-1.5 gap-0.5 overflow-hidden rounded-[3px] bg-white/5">
        {segs.map((s) => (
          <span key={s.label} style={{ flex: `${s.n} 1 0`, background: s.color }} />
        ))}
      </div>
      {segs.length > 0 && (
        <div className="flex flex-wrap gap-x-3 gap-y-1 font-mono text-[10.5px] text-muted">
          {segs.map((s) => (
            <span key={s.label} className="flex items-center gap-1.5 whitespace-nowrap">
              <span className="size-1.5 rounded-[2px]" style={{ background: s.color }} />
              {s.n} {s.label}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/** The project's provider list in order: its own, else map.toml's `provider_order`. */
export function projectProviders(info: ProjectInfo | null, defaults: ProjectDefaults | null, providers: ProviderInfo[]): string[] {
  if (info?.project.settings.providers.length) return info.project.settings.providers.map((p) => p.name);
  return defaults?.provider_order ?? providers.map((p) => p.name);
}
