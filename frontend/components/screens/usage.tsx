"use client";

import { RotateCw } from "lucide-react";

import { projectColor, providerColor } from "@/lib/colors";
import type { ProjectShare } from "@/lib/generated/ProjectShare";
import type { ProviderUsage } from "@/lib/generated/ProviderUsage";
import type { UsageWindow } from "@/lib/generated/UsageWindow";
import { badgeOf, clockAt, dur, fmtTokens, meterColor, providerName, readUsageNow, useUsage, worstOf } from "@/lib/use-usage";

import { useApp } from "../app-context";
import { Main } from "../shell/page";
import { TopBar } from "../shell/top-bar";

const mono = "font-mono";

/** Badge colours of the design (text, fill, border) by tone. */
const TONE: Record<"ok" | "warn" | "bad" | "idle", string> = {
  ok: "text-ok bg-ok/[.08] border-ok/30",
  warn: "text-warn bg-warn/[.08] border-warn/30",
  bad: "text-bad bg-bad/10 border-bad/40",
  idle: "text-muted bg-white/[.03] border-line-2",
};

function StatusBadge({ text, tone }: { text: string; tone: keyof typeof TONE }) {
  return (
    <span
      className={`inline-flex h-5 flex-none items-center rounded border px-[7px] ${mono} text-[9.5px] font-medium tracking-[0.08em] whitespace-nowrap uppercase ${TONE[tone]}`}
    >
      {text}
    </span>
  );
}

/** "at this pace: skip line at 16:40, limit at reset", from the last hour's rate (percentage points per hour). */
function paceNote(w: UsageWindow, p: ProviderUsage, skip: number, now: number): { text: string; warn: boolean } | null {
  if (w.note) return { text: w.note, warn: false };
  const used = w.used_pct;
  if (used === null || !w.resets_at) return null;
  if (used >= skip) return { text: "over skip line · no new tasks until reset", warn: true };
  const rate = w.rate_per_hour;
  if (rate === null) {
    if (p.status === "live") return { text: "pace: after a few readings", warn: false };
    if (p.status === "snapshot" || p.status === "signed_out") return { text: "no pace: codex has not run in the last hour", warn: false };
    return null;
  }
  const left = new Date(w.resets_at).getTime() - now;
  if (rate <= 0) return { text: "at this pace: no change by reset", warn: false };
  const hour = 3600000;
  const skipIn = ((skip - used) / rate) * hour;
  const hitIn = ((100 - used) / rate) * hour;
  if (skipIn < left) {
    return { text: `at this pace: skip line at ${clockAt(now + skipIn, now)}, limit at ${hitIn < left ? clockAt(now + hitIn, now) : "reset"}`, warn: true };
  }
  return { text: `at this pace: ~${Math.round(w.projected_pct ?? used)}% by reset`, warn: false };
}

function Meter({ w, p, color, skip, now }: { w: UsageWindow; p: ProviderUsage; color: string; skip: number; now: number }) {
  const used = w.used_pct;
  const tone = meterColor(used, skip, color);
  const fill = Math.min(100, used ?? 0);
  const proj = w.projected_pct !== null && used !== null ? Math.max(0, Math.min(100, w.projected_pct) - fill) : 0;
  const pace = paceNote(w, p, skip, now);
  const value =
    w.cap !== null && w.used !== null
      ? `${Math.round(w.used).toLocaleString("en-US")} / ${w.cap.toLocaleString("en-US")}`
      : used === null
        ? "—"
        : `${Math.round(used)}%`;
  const reset = w.resets_at ? new Date(w.resets_at).getTime() : null;
  return (
    <div className="flex flex-col gap-[7px]">
      <div className="flex items-baseline justify-between gap-2.5">
        <span className="text-[12.5px] leading-[1.2] font-medium text-text">{w.label}</span>
        <span className={`${mono} text-[15px] leading-none font-semibold whitespace-nowrap`} style={{ color: tone }}>
          {value}
        </span>
      </div>
      <div className="relative h-2 overflow-hidden rounded bg-line">
        <span className="absolute inset-y-0 left-0 rounded" style={{ width: `${fill}%`, background: tone }} />
        {proj > 0 && (
          <span
            className="absolute inset-y-0 opacity-45"
            style={{ left: `${fill}%`, width: `${proj}%`, background: `repeating-linear-gradient(90deg, ${tone} 0 2px, transparent 2px 4px)` }}
          />
        )}
        <span className="absolute inset-y-0 w-px bg-text/60" style={{ left: `${skip}%` }} />
      </div>
      <div className={`flex justify-between gap-2.5 ${mono} text-[10.5px] leading-[1.3] text-muted`}>
        <span>{w.kind}</span>
        {reset !== null && (
          <span className="whitespace-nowrap">{reset > now ? `resets in ${dur(reset - now)} · ${clockAt(reset, now)}` : `reset ${clockAt(reset, now)}`}</span>
        )}
      </div>
      {pace && (
        <span className={`${mono} text-[10.5px] leading-[1.3]`} style={{ color: pace.warn ? "var(--warn)" : "var(--muted)" }}>
          {pace.text}
        </span>
      )}
    </div>
  );
}

function shareColor(s: ProjectShare, order: (string | null)[]): string {
  if (s.kind === "other") return "#8b95a5";
  return projectColor(s.kind === "project" ? s.slug : null, order);
}

function ProviderCard({ p, skip, now, order }: { p: ProviderUsage; skip: number; now: number; order: (string | null)[] }) {
  const color = providerColor(p.provider);
  const badge = badgeOf(p, skip);
  const near = badge.text === "skipped" || badge.text === "limited";
  const border = near ? (badge.tone === "bad" ? "rgba(242,109,109,.4)" : "rgba(242,193,78,.45)") : undefined;
  const total = p.by_project.reduce((n, s) => n + s.tokens, 0);
  const t = p.tokens_today;
  const age = p.read_at ? now - new Date(p.read_at).getTime() : null;
  return (
    <div className="flex min-w-0 flex-col gap-4 rounded-[14px] border border-line bg-surface p-[18px]" style={border ? { borderColor: border } : undefined}>
      <div className="flex items-start gap-2.5">
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <span className="flex items-center gap-2">
            <span className="size-[9px] rounded-full" style={{ background: color }} />
            <span className="text-base leading-[1.2] font-semibold">{providerName(p.provider)}</span>
          </span>
          <span className={`${mono} text-[11px] leading-[1.3] text-muted`}>
            {p.plan ?? "plan unknown"} · {p.status === "no_reader" ? "no local usage source" : `read via ${p.source}`}
            {p.status === "snapshot" || p.status === "signed_out" ? (age !== null ? ` · snapshot ${dur(age, false)} old` : "") : ""}
          </span>
        </div>
        <StatusBadge text={badge.text} tone={badge.tone} />
      </div>

      {p.error && <p className={`${mono} -mt-1 text-[10.5px] leading-[1.4] text-warn`}>{p.error}</p>}

      {p.status === "no_reader" && (
        <p className="text-[12.5px] leading-[1.5] text-text-2">
          MAP has no local source for this CLI&apos;s plan limits yet, so it shows none rather than a guess. Its tasks are only paused by rate-limit messages
          the watcher sees in its pane.
        </p>
      )}
      {p.status === "disabled" && <p className="text-[12.5px] leading-[1.5] text-text-2">The reader is turned off in [usage.providers.{p.provider}].</p>}
      {p.status === "pending" && <p className="text-[12.5px] leading-[1.5] text-text-2">Reading…</p>}

      {p.windows.map((w) => (
        <Meter key={w.id} w={w} p={p} color={color} skip={skip} now={now} />
      ))}

      {t && (
        <div className="grid grid-cols-3 gap-2 rounded-[9px] border border-line bg-inset px-3 py-2.5" title={`cache writes today: ${fmtTokens(t.cache_write)}`}>
          {(
            [
              ["input · today", fmtTokens(t.input)],
              ["output · today", fmtTokens(t.output)],
              ["cache read · today", fmtTokens(t.cache_read)],
            ] as const
          ).map(([k, v]) => (
            <span key={k} className="flex min-w-0 flex-col gap-[5px]">
              <span className={`${mono} truncate text-[9.5px] leading-none tracking-[0.1em] whitespace-nowrap text-muted uppercase`}>{k}</span>
              <span className={`${mono} truncate text-sm leading-none font-semibold whitespace-nowrap text-text`}>{v}</span>
            </span>
          ))}
        </div>
      )}

      {t && (
        <div className="flex flex-col gap-[7px]">
          <span className={`${mono} text-[9.5px] leading-none tracking-[0.1em] text-muted uppercase`}>
            by project · this window{p.by_project_since ? ` · since ${clockAt(new Date(p.by_project_since).getTime(), now)}` : ""}
          </span>
          {total > 0 ? (
            <>
              <div className="flex h-1.5 gap-0.5 overflow-hidden rounded-[3px]">
                {p.by_project.map((s) => (
                  <span
                    key={`${s.kind}-${s.slug ?? s.name}`}
                    title={`${s.name}: ${fmtTokens(s.tokens)} tokens`}
                    style={{ width: `${(100 * s.tokens) / total}%`, background: shareColor(s, order) }}
                  />
                ))}
              </div>
              <div className="flex flex-wrap gap-x-3 gap-y-1">
                {p.by_project.map((s) => (
                  <span
                    key={`${s.kind}-${s.slug ?? s.name}`}
                    className={`flex items-center gap-[5px] ${mono} text-[10.5px] leading-[1.2] whitespace-nowrap text-text-2`}
                  >
                    <span className="size-1.5 rounded-[2px]" style={{ background: shareColor(s, order) }} />
                    {s.name} {Math.round((100 * s.tokens) / total)}%
                  </span>
                ))}
              </div>
            </>
          ) : (
            <span className={`${mono} text-[10.5px] text-muted`}>no tokens in this window</span>
          )}
        </div>
      )}
    </div>
  );
}

function Strip({ p, skip }: { p: ProviderUsage; skip: number }) {
  const top = worstOf(p);
  const badge = badgeOf(p, skip);
  const flagged = badge.text === "skipped" || badge.text === "limited";
  const note = top ? `${flagged ? "skipped · " : ""}${top.label.toLowerCase()}` : badge.text;
  const pct = top?.used_pct ?? null;
  return (
    <div className="flex min-w-0 flex-[1_1_200px] items-center gap-2.5 rounded-[10px] border border-line bg-surface px-3 py-2.5">
      <span className="size-2 flex-none rounded-full" style={{ background: providerColor(p.provider) }} />
      <span className="flex min-w-0 flex-col gap-[5px]">
        <span className="text-[12.5px] leading-[1.1] font-medium">{providerName(p.provider)}</span>
        <span
          className={`${mono} truncate text-[10.5px] leading-[1.2] whitespace-nowrap`}
          style={{ color: flagged || badge.tone === "bad" ? "var(--warn)" : "var(--muted)" }}
        >
          {note}
        </span>
      </span>
      <span
        className={`ml-auto ${mono} text-lg leading-none font-semibold`}
        style={{ color: pct === null ? "var(--muted)" : pct >= 100 ? "var(--bad)" : pct >= skip ? "var(--warn)" : "var(--text)" }}
      >
        {pct === null ? "—" : `${Math.round(pct)}%`}
      </span>
    </div>
  );
}

/** Plan limits for each agent account on this machine (design: Usage.dc.html). Shared by all projects. */
export function UsageScreen() {
  const { now, projects } = useApp();
  const { report, reading, error } = useUsage();
  const skip = report?.skip_pct ?? 90;
  const order: (string | null)[] = [null, ...projects.map((p) => p.project.slug)];
  const readAt = report?.read_at ? new Date(report.read_at).getTime() : null;
  const readLabel = reading
    ? "reading /usage, codex logs…"
    : readAt !== null
      ? `read ${dur(now - readAt)} ago`
      : error
        ? `could not load: ${error}`
        : "not read yet";
  return (
    <>
      <TopBar />
      <Main className="!gap-[22px]">
        <div className="flex flex-wrap items-end justify-between gap-5">
          <div className="flex max-w-[720px] flex-col gap-2.5">
            <span className={`${mono} text-[10px] leading-none font-medium tracking-[0.2em] text-muted uppercase`}>
              Accounts on this machine · shared by all projects
            </span>
            <h1 className="m-0 text-[30px] leading-[1.05] font-semibold tracking-[-0.02em] sm:text-[38px]">Usage</h1>
            <p className="m-0 text-sm leading-[1.55] text-pretty text-text-2">
              Plan limits for each agent account, read from each CLI&apos;s own usage source. The orchestrator stops giving new tasks to a provider once any of
              its windows passes {Math.round(skip)}%.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2.5">
            <span className={`${mono} text-[11px] leading-none whitespace-nowrap text-muted`}>{readLabel}</span>
            <button
              onClick={() => void readUsageNow()}
              disabled={reading}
              className="flex h-[34px] cursor-pointer items-center gap-1.5 rounded-lg border border-accent bg-accent px-3.5 text-[12.5px] leading-none font-semibold whitespace-nowrap text-accent-text disabled:cursor-wait disabled:opacity-80"
            >
              <RotateCw className={`size-3.5 ${reading ? "animate-spin" : ""}`} strokeWidth={2.5} />
              {reading ? "Reading…" : "Read now"}
            </button>
          </div>
        </div>

        {report && (
          <>
            <div className="flex flex-wrap gap-2">
              {report.providers.map((p) => (
                <Strip key={p.provider} p={p} skip={skip} />
              ))}
            </div>

            <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fit,minmax(min(320px,100%),1fr))]">
              {report.providers.map((p) => (
                <ProviderCard key={p.provider} p={p} skip={skip} now={now} order={order} />
              ))}
            </div>

            <div className={`flex flex-wrap gap-x-[18px] gap-y-1.5 ${mono} text-[11px] leading-[1.5] text-muted`}>
              <span className="flex items-center gap-1.5">
                <span className="h-2.5 w-px bg-text/60" />
                skip line · {Math.round(skip)}%
              </span>
              <span className="flex items-center gap-1.5">
                <span className="h-1.5 w-3.5 opacity-60" style={{ background: "repeating-linear-gradient(90deg, var(--text-2) 0 2px, transparent 2px 4px)" }} />
                projected use by reset, at the last hour&apos;s pace
              </span>
              <span>tokens from the CLIs&apos; session logs; shares exclude cache reads</span>
            </div>
          </>
        )}
      </Main>
    </>
  );
}
