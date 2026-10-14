import clsx from "clsx";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";

import type { SessionState } from "@/lib/generated/SessionState";
import type { TaskStatus } from "@/lib/generated/TaskStatus";

export function Panel({ title, actions, children, className }: { title: ReactNode; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={clsx("flex min-w-0 flex-col rounded-md border border-line bg-panel", className)}>
      <header className="flex items-center justify-between gap-2 border-b border-line px-4 py-2">
        <h2 className="label">{title}</h2>
        <div className="flex items-center gap-1.5">{actions}</div>
      </header>
      <div className="min-h-0 flex-1">{children}</div>
    </section>
  );
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "ghost" | "danger"; size?: "sm" | "md" };

export function Button({ variant = "ghost", size = "md", className, ...props }: ButtonProps) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex items-center justify-center gap-1.5 rounded font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent",
        size === "sm" ? "h-6 px-2 text-[11px]" : "h-8 px-3 text-xs",
        variant === "primary" && "bg-accent text-accent-text hover:opacity-90",
        variant === "ghost" && "border border-line bg-panel-2 text-text hover:border-muted",
        variant === "danger" && "border border-line bg-panel-2 text-bad hover:border-bad",
        className,
      )}
    />
  );
}

const field = "w-full rounded border border-line bg-panel-2 px-2.5 text-[13px] text-text placeholder:text-muted focus:border-accent focus:outline-none";

export function Input(props: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={clsx(field, "h-8", props.className)} />;
}

export function Textarea(props: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea {...props} className={clsx(field, "py-2 font-mono text-[13px] leading-5", props.className)} />;
}

export function Select(props: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...props} className={clsx(field, "h-8", props.className)} />;
}

export function Label({ text, children }: { text: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1 text-[11px] font-medium tracking-wide text-muted uppercase">
      {text}
      {children}
    </label>
  );
}

type Tone = "ok" | "warn" | "bad" | "info" | "idle" | "accent";

const toneClass: Record<Tone, string> = {
  ok: "text-ok border-ok/40 bg-ok/10",
  warn: "text-warn border-warn/40 bg-warn/10",
  bad: "text-bad border-bad/40 bg-bad/10",
  info: "text-info border-info/40 bg-info/10",
  idle: "text-idle border-idle/40 bg-idle/10",
  accent: "text-accent border-accent/40 bg-accent/10",
};

export function Badge({ tone = "idle", children, pulse }: { tone?: Tone; children: ReactNode; pulse?: boolean }) {
  return (
    <span className={clsx("inline-flex shrink-0 items-center gap-1.5 rounded border px-1.5 py-0.5 text-[11px] font-medium", toneClass[tone])}>
      {pulse && <span className="size-1.5 animate-pulse rounded-full bg-current" />}
      {children}
    </span>
  );
}

const sessionTone: Record<SessionState, Tone> = {
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

export function SessionBadge({ state }: { state: SessionState }) {
  return (
    <Badge tone={sessionTone[state]} pulse={state === "working" || state === "starting"}>
      {state.replace("_", " ")}
    </Badge>
  );
}

const taskTone: Record<TaskStatus, Tone> = {
  pending: "idle",
  running: "ok",
  verifying: "info",
  auditing: "info",
  done: "accent",
  failed: "bad",
  cancelled: "idle",
};

export function TaskBadge({ status }: { status: TaskStatus }) {
  return (
    <Badge tone={taskTone[status]} pulse={status === "running" || status === "verifying" || status === "auditing"}>
      {status}
    </Badge>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="px-4 py-8 text-center text-sm text-muted">{children}</p>;
}

export function relative(iso: string | null | undefined, now: number): string {
  if (!iso) return "";
  const diff = Math.round((new Date(iso).getTime() - now) / 1000);
  const abs = Math.abs(diff);
  const text = abs < 60 ? `${abs}s` : abs < 3600 ? `${Math.floor(abs / 60)}m ${abs % 60}s` : `${Math.floor(abs / 3600)}h ${Math.floor((abs % 3600) / 60)}m`;
  return diff >= 0 ? `in ${text}` : `${text} ago`;
}

/** A titled block of a side rail: spaced-capital title, optional note, hairline rule below. */
export function Section({ title, note, actions, children, className }: { title: ReactNode; note?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={clsx("border-b border-line px-4 py-3.5", className)}>
      <header className="mb-2 flex items-center justify-between gap-2">
        <h2 className="label text-text">{title}</h2>
        {actions}
      </header>
      {note && <p className="-mt-1 mb-2 text-[11px] text-muted">{note}</p>}
      {children}
    </section>
  );
}

/** A thin progress bar for "n of m" rows. */
export function Meter({ value, max, tone = "accent" }: { value: number; max: number; tone?: "accent" | "ok" | "bad" | "info" }) {
  const pct = max > 0 ? Math.min(100, (100 * value) / max) : 0;
  const color = { accent: "bg-accent", ok: "bg-ok", bad: "bg-bad", info: "bg-info" }[tone];
  return (
    <span className="block h-1 w-full overflow-hidden rounded-full bg-panel-2">
      <span className={clsx("block h-full rounded-full", color)} style={{ width: `${pct}%` }} />
    </span>
  );
}

/** A dot that reads as the state's colour; always shown next to the state's name. */
export function Dot({ tone }: { tone: Tone }) {
  const color = { ok: "bg-ok", warn: "bg-warn", bad: "bg-bad", info: "bg-info", idle: "bg-idle", accent: "bg-accent" }[tone];
  return <span className={clsx("inline-block size-1.5 shrink-0 rounded-full", color)} />;
}

export type { Tone };
