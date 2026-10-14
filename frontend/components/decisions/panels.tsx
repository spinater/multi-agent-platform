"use client";

import clsx from "clsx";
import type { ReactNode } from "react";

import type { DecisionModel } from "@/lib/generated/DecisionModel";
import type { DecisionNode } from "@/lib/generated/DecisionNode";
import type { Event } from "@/lib/generated/Event";
import type { Task } from "@/lib/generated/Task";

import type { Selection } from "./graph";
import { decisionColor, hms, nodeTone, tint, type Check, type KindMap } from "./model";

export type Row = {
  task: Task;
  node: DecisionNode;
  provider: string | null;
  color: string;
  sub: string;
};

function Tag({ color, children, size = "sm" }: { color: string; children: ReactNode; size?: "sm" | "md" }) {
  return (
    <span
      className={clsx("inline-flex flex-none items-center rounded font-mono font-medium tracking-[0.08em] whitespace-nowrap uppercase", size === "sm" ? "h-[18px] px-1.5 text-[9px]" : "h-5 px-[7px] text-[9.5px]")}
      style={{ color, background: tint(color, 10), border: `1px solid ${tint(color, 35)}` }}
    >
      {children}
    </span>
  );
}

/** Node chips with counts; clicking one filters the list. */
export function NodeChips({ chips, filter, onPick }: { chips: { node: DecisionNode; label: string; n: number }[]; filter: DecisionNode | null; onPick: (n: DecisionNode) => void }) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {chips.map((c) => {
        const color = nodeTone(c.node);
        const on = filter === c.node;
        return (
          <button
            key={c.node}
            onClick={() => onPick(c.node)}
            className="flex h-[30px] items-center gap-2 rounded-[7px] border px-[11px] font-mono text-[11.5px] font-medium whitespace-nowrap text-text"
            style={{ borderColor: on ? color : "var(--line-2)", background: on ? tint(color, 12) : "transparent" }}
          >
            <span className="size-[7px] rounded-[2px]" style={{ background: color }} />
            {c.label}
            <span className="font-semibold" style={{ color }}>
              {c.n}
            </span>
          </button>
        );
      })}
    </div>
  );
}

/** "Tasks by decision point". */
export function TaskRows({ rows, label, selected, nodeLabel, onSelect }: { rows: Row[]; label: string; selected: number | null; nodeLabel: (n: DecisionNode) => string; onSelect: (id: number) => void }) {
  return (
    <section className="flex min-w-0 flex-[1_1_320px] flex-col gap-2.5">
      <div className="flex items-center justify-between gap-2.5 font-mono text-[11px] font-semibold tracking-[0.14em] uppercase">
        <span className="flex items-center gap-2 whitespace-nowrap">
          <span className="text-accent">◇</span>Tasks by decision point
        </span>
        <span className="truncate font-normal tracking-[0.06em] text-muted normal-case">{label}</span>
      </div>
      <div className="flex flex-col gap-[5px]">
        {rows.map((r) => {
          const sel = selected === r.task.id;
          const tone = nodeTone(r.node);
          return (
            <button
              key={r.task.id}
              onClick={() => onSelect(r.task.id)}
              className="grid grid-cols-[30px_8px_minmax(0,1fr)_auto] items-center gap-[9px] rounded-lg border px-2.5 py-[9px] text-left text-text"
              style={{ borderColor: sel ? tint("var(--accent)", 50) : "var(--line)", background: sel ? tint("var(--accent)", 7) : "var(--surface)" }}
            >
              <span className="font-mono text-[10.5px] text-muted">#{r.task.id}</span>
              <span className="size-[7px] rounded-full" style={{ background: r.color }} />
              <span className="flex min-w-0 flex-col gap-1">
                <span className="truncate text-[12.5px] leading-tight font-medium">{r.task.title}</span>
                <span className="truncate font-mono text-[10.5px] text-muted">{r.sub}</span>
              </span>
              <Tag color={tone}>{nodeLabel(r.node)}</Tag>
            </button>
          );
        })}
      </div>
    </section>
  );
}

type Item = { key: string | number; time: string; tag: string; title: string; detail: string; color: string; onClick: () => void };

/** The right-hand panel: a task with its "now deciding" box and trace, or a node with its rule and tasks. */
function Panel({ kicker, dot, title, sub, badge, badgeColor, now, listLabel, items }: {
  kicker: string;
  dot: string;
  title: string;
  sub: string;
  badge: string;
  badgeColor: string;
  now: { kicker: string; color: string; question: string; checks: Check[]; source: ReactNode };
  listLabel: string;
  items: Item[];
}) {
  return (
    <section className="flex min-w-0 flex-[1.4_1_420px] flex-col overflow-hidden rounded-[14px] border border-line-2 bg-[color-mix(in_srgb,var(--panel)_92%,transparent)]">
      <div className="flex items-start gap-3 border-b border-line px-[18px] pt-4 pb-3.5">
        <div className="flex min-w-0 flex-1 flex-col gap-[7px]">
          <span className="flex items-center gap-[7px] font-mono text-[10px] font-medium tracking-[0.14em] text-muted uppercase">
            <span className="size-[7px] rounded-full" style={{ background: dot }} />
            {kicker}
          </span>
          <span className="text-lg leading-tight font-semibold break-words">{title}</span>
          <span className="font-mono text-[11px] text-muted">{sub}</span>
        </div>
        <Tag color={badgeColor} size="md">
          {badge}
        </Tag>
      </div>
      <div className="mx-[18px] mt-3.5 mb-1 flex flex-col gap-2 rounded-[10px] border px-4 py-3.5" style={{ borderColor: tint(now.color, 40), background: tint(now.color, 6) }}>
        <span className="font-mono text-[10px] font-semibold tracking-[0.16em] uppercase" style={{ color: now.color }}>
          {now.kicker}
        </span>
        <span className="text-[14.5px] leading-snug font-semibold">{now.question}</span>
        {now.checks.map((c, i) => (
          <div key={i} className="grid grid-cols-[14px_minmax(0,1fr)] gap-2 font-mono text-xs leading-normal">
            <span style={{ color: c.color }}>{c.mark}</span>
            <span className="break-words text-text-2">{c.text}</span>
          </div>
        ))}
        <span className="font-mono text-[10.5px] leading-snug text-muted">{now.source}</span>
      </div>
      <div className="flex flex-col px-[18px] pt-3.5 pb-[18px]">
        <span className="mb-2.5 font-mono text-[10px] tracking-[0.12em] text-muted uppercase">{listLabel}</span>
        {items.map((it) => (
          <button key={it.key} onClick={it.onClick} className="grid grid-cols-[62px_14px_minmax(0,1fr)] gap-2.5 text-left text-text">
            <span className="pt-2 font-mono text-[10.5px] leading-normal text-muted">{it.time}</span>
            <span className="relative flex justify-center">
              <span className="absolute inset-y-0 w-px bg-line" />
              <span className="relative mt-[11px] size-[9px] rounded-full" style={{ background: it.color, boxShadow: "0 0 0 3px var(--panel)" }} />
            </span>
            <span className="flex min-w-0 flex-col gap-[5px] pt-[7px] pb-3">
              <span className="flex flex-wrap items-center gap-2">
                <span className="inline-flex h-[18px] items-center rounded border border-line-2 bg-white/5 px-1.5 font-mono text-[9.5px] font-medium tracking-[0.08em] whitespace-nowrap uppercase">{it.tag}</span>
                <span className="text-[12.5px] leading-tight font-medium" style={{ color: it.color }}>
                  {it.title}
                </span>
              </span>
              <span className="font-mono text-[11.5px] leading-normal break-words text-text-2">{it.detail}</span>
            </span>
          </button>
        ))}
      </div>
    </section>
  );
}

export function TaskPanel({ task, node, provider, color, model, kinds, trace, checks, attempt, onSelect }: {
  task: Task;
  node: DecisionNode;
  provider: string | null;
  color: string;
  model: DecisionModel;
  kinds: KindMap;
  trace: Event[];
  checks: Check[];
  attempt: number;
  onSelect: (s: Selection) => void;
}) {
  const point = model.nodes.find((n) => n.id === node)!;
  const tone = nodeTone(node);
  const label = (n: DecisionNode) => model.nodes.find((x) => x.id === n)?.label ?? n;
  return (
    <Panel
      kicker={`task #${task.id}${provider ? ` · ${provider}` : ""} · attempt ${attempt}`}
      dot={color}
      title={task.title}
      sub={`${task.session ?? "no session yet"}${task.depends_on.length ? ` · after ${task.depends_on.map((d) => `#${d}`).join(", ")}` : ""}${task.git ? ` · ${task.git.branch} → ${task.git.base}` : ""}`}
      badge={`at ${point.label}`}
      badgeColor={tone}
      now={{
        kicker: point.terminal ? "outcome" : `now deciding · ${point.label}`,
        color: tone,
        question: point.question,
        checks,
        source: (
          <>
            {point.source} · <span className="text-text-2">{point.rule}</span>
          </>
        ),
      }}
      listLabel={trace.length ? "decision trace · newest first" : "no decisions recorded yet"}
      items={trace.map((e: Event) => {
        const k = kinds.get(e.kind);
        return {
          key: e.id,
          time: hms(e.at),
          tag: k ? label(k.from) : e.kind,
          title: k?.outcome ?? e.kind,
          detail: e.message,
          color: decisionColor(e, k),
          onClick: () => k && onSelect({ type: "node", id: k.from }),
        };
      })}
    />
  );
}

export function NodePanel({ node, model, counts, rows, onSelect }: { node: DecisionNode; model: DecisionModel; counts: Map<string, number>; rows: Row[]; onSelect: (s: Selection) => void }) {
  const point = model.nodes.find((n) => n.id === node)!;
  const tone = nodeTone(node);
  const label = (n: DecisionNode) => model.nodes.find((x) => x.id === n)?.label ?? n;
  const outs = model.edges.filter((e) => e.from === node);
  const ins = model.edges.filter((e) => e.to === node).map((e) => label(e.from));
  const checks: Check[] = outs.length
    ? outs.map((e) => ({
        mark: "→",
        color: ["retry", "failed", "blocked"].includes(e.to) ? "var(--bad)" : "var(--ok)",
        text: `${e.label} → ${label(e.to)} · ×${counts.get(`${e.from}>${e.to}`) ?? 0} today`,
      }))
    : [{ mark: "·", color: "var(--idle)", text: `terminal state · arrives from ${ins.join(", ")}` }];
  return (
    <Panel
      kicker="decision point"
      dot={tone}
      title={point.label}
      sub={point.source}
      badge={`${rows.length} here`}
      badgeColor={tone}
      now={{
        kicker: "rule",
        color: tone,
        question: point.question,
        checks: [{ mark: "≡", color: tone, text: point.rule }, ...checks],
        source: `arrives from: ${ins.join(", ") || "new tasks"}`,
      }}
      listLabel={rows.length ? "tasks here now" : "no tasks here right now"}
      items={rows.map((r) => ({
        key: r.task.id,
        time: `#${r.task.id}`,
        tag: r.provider ?? "unassigned",
        title: r.task.title,
        detail: r.sub,
        color: r.color,
        onClick: () => onSelect({ type: "task", id: r.task.id }),
      }))}
    />
  );
}
