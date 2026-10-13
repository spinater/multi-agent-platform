import clsx from "clsx";
import type { ButtonHTMLAttributes, CSSProperties, ReactNode } from "react";

import type { Lane } from "@/lib/generated/Lane";

import { LANE_COLOR, mix, type Axis, pctPos, ticks } from "./lib";

/** A numbered section card of layout A: `01  How it works`, meta on the right. */
export function RxSection({ id, n, title, meta, children, className }: { id: string; n: string; title: string; meta?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section id={id} className={clsx("flex min-w-0 flex-col gap-3.5 rounded-xl border border-r-line bg-r-panel p-4 sm:p-[18px]", className)}>
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1.5">
        <h2 className="text-[11px] leading-tight font-semibold tracking-[0.16em] whitespace-nowrap uppercase">
          <span className="text-r-accent">{n}</span>&nbsp;&nbsp;{title}
        </h2>
        {meta && <span className="text-[10.5px] leading-tight text-r-muted">{meta}</span>}
      </div>
      {children}
    </section>
  );
}

/** Small spaced capitals. */
export function Cap({ children, className, style }: { children: ReactNode; className?: string; style?: CSSProperties }) {
  return (
    <span className={clsx("text-[10px] leading-tight font-medium tracking-[0.12em] text-r-muted uppercase", className)} style={style}>
      {children}
    </span>
  );
}

type BtnProps = ButtonHTMLAttributes<HTMLButtonElement> & { tone?: "primary" | "ghost"; size?: "sm" | "md" | "lg" };

export function RxButton({ tone = "ghost", size = "md", className, ...props }: BtnProps) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-md border font-semibold tracking-[0.04em] whitespace-nowrap transition-opacity",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-r-accent disabled:cursor-not-allowed disabled:opacity-40",
        size === "sm" ? "h-6 px-2.5 text-[10.5px]" : size === "lg" ? "h-[34px] px-4 text-[12px]" : "h-[30px] px-3 text-[11px]",
        tone === "primary" ? "border-r-accent bg-r-accent text-r-accent-ink hover:opacity-90" : "border-r-line2 bg-transparent text-r-text hover:border-r-muted",
        className,
      )}
    />
  );
}

/** A selectable chip (question picker, log filter). */
export function Chip({ active, color = "var(--r-accent)", children, onClick, className }: { active: boolean; color?: string; children: ReactNode; onClick: () => void; className?: string }) {
  return (
    <button
      onClick={onClick}
      aria-pressed={active}
      className={clsx("h-7 rounded-md border px-2.5 text-[11px] font-medium whitespace-nowrap", active ? "text-r-text" : "text-r-muted hover:text-r-text", className)}
      style={{ borderColor: active ? color : "var(--r-line)", background: active ? mix(color, 12) : "transparent" }}
    >
      {children}
    </button>
  );
}

export function LaneBadge({ lane }: { lane: Lane }) {
  const c = LANE_COLOR[lane];
  return (
    <span className="inline-flex h-[22px] items-center gap-[7px] rounded-[5px] border px-2 text-[10px] font-semibold tracking-[0.14em] whitespace-nowrap uppercase" style={{ borderColor: c, color: c }}>
      <span className="size-1.5 rounded-full" style={{ background: c }} />
      lane · {lane}
    </span>
  );
}

export function LaneLabel({ lane }: { lane: Lane }) {
  const c = LANE_COLOR[lane];
  return (
    <span className="inline-flex items-center gap-1.5 text-[9.5px] font-semibold tracking-[0.12em] uppercase" style={{ color: c }}>
      <span className="size-1.5 rounded-full" style={{ background: c }} />
      {lane}
    </span>
  );
}

/** The mode badge, top right of the page. */
export function ModeBadge({ label, color }: { label: string; color: string }) {
  return (
    <span className="inline-flex h-7 items-center gap-2 rounded-md border px-[11px] text-[10.5px] font-semibold tracking-[0.16em] whitespace-nowrap uppercase" style={{ borderColor: color, color }}>
      <span className="size-[7px] rounded-full" style={{ background: color }} />
      {label}
    </span>
  );
}

/** A state banner with a call to action. */
export function Banner({ title, text, color, actions }: { title: ReactNode; text: ReactNode; color: string; actions?: ReactNode }) {
  return (
    <div role="status" className="flex flex-wrap items-center justify-between gap-3 rounded-[10px] border px-3.5 py-3" style={{ borderColor: mix(color, 45), background: mix(color, 6) }}>
      <span className="flex max-w-[860px] min-w-0 flex-col gap-[5px]">
        <span className="text-[12.5px] leading-snug font-semibold">{title}</span>
        <span className="text-[11.5px] leading-[1.55] break-words text-r-muted">{text}</span>
      </span>
      {actions && <div className="flex flex-wrap gap-2">{actions}</div>}
    </div>
  );
}

/** A figure tile: label, big value, note. */
export function Tile({ k, v, color, vcolor, sub, dot }: { k: string; v: ReactNode; color?: string; vcolor?: string; sub: ReactNode; dot?: boolean }) {
  return (
    <div className="flex min-w-0 flex-col gap-[7px] rounded-lg border border-r-line px-3 py-2.5">
      <span className="flex items-center gap-1.5 text-[9.5px] leading-tight font-medium tracking-[0.14em] text-r-muted uppercase">
        {dot && <span className="size-1.5 rounded-full" style={{ background: color }} />}
        {k}
      </span>
      <span className="text-[20px] leading-none font-semibold" style={{ color: vcolor ?? color }}>
        {v}
      </span>
      <span className="text-[10px] leading-snug text-r-faint">{sub}</span>
    </div>
  );
}

/** Tick labels under a log axis. */
export function AxisTicks({ axis }: { axis: Axis }) {
  const all = ticks(axis);
  // More than two decades: label only the decades, so the labels do not collide.
  const shown = axis.hi - axis.lo > 2 ? all.filter((t) => t.label.startsWith("1e")) : all;
  return (
    <div className="relative h-3 text-[9.5px] leading-none text-r-faint" aria-hidden>
      {shown.map((t, i) => (
        <span
          key={t.label}
          className="absolute whitespace-nowrap"
          style={i === 0 ? { left: 0 } : i === shown.length - 1 ? { right: 0 } : { left: pctPos(t.at), transform: "translateX(-50%)" }}
        >
          {t.label}
        </span>
      ))}
    </div>
  );
}

/** A thin bar: track plus fill. */
export function Bar({ width, color, className }: { width: string; color: string; className?: string }) {
  return (
    <span className={clsx("block h-1 overflow-hidden rounded-sm bg-r-line", className)}>
      <span className="block h-full" style={{ width, background: color }} />
    </span>
  );
}

export const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));
