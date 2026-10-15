"use client";

/** The Configure screen's small pieces, in the handoff design's style (Configure.dc.html): the source badge, the
 * switch, chips, steppers, radio cards, cards and labels. Colours come from the theme tokens. */

import clsx from "clsx";
import { createContext, useContext, type ReactNode } from "react";

import type { ConfigSource } from "@/lib/generated/ConfigSource";
import { TAB_DESC_TH, useLang, useT } from "@/lib/i18n";

export type Scope = "project" | "machine";

/** What a tab has not saved yet: the save bar adds these up over every tab. */
export type Pending = { count: number; files: string[]; save: () => Promise<void>; discard: () => void };

export type ConfigureCtx = {
  scope: Scope;
  /** The project in project scope; `null` in machine scope or for the default workspace. */
  slug: string | null;
  setScope: (s: Scope) => void;
  /** A tab reports its unsaved edits (`null`: none). */
  setPending: (tab: string, p: Pending | null) => void;
  flash: (msg: string, tone?: "ok" | "warn" | "bad") => void;
  /** The config language (`[configure] language`): what agents write in the "Ask an agent" panels. */
  cfgLang: "en" | "th";
};

export const Ctx = createContext<ConfigureCtx | null>(null);

export function useConfigure(): ConfigureCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useConfigure outside the Configure screen");
  return c;
}

const SRC: Record<ConfigSource | "read_only", [string, string]> = {
  project: ["var(--accent-soft)", "color-mix(in srgb, var(--accent) 35%, transparent)"],
  machine: ["var(--text-2)", "var(--line-3)"],
  override: ["var(--warn)", "color-mix(in srgb, var(--warn) 40%, transparent)"],
  read_only: ["var(--muted)", "var(--line-2)"],
};

/** `machine`, `project` or `override`: where the item's value comes from. */
export function SourceBadge({ source, label }: { source: ConfigSource | "read_only"; label?: string }) {
  const [color, border] = SRC[source];
  return (
    <span
      className="inline-flex flex-none items-center rounded-[3px] border px-[5px] py-[2px] font-mono text-[9px] leading-none font-medium tracking-[0.08em] whitespace-nowrap uppercase"
      style={{ color, borderColor: border }}
    >
      {label ?? (source === "read_only" ? "map.toml" : source)}
    </span>
  );
}

/** The 32×18 on/off switch. */
export function Switch({ on, onClick, disabled, title }: { on: boolean; onClick: () => void; disabled?: boolean; title?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      title={title}
      disabled={disabled}
      onClick={onClick}
      className="relative h-[18px] w-8 flex-none cursor-pointer rounded-[9px] disabled:cursor-not-allowed disabled:opacity-50"
      style={{ background: on ? "var(--accent)" : "color-mix(in srgb, var(--text) 14%, transparent)" }}
    >
      <span className="absolute top-[2px] size-[14px] rounded-full bg-[#e6e9ef] transition-[left]" style={{ left: on ? 16 : 2 }} />
    </button>
  );
}

/** A ✓ / + chip that toggles membership (skills, tools, readers). */
export function Chip({ on, onClick, children, title, muted, disabled }: { on: boolean; onClick?: () => void; children: ReactNode; title?: string; muted?: boolean; disabled?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      disabled={disabled}
      className="flex h-7 cursor-pointer items-center gap-1.5 rounded-md border px-2.5 font-mono text-[11.5px] font-medium whitespace-nowrap disabled:cursor-default"
      style={{
        borderColor: on ? "var(--accent)" : "var(--line-2)",
        background: on ? "var(--accent-tint)" : "transparent",
        color: muted ? "var(--muted)" : on ? "var(--accent-soft)" : "var(--text-2)",
      }}
    >
      <span>{on ? "✓" : "+"}</span>
      {children}
    </button>
  );
}

/** One-of buttons in a bordered strip (model, effort, land mode). */
export function Seg<T extends string>({ value, options, onChange, size = "sm" }: { value: T; options: (T | [T, string])[]; onChange: (v: T) => void; size?: "sm" | "md" }) {
  return (
    <div className="flex w-max max-w-full flex-wrap gap-0.5 rounded-[7px] border border-line-2 p-0.5">
      {options.map((o) => {
        const [v, l] = Array.isArray(o) ? o : [o, o];
        const on = v === value;
        return (
          <button
            key={v}
            type="button"
            onClick={() => onChange(v)}
            className={clsx("cursor-pointer rounded-[5px] px-2.5 font-mono font-medium whitespace-nowrap", size === "sm" ? "h-[22px] text-[11px]" : "h-[26px] text-[11.5px]")}
            style={{ background: on ? "color-mix(in srgb, var(--text) 8%, transparent)" : "transparent", color: on ? "var(--text)" : "var(--muted)" }}
          >
            {l}
          </button>
        );
      })}
    </div>
  );
}

/** Bordered option buttons where the chosen one is orange (models). */
export function Pick<T extends string>({ value, options, onChange }: { value: T | null; options: T[]; onChange: (v: T) => void }) {
  return (
    <div className="flex flex-wrap gap-1">
      {options.map((m) => {
        const on = m === value;
        return (
          <button
            key={m}
            type="button"
            onClick={() => onChange(m)}
            className="h-6 cursor-pointer rounded-[5px] border px-2.5 font-mono text-[11px] font-medium"
            style={{ borderColor: on ? "var(--accent)" : "var(--line-2)", background: on ? "var(--accent-tint)" : "transparent", color: on ? "var(--accent-soft)" : "var(--text-2)" }}
          >
            {m}
          </button>
        );
      })}
    </div>
  );
}

/** − N + */
export function Stepper({ value, onMinus, onPlus, width = 40 }: { value: ReactNode; onMinus: () => void; onPlus: () => void; width?: number }) {
  return (
    <div className="flex w-max overflow-hidden rounded-[7px] border border-line-2">
      <button type="button" onClick={onMinus} className="h-8 w-8 cursor-pointer text-[15px] text-text" aria-label="Less">
        −
      </button>
      <span className="flex items-center justify-center font-mono text-[13px] font-semibold" style={{ width }}>
        {value}
      </span>
      <button type="button" onClick={onPlus} className="h-8 w-8 cursor-pointer text-[15px] text-text" aria-label="More">
        +
      </button>
    </div>
  );
}

/** A radio card: ring + dot, title and description. */
export function RadioCard({ on, title, desc, onClick, disabled }: { on: boolean; title: ReactNode; desc: ReactNode; onClick: () => void; disabled?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="flex cursor-pointer items-start gap-2.5 rounded-[10px] border p-3 text-left text-text disabled:cursor-default"
      style={{ borderColor: on ? "var(--accent)" : "var(--line-2)", background: on ? "color-mix(in srgb, var(--accent) 6%, transparent)" : "transparent" }}
    >
      <span className="mt-px flex size-[14px] flex-none items-center justify-center rounded-full border" style={{ borderColor: on ? "var(--accent)" : "var(--line-3)" }}>
        <span className="size-1.5 rounded-full" style={{ background: on ? "var(--accent)" : "transparent" }} />
      </span>
      <span className="flex flex-col gap-1">
        <span className="text-[13px] leading-[1.2] font-semibold">{title}</span>
        <span className="text-[11.5px] leading-[1.45] text-text-2">{desc}</span>
      </span>
    </button>
  );
}

/** The bordered card every tab body is made of. */
export function Box({ children, className, dashed }: { children: ReactNode; className?: string; dashed?: boolean }) {
  return (
    <div className={clsx("flex min-w-0 flex-col gap-3 rounded-xl border p-4", dashed ? "border-dashed border-line-3" : "border-line bg-surface", className)}>{children}</div>
  );
}

/** The spaced-capitals label above a field. */
export function Lbl({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={clsx("font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase", className)}>{children}</span>;
}

export const inputCls = "h-8 min-w-0 rounded-[7px] border border-line-2 bg-inset px-2.5 font-mono text-[12px] text-text outline-none focus:border-accent";
export const btnCls = "h-[30px] cursor-pointer rounded-[7px] border border-line-3 bg-transparent px-3 text-[12px] font-medium whitespace-nowrap text-nav hover:text-text disabled:cursor-default disabled:opacity-50";
export const primaryCls = "h-8 cursor-pointer rounded-[7px] border border-accent bg-accent px-3.5 text-[12.5px] font-semibold whitespace-nowrap text-accent-text disabled:opacity-50";
export const dangerCls =
  "h-[30px] cursor-pointer rounded-md border border-[color-mix(in_srgb,var(--bad)_35%,transparent)] bg-transparent px-3 text-[12px] font-medium whitespace-nowrap text-bad";

/** A box saying the tab needs something first (a project, a backend that is not there yet). */
export function Note({ children, tone = "muted" }: { children: ReactNode; tone?: "muted" | "warn" | "bad" }) {
  const c = tone === "warn" ? "var(--warn)" : tone === "bad" ? "var(--bad)" : "var(--line-2)";
  return (
    <div className="rounded-[10px] border px-3.5 py-3 text-[12.5px] leading-normal text-text-2" style={{ borderColor: tone === "muted" ? c : `color-mix(in srgb, ${c} 45%, transparent)` }}>
      {children}
    </div>
  );
}

/** Same JSON: for dirty checks of drafts against what was loaded. */
export const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** A tab's header: title, one-line description, where it saves, and its add action. */
export function TabHead({ title, desc, savesTo, add }: { title: string; desc: ReactNode; savesTo: ReactNode; add?: { label: string; onClick: () => void; disabled?: boolean } }) {
  const lang = useLang();
  const t = useT();
  return (
    <div className="flex flex-wrap items-end justify-between gap-3 border-b border-line pb-3.5">
      <div className="flex max-w-[640px] flex-col gap-[7px]">
        <h2 className="text-[22px] leading-[1.15] font-semibold tracking-[-0.01em]">{title}</h2>
        <p className="text-[13px] leading-normal text-text-2">{lang === "th" && TAB_DESC_TH[title] ? TAB_DESC_TH[title] : desc}</p>
        <span className="font-mono text-[10.5px] leading-[1.3] text-muted">
          {t("cfg.savesTo")} {savesTo}
        </span>
      </div>
      {add && (
        <button onClick={add.onClick} disabled={add.disabled} className={primaryCls}>
          {add.label}
        </button>
      )}
    </div>
  );
}
