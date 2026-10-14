import clsx from "clsx";
import type { ButtonHTMLAttributes, ReactNode } from "react";

import type { Level } from "@/lib/generated/Level";
import type { SessionState } from "@/lib/generated/SessionState";
import type { TaskStatus } from "@/lib/generated/TaskStatus";

/** Small pieces of the command centre (design: Agent Command Centre.dc.html). */

export type Tone = "ok" | "warn" | "bad" | "info" | "idle" | "accent";

export const SESSION_TONE: Record<SessionState, Tone> = {
  starting: "info",
  working: "ok",
  idle: "idle",
  waiting_input: "accent",
  rate_limited: "warn",
  needs_auth: "bad",
  needs_trust: "warn",
  error: "bad",
  stalled: "warn",
  exited: "idle",
};

export const TASK_TONE: Record<TaskStatus, Tone> = { pending: "idle", running: "ok", verifying: "info", auditing: "info", landing: "info", review: "warn", done: "accent", failed: "bad", cancelled: "idle" };

export const LEVEL_TONE: Record<Level, Tone> = { info: "info", success: "ok", warning: "warn", error: "bad" };

const PILL: Record<Tone, string> = {
  ok: "text-ok bg-ok/10 border-ok/35",
  warn: "text-warn bg-warn/10 border-warn/35",
  bad: "text-bad bg-bad/10 border-bad/35",
  info: "text-info bg-info/10 border-info/35",
  idle: "text-idle bg-idle/10 border-idle/35",
  accent: "text-accent bg-accent/10 border-accent/35",
};

export const TONE_TEXT: Record<Tone, string> = { ok: "text-ok", warn: "text-warn", bad: "text-bad", info: "text-info", idle: "text-idle", accent: "text-accent" };
export const TONE_BG: Record<Tone, string> = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", info: "bg-info", idle: "bg-idle", accent: "bg-accent" };

/** Status badge: 18px, mono 9.5px spaced capitals, tinted by tone. */
export function Pill({ tone, children, small }: { tone: Tone; children: ReactNode; small?: boolean }) {
  return (
    <span
      className={clsx(
        "inline-flex flex-none items-center rounded border px-1.5 font-mono font-medium tracking-[0.08em] whitespace-nowrap uppercase",
        small ? "h-[17px] text-[9px]" : "h-[18px] text-[9.5px]",
        PILL[tone],
      )}
    >
      {children}
    </span>
  );
}

/** Column section heading: accent glyph, spaced capitals, a note or actions on the right. */
export function SideTitle({ glyph, children, note }: { glyph: ReactNode; children: ReactNode; note?: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-2.5 font-mono text-[11px] leading-none font-semibold tracking-[0.14em] text-text uppercase">
      <span className="flex items-center gap-2 whitespace-nowrap">
        <span className="text-[12px] text-accent">{glyph}</span>
        {children}
      </span>
      {note && <span className="flex min-w-0 items-center gap-2.5 font-normal tracking-[0.06em] whitespace-nowrap text-muted normal-case">{note}</span>}
    </div>
  );
}

type Kind = "primary" | "ghost" | "danger" | "accent-ghost";

/** The design's buttons: Geist 600, 7px radius; primary is solid accent. */
export function Btn({ kind = "ghost", size = "md", className, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { kind?: Kind; size?: "sm" | "md" | "lg" }) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex flex-none cursor-pointer items-center justify-center gap-1.5 border font-sans font-semibold whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-45",
        size === "sm" ? "h-6 rounded-[5px] px-2.5 text-[11px]" : size === "lg" ? "h-[34px] rounded-[7px] px-3.5 text-[12.5px]" : "h-[30px] rounded-[7px] px-3 text-[12px]",
        kind === "primary" && "border-accent bg-accent text-accent-text hover:opacity-90",
        kind === "ghost" && "border-line-3 bg-transparent text-text hover:border-text-2/50",
        kind === "danger" && "border-bad/40 bg-transparent text-bad hover:border-bad",
        kind === "accent-ghost" && "border-accent bg-accent-tint text-accent-soft",
        className,
      )}
    />
  );
}

/** A toggle chip of the forms and toolbars (provider, model, dependency…). */
export function Chip({ on, className, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { on: boolean }) {
  return (
    <button
      type="button"
      {...props}
      className={clsx(
        "inline-flex h-7 cursor-pointer items-center gap-1.5 rounded-md border px-[11px] font-mono text-[11.5px] font-medium whitespace-nowrap",
        on ? "border-accent bg-accent/15 text-accent-soft" : "border-line-2 bg-transparent text-nav hover:border-line-3",
        className,
      )}
    />
  );
}

export const hms = (iso: string) => new Date(iso).toTimeString().slice(0, 8);

export function rel(iso: string | null | undefined, now: number): string {
  if (!iso) return "";
  const d = Math.round((new Date(iso).getTime() - now) / 1000);
  const a = Math.abs(d);
  const t = a < 60 ? `${a}s` : a < 3600 ? `${Math.floor(a / 60)}m ${a % 60}s` : a < 86400 ? `${Math.floor(a / 3600)}h ${Math.floor((a % 3600) / 60)}m` : `${Math.floor(a / 86400)}d ${Math.floor((a % 86400) / 3600)}h`;
  return d >= 0 ? `in ${t}` : `${t} ago`;
}
