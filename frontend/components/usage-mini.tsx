"use client";

import { RotateCw } from "lucide-react";
import { useRef, useState } from "react";
import { createPortal } from "react-dom";

import { providerColor } from "@/lib/colors";
import type { ProviderUsage } from "@/lib/generated/ProviderUsage";
import { badgeOf, byLength, dur, headline, meterColor, providerName, useUsage, windowValue, worstOf } from "@/lib/use-usage";

import { useApp } from "./app-context";

const PINNED = "map.usage.pinned";
const DOCK_OPEN = "map.usage.dockOpen";
const WIDTH = 360;

function stored(key: string, fallback: boolean): boolean {
  try {
    const v = localStorage.getItem(key);
    return v === null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

function store(key: string, value: boolean) {
  try {
    localStorage.setItem(key, value ? "1" : "0");
  } catch {
    // Private mode: the choice lasts for this page.
  }
}

const BADGE: Record<"ok" | "warn" | "bad" | "idle", string> = {
  ok: "text-ok border-ok/35",
  warn: "text-warn border-warn/45",
  bad: "text-bad border-bad/50",
  idle: "text-muted border-line-2",
};

function Countdown({ at, now }: { at: string | null; now: number }) {
  if (!at) return null;
  const ms = new Date(at).getTime() - now;
  return (
    <span className="flex items-center gap-[3px] font-mono text-[9.5px] leading-none whitespace-nowrap text-muted">
      <RotateCw className="size-[8px]" strokeWidth={2.5} />
      {ms > 0 ? dur(ms, false) : "reset"}
    </span>
  );
}

function PopoverRow({ p, skip, now }: { p: ProviderUsage; skip: number; now: number }) {
  const color = providerColor(p.provider);
  const badge = badgeOf(p, skip);
  return (
    <div className="flex flex-col gap-2 border-b border-line pb-2.5">
      <div className="flex items-center gap-2">
        <span className="size-[7px] rounded-full" style={{ background: color }} />
        <span className="text-[12.5px] leading-none font-semibold">{providerName(p.provider, true)}</span>
        {p.plan && <span className="font-mono text-[10.5px] leading-none text-muted">{p.plan}</span>}
        <span
          className={`ml-auto inline-flex h-[17px] items-center rounded border px-1.5 font-mono text-[9px] leading-none font-medium tracking-[0.08em] uppercase ${BADGE[badge.tone]}`}
        >
          {badge.text}
        </span>
      </div>
      {p.windows.map((w) => {
        const c = meterColor(w.used_pct, skip, color);
        return (
          <div key={w.id} className="grid grid-cols-[96px_minmax(0,1fr)_62px] items-center gap-2" title={w.note ?? w.label}>
            <span className="truncate font-mono text-[10.5px] leading-[1.2] whitespace-nowrap text-text-2">{w.short}</span>
            <span className="relative h-1.5 overflow-hidden rounded-[3px] bg-line">
              <span className="absolute inset-y-0 left-0 rounded-[3px]" style={{ width: `${Math.min(100, w.used_pct ?? 0)}%`, background: c }} />
              <span className="absolute inset-y-0 w-px bg-text opacity-60" style={{ left: `${skip}%` }} />
            </span>
            <span className="flex flex-col items-end gap-[3px]">
              <span className="font-mono text-[11.5px] leading-none font-semibold" style={{ color: c }}>
                {windowValue(w, true)}
              </span>
              <Countdown at={w.resets_at} now={now} />
            </span>
          </div>
        );
      })}
      {p.windows.length > 0 && (p.status === "snapshot" || p.status === "signed_out") && p.read_at && (
        <span className="font-mono text-[10px] leading-none text-muted">
          {p.status === "signed_out" ? "signed out · " : ""}snapshot from {dur(now - new Date(p.read_at).getTime(), false)} ago
        </span>
      )}
      {p.windows.length === 0 && (
        <span className="font-mono text-[10.5px] text-muted">{p.error ?? (p.status === "no_reader" ? "no local usage source" : "no reading")}</span>
      )}
    </div>
  );
}

/** The top bar's usage pill, its popover and the optional pinned corner dock (design: UsageMini.dc.html). */
export function UsageMini() {
  const { now, go } = useApp();
  const { report } = useUsage();
  const button = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const [pinned, setPinned] = useState(() => stored(PINNED, false));
  const [dockOpen, setDockOpen] = useState(() => stored(DOCK_OPEN, true));

  const skip = report?.skip_pct ?? 90;
  const providers = report?.providers ?? [];
  const measured = providers.filter((p) => p.windows.length > 0);
  const { text, hot } = headline(report);
  const readAt = report?.read_at ? new Date(report.read_at).getTime() : null;

  const toggle = () => {
    if (pos) return setPos(null);
    const r = button.current?.getBoundingClientRect();
    if (!r) return;
    const w = Math.min(WIDTH, window.innerWidth - 16);
    // Open toward the side with room, then keep it on screen.
    const left = r.left + r.width / 2 < window.innerWidth / 2 ? r.left : r.right - w;
    setPos({ top: r.bottom + 6, left: Math.max(8, Math.min(window.innerWidth - w - 8, left)) });
  };
  const togglePin = () => {
    store(PINNED, !pinned);
    setPinned(!pinned);
  };
  const toggleDock = () => {
    store(DOCK_OPEN, !dockOpen);
    setDockOpen(!dockOpen);
  };

  return (
    <div className="relative">
      <button
        ref={button}
        onClick={toggle}
        title="Usage across all providers"
        aria-expanded={pos !== null}
        className={`flex h-7 cursor-pointer items-center gap-2.5 rounded-[7px] border px-2.5 whitespace-nowrap text-text ${hot ? "border-warn/45" : "border-line-2"} ${pos ? "bg-white/5" : "bg-transparent"}`}
      >
        {measured.length > 0 && (
          <span className="flex h-3.5 items-end gap-[5px]">
            {measured.map((p) => {
              const ws = byLength(p.windows);
              const a = ws[0];
              const b = ws[1] ?? ws[0];
              const color = providerColor(p.provider);
              return (
                <span
                  key={p.provider}
                  title={`${providerName(p.provider, true)} · ${p.windows.map((w) => `${w.short} ${windowValue(w, true)}`).join(" · ")}`}
                  className="flex h-3.5 items-end gap-px"
                >
                  {[a, b].map((w, i) => (
                    <span key={i} className="relative h-3.5 w-[3px] overflow-hidden rounded-[1px] bg-line-2">
                      <span
                        className="absolute inset-x-0 bottom-0"
                        style={{ height: `${Math.min(100, w.used_pct ?? 0)}%`, background: meterColor(w.used_pct, skip, color) }}
                      />
                    </span>
                  ))}
                </span>
              );
            })}
          </span>
        )}
        <span className="font-mono text-[11px] leading-none font-medium" style={{ color: hot ? "var(--warn)" : "var(--text-2)" }}>
          {text}
        </span>
        <span className="font-mono text-[10px] leading-none text-muted">▾</span>
      </button>

      {pos &&
        createPortal(
          <>
            <div className="fixed inset-0 z-[60]" onClick={() => setPos(null)} />
            <div
              className="fixed z-[61] flex max-h-[calc(100vh-80px)] w-[360px] max-w-[calc(100vw-16px)] flex-col gap-3 overflow-auto rounded-xl border border-line-2 bg-menu p-3 shadow-[0_20px_50px_rgba(0,0,0,.5)]"
              style={{ top: pos.top, left: pos.left }}
            >
              <div className="flex items-center justify-between gap-2.5">
                <span className="font-mono text-[10px] leading-none font-medium tracking-[0.14em] text-muted uppercase">Usage · all projects</span>
                <button
                  onClick={togglePin}
                  className={`h-6 cursor-pointer rounded-md border bg-transparent px-[9px] font-mono text-[10.5px] leading-none font-medium whitespace-nowrap ${pinned ? "border-accent text-accent-soft" : "border-line-3 text-nav"}`}
                >
                  {pinned ? "✓ pinned to corner" : "pin to corner"}
                </button>
              </div>
              {providers.map((p) => (
                <PopoverRow key={p.provider} p={p} skip={skip} now={now} />
              ))}
              <div className="flex items-center justify-between gap-2.5">
                <span className="font-mono text-[10.5px] leading-none text-muted">
                  {readAt !== null ? `read ${dur(now - readAt)} ago` : "not read yet"} · skip at {Math.round(skip)}%
                </span>
                <a
                  href="#usage"
                  onClick={(e) => {
                    e.preventDefault();
                    setPos(null);
                    go("usage");
                  }}
                  className="text-xs leading-none font-medium text-link no-underline"
                >
                  Open usage →
                </a>
              </div>
            </div>
          </>,
          document.body,
        )}

      {pinned &&
        createPortal(
          <div
            className="fixed right-4 bottom-4 z-[55] flex max-w-[calc(100vw-32px)] flex-col gap-2 rounded-xl border border-line-2 bg-menu px-3 py-2.5 shadow-[0_12px_36px_rgba(0,0,0,.45)]"
            style={{ width: dockOpen ? 300 : 230 }}
          >
            <div className="flex items-center gap-2">
              <span className="font-mono text-[9.5px] leading-none font-medium tracking-[0.14em] text-muted uppercase">usage</span>
              <span className="truncate font-mono text-[10px] leading-none whitespace-nowrap" style={{ color: hot ? "var(--warn)" : "var(--text-2)" }}>
                {text}
              </span>
              <span className="ml-auto flex gap-0.5">
                <button
                  onClick={toggleDock}
                  title={dockOpen ? "Collapse" : "Expand"}
                  className="size-[22px] cursor-pointer rounded-[5px] font-mono text-xs font-medium text-muted hover:bg-white/5"
                >
                  {dockOpen ? "–" : "+"}
                </button>
                <button
                  onClick={togglePin}
                  title="Unpin"
                  className="size-[22px] cursor-pointer rounded-[5px] font-mono text-xs font-medium text-muted hover:bg-white/5"
                >
                  ×
                </button>
              </span>
            </div>
            {dockOpen &&
              measured.map((p) => {
                const top = worstOf(p);
                const color = providerColor(p.provider);
                const bar = meterColor(top?.used_pct ?? null, skip, color);
                return (
                  <div
                    key={p.provider}
                    title={`${providerName(p.provider, true)} · tightest: ${top?.label ?? "—"}`}
                    className="grid grid-cols-[8px_64px_minmax(0,1fr)_34px_52px] items-center gap-[7px]"
                  >
                    <span className="size-[7px] rounded-full" style={{ background: color }} />
                    <span className="truncate text-[11px] leading-none font-medium whitespace-nowrap">{providerName(p.provider, true)}</span>
                    <span className="relative h-[5px] overflow-hidden rounded-[3px] bg-line">
                      <span className="absolute inset-y-0 left-0 rounded-[3px]" style={{ width: `${Math.min(100, top?.used_pct ?? 0)}%`, background: bar }} />
                    </span>
                    <span className="text-right font-mono text-[11px] leading-none font-semibold" style={{ color: bar }}>
                      {top ? windowValue(top, true) : "—"}
                    </span>
                    <span className="flex justify-end">
                      <Countdown at={top?.resets_at ?? null} now={now} />
                    </span>
                  </div>
                );
              })}
          </div>,
          document.body,
        )}
    </div>
  );
}
