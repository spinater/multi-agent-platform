import clsx from "clsx";
import type { ReactNode } from "react";

/** The centred page column: max 1440px, 32/24/60 padding, 24px between blocks. */
export function Main({ children, className }: { children: ReactNode; className?: string }) {
  return <main className={clsx("mx-auto flex w-full max-w-[1440px] flex-col gap-6 px-4 pt-8 pb-16 sm:px-6", className)}>{children}</main>;
}

/** Kicker, H1 and lead paragraph, with actions on the right that wrap below on narrow screens. */
export function PageHeader({ kicker, title, lead, actions, children }: { kicker: ReactNode; title: ReactNode; lead?: ReactNode; actions?: ReactNode; children?: ReactNode }) {
  return (
    <header className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div className="min-w-0 max-w-[700px]">
          <p className="label">{kicker}</p>
          <h1 className="mt-2 text-[30px] leading-[1.05] font-semibold tracking-[-0.02em] sm:text-[38px]">{title}</h1>
          {lead && <p className="mt-3 text-sm leading-[1.55] text-text-2">{lead}</p>}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </header>
  );
}

/** A bordered card: 12px radius, faint surface, no shadow. */
export function Card({ children, className }: { children: ReactNode; className?: string }) {
  return <section className={clsx("min-w-0 rounded-xl border border-line bg-surface", className)}>{children}</section>;
}

/** A section title in the design's style: accent square, spaced capitals, note on the right. */
export function SectionTitle({ children, note, glyph = "■" }: { children: ReactNode; note?: ReactNode; glyph?: string }) {
  return (
    <div className="flex flex-wrap items-baseline justify-between gap-2">
      <h2 className="flex items-center gap-2 font-mono text-[11px] font-semibold tracking-[0.2em] text-text uppercase">
        <span className="text-[9px] text-accent">{glyph}</span>
        {children}
      </h2>
      {note && <span className="font-mono text-[10.5px] tracking-[0.08em] text-muted">{note}</span>}
    </div>
  );
}

/** Centred modal; closes on backdrop click. */
export function Modal({ children, onClose, width = 560, label }: { children: ReactNode; onClose: () => void; width?: number; label: string }) {
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 p-4 pt-[8vh]" onClick={onClose}>
      <div
        role="dialog"
        aria-label={label}
        className="w-full rounded-[14px] border border-line-2 bg-menu shadow-[0_20px_50px_rgba(0,0,0,.6)]"
        style={{ maxWidth: width }}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>
  );
}
