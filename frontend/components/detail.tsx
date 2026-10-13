"use client";

import clsx from "clsx";
import { useEffect, useRef } from "react";

import { providerColor } from "@/lib/colors";
import type { OrchestratorStatus } from "@/lib/generated/OrchestratorStatus";
import type { Session } from "@/lib/generated/Session";

import { useApp } from "./app-context";
import { keySlug, type Command } from "./command-data";
import { Btn, LEVEL_TONE, Pill, SESSION_TONE, TASK_TONE, TONE_TEXT, hms, rel, type Tone } from "./command-ui";
import { PromptBar, closeSession, usePane } from "./console";
import { QUEUE } from "./graph/model";
import { blocked, taskActions } from "./tasks";

type Line = { text: string; tone?: Tone | "text" | "muted" };
type Action = { label: string; kind?: "primary" | "ghost" | "danger"; run: () => void; /** Only looks, so it stays in all-projects mode. */ nav?: boolean };
type Detail = {
  kicker: string;
  title: string;
  color: string;
  badge: string;
  tone: Tone;
  rows: [string, string | number | null | undefined | false][];
  blocks: { label: string; note?: string; lines: Line[] }[];
  actions: Action[];
  /** Live sessions get a terminal input. */
  live?: Session;
  /** The project of the record, for "Open in <project> →" in all-projects mode (`undefined`: none). */
  project?: string | null;
  pane?: Session;
};

const LINE_TONE: Record<string, string> = { text: "text-term-text", muted: "text-muted", ...TONE_TEXT };

/** The card of the selected node (design: `detail()` in Agent Command Centre.dc.html): kicker, title, status,
 * key/value rows, log blocks, actions, and a prompt bar for live sessions. Every value and button is real. */
export function DetailCard({
  id,
  cmd,
  orchestrator,
  select,
  fly,
  onClose,
  onNewTask,
  onNewSession,
  onOpenConsole,
  onToggleRun,
}: {
  id: string;
  cmd: Command;
  orchestrator: OrchestratorStatus | null;
  select: (id: string | null) => void;
  fly: (id: string) => void;
  onClose: () => void;
  onNewTask: () => void;
  onNewSession: (provider?: string) => void;
  onOpenConsole: (name: string) => void;
  onToggleRun: () => void;
}) {
  const { now, toast, openAuth, setProject, setScope, server } = useApp();
  const ty = id.split(":")[0];
  const key = id.slice(ty.length + 1);
  const session = ty === "s" ? cmd.sessions.find((s) => s.name === key) : undefined;
  const paneText = usePane(session?.name ?? null, !!session && session.state !== "exited", 200);
  const act = (ok: string, run: () => Promise<unknown>) => {
    run().then(
      () => toast(ok),
      (err: Error) => toast(err.message, "bad"),
    );
  };
  const flyBtn: Action = { label: "Fly to", run: () => fly(id), nav: true };

  const d = ((): Detail | null => {
    const p = cmd.pipeline;
    if (id === "orch") {
      const running = !!orchestrator?.running;
      return {
        kicker: "orchestrator",
        title: "map · orchestrator",
        color: "var(--accent)",
        badge: running ? "running" : "paused",
        tone: running ? "ok" : "idle",
        rows: [
          ["Parallel", `${orchestrator?.active ?? 0} / ${orchestrator?.max_parallel ?? 1}`],
          ["Order", p?.provider_order.join(" → ")],
          ["Verify", p && (p.verify.length ? p.verify.join(" · ") : "none configured")],
          ["Audit", p && (p.audit_enabled ? `${p.audit_chain.join(" → ")}${p.audit_required ? " · required" : ""}` : "off")],
          ["Post done", p && (p.post_done.length ? p.post_done.join(" · ") : "none")],
          ["Retries", p && `max_attempts ${p.max_attempts} · timeout ${p.task_timeout_minutes}m`],
          ["Watcher", server && `every ${server.watcher_interval_secs}s`],
        ],
        blocks: [],
        actions: [{ label: running ? "Pause orchestrator" : "Run orchestrator", kind: running ? "ghost" : "primary", run: onToggleRun }, { label: "New task", run: onNewTask }],
      };
    }
    if (ty === "p" && cmd.all) {
      const slug = keySlug(key);
      const s = cmd.summary(slug);
      const info = cmd.groups.find((g) => g.key === key);
      return {
        kicker: "project",
        title: s.name,
        color: info?.color ?? "var(--idle)",
        badge: s.state,
        tone: s.state === "running" ? "ok" : s.state === "paused" ? "warn" : "idle",
        rows: [
          ["Tasks", ["running", "verifying", "auditing", "pending", "done", "failed"].filter((x) => s.counts[x]).map((x) => `${s.counts[x]} ${x}`).join(" · ") || "none"],
          ["Live", `${s.live} sessions`],
          ["Needs you", s.need || ""],
          ["Workspace", slug === null ? server?.workspace_dir : `/git/${slug}/code.git`],
        ],
        blocks: [],
        actions: [flyBtn],
        project: slug,
      };
    }
    if (ty === "p" && key === QUEUE) {
      const q = cmd.tasks.filter((t) => !t.session || !cmd.sessions.some((s) => s.name === t.session));
      return {
        kicker: "queue",
        title: "Unassigned tasks",
        color: "var(--idle)",
        badge: `${q.length} tasks`,
        tone: "idle",
        rows: [
          ["Pending", q.filter((t) => t.status === "pending").length],
          ["Failed", q.filter((t) => t.status === "failed").length],
          ["Cancelled", q.filter((t) => t.status === "cancelled").length],
          ["Rule", "starts when every depends_on task is done"],
        ],
        blocks: [],
        actions: [{ label: "New task", kind: "primary", run: onNewTask }, flyBtn],
      };
    }
    if (ty === "p") {
      const info = cmd.providers.find((x) => x.name === key);
      if (!info) return null;
      const st = cmd.providerState(info, now);
      const live = cmd.sessions.filter((s) => s.provider === key && s.state !== "exited");
      const order = p?.provider_order.indexOf(key) ?? -1;
      const actions: Action[] = [];
      if (st.signIn) actions.push({ label: `Sign in to ${key}`, kind: "primary", run: () => openAuth(key) });
      if (info.available) actions.push({ label: "Start session", kind: st.signIn ? "ghost" : "primary", run: () => onNewSession(key) });
      actions.push(flyBtn);
      return {
        kicker: "provider",
        title: key,
        color: providerColor(key),
        badge: st.word,
        tone: st.tone,
        rows: [
          ["Auth", !info.available ? "—" : st.signedIn === true ? "signed in" : st.signedIn === false || st.signIn ? "signed out · new tasks skip it" : "unknown"],
          ["Default", info.default_model],
          ["Models", info.models.join(" · ")],
          ["Binary", info.available ? "found" : "not found on PATH"],
          ["Cooldown", info.cooldown_until && new Date(info.cooldown_until).getTime() > now ? `skipped until ${hms(info.cooldown_until)} (${rel(info.cooldown_until, now)})` : ""],
          ["Live", `${live.length} sessions`],
          ["Order", order >= 0 ? `${order + 1} of ${p?.provider_order.length}` : "not in provider_order"],
          ["Audits", p?.audit_enabled && p.audit_chain.includes(key) ? `${p.audit_chain.indexOf(key) + 1} of ${p.audit_chain.length}` : ""],
        ],
        blocks: [],
        actions,
      };
    }
    if (ty === "s") {
      const s = session;
      if (!s) return null;
      const t = cmd.tasks.find((x) => x.id === s.task_id);
      const live = s.state !== "exited";
      const lines: Line[] = paneText
        ? paneText
            .split("\n")
            .slice(-18)
            .map((text) => ({ text: text || " ", tone: "text" as const }))
        : [{ text: live ? "reading the pane…" : "(no final pane saved)", tone: "muted" }];
      if (s.state === "rate_limited" && s.rate_limit_reset_at) lines.push({ text: `⎿ Usage limit · the watcher wakes it ${rel(s.rate_limit_reset_at, now)}`, tone: "warn" });
      if (!live) lines.push({ text: `[exited] final pane saved under .map/runs/${s.name}/`, tone: "muted" });
      const actions: Action[] = [];
      if (s.state === "needs_auth") actions.push({ label: `Sign in to ${s.provider}`, kind: "primary", run: () => openAuth(s.provider) });
      if (t) actions.push({ label: `Open task #${t.id}`, run: () => select(`t:${t.id}`), nav: true });
      actions.push({ label: "Console", run: () => onOpenConsole(s.name) });
      actions.push(flyBtn);
      actions.push({ label: live ? "Close session" : "Remove", kind: "danger", run: () => void closeSession(s, toast).then((ok) => ok && !live && select(null)) });
      return {
        kicker: `session · ${s.role}`,
        title: s.name,
        color: cmd.all ? cmd.colorOfProject(s.project) : providerColor(s.provider),
        badge: s.state.replace("_", " "),
        tone: SESSION_TONE[s.state],
        rows: [
          ["Provider", `${s.provider} · ${s.model}`],
          ["Project", cmd.all && cmd.projectName(s.project)],
          ["Task", t ? `${cmd.taskLabel(t)} ${t.title}` : "—"],
          ["Detail", s.detail],
          ["Wakes", s.state === "rate_limited" && s.rate_limit_reset_at ? rel(s.rate_limit_reset_at, now) : ""],
          ["Wake-ups", s.wakeups || ""],
          ["Attached", s.attached ? "yes · protected from kill" : ""],
          ["Started", rel(s.created_at, now)],
          ["Active", rel(s.last_activity_at, now)],
        ],
        blocks: [live ? { label: "live pane", note: `tmux attach -t '=${s.name}'`, lines } : { label: "final pane", lines }],
        actions,
        live: live ? s : undefined,
        project: s.project,
      };
    }
    if (ty === "t") {
      const t = cmd.tasks.find((x) => x.id === Number(key));
      if (!t) return null;
      const s = t.session ? cmd.sessions.find((x) => x.name === t.session) : undefined;
      const isBlocked = blocked(t, cmd.tasks);
      const actions: Action[] = [];
      if (s) actions.push({ label: `Open ${s.name}`, run: () => select(`s:${s.name}`), nav: true });
      for (const a of taskActions(t, act)) actions.push({ label: a.label, kind: a.kind, run: a.run });
      actions.push(flyBtn);
      const blocks: Detail["blocks"] = [{ label: "prompt", lines: t.prompt.split("\n").map((text) => ({ text: text || " ", tone: "text" })) }];
      if (t.result) blocks.push({ label: "result", lines: t.result.split("\n").map((text) => ({ text: text || " ", tone: t.status === "failed" ? "bad" : "ok" })) });
      return {
        kicker: `${cmd.all ? `${cmd.projectName(t.project)} · ` : ""}task #${t.id}`,
        title: t.title,
        color: s ? (cmd.all ? cmd.colorOfProject(t.project) : providerColor(s.provider)) : "var(--idle)",
        badge: isBlocked ? "blocked" : t.status,
        tone: isBlocked ? "bad" : TASK_TONE[t.status],
        rows: [
          ["Depends on", t.depends_on.length ? t.depends_on.map((x) => `#${x}`).join(", ") : "none"],
          ["Provider", t.provider ? `pinned · ${t.provider}${t.model ? ` (${t.model})` : ""}` : "any · failover"],
          ["Session", t.session ?? "not started"],
          ["Attempts", `${t.attempts}${p ? ` of ${p.max_attempts}` : ""}`],
          ["Created", rel(t.created_at, now)],
          ["Updated", rel(t.updated_at, now)],
        ],
        blocks,
        actions,
        project: t.project,
      };
    }
    if (ty === "g") {
      const g = cmd.graph.stages.find((x) => x.id === key);
      if (!g) return null;
      const act = cmd.graph.activeStages.has(id);
      const users = cmd.tasks.filter((t) => (g.phase === "verify" && t.status === "verifying") || (g.phase === "audit" && t.status === "auditing"));
      const done = cmd.tasks
        .filter((t) => t.status === "done")
        .slice(-4)
        .map((t) => ({ text: `✓ ${cmd.taskLabel(t)} ${t.title}`, tone: "ok" as const }));
      const ofPipeline = g.phase === "verify" || g.phase === "audit" || g.phase === "post_done";
      return {
        kicker: `pipeline · ${g.phase.replace("_", " ")}`,
        title: g.label,
        color: act ? "var(--info)" : "var(--text-2)",
        badge: act ? "running" : "idle",
        tone: act ? "info" : "idle",
        rows: [
          ["Phase", g.phase],
          ["Command", g.command],
          ["Running for", ofPipeline ? users.map((t) => cmd.taskLabel(t)).join(", ") || "—" : ""],
          ["Rule", g.phase === "verify" ? "every command must exit 0, else the task retries with the output" : g.phase === "audit" ? "first auditor with a verdict decides; VERDICT: PASS accepts" : g.phase === "post_done" ? "runs after a task passes; {id} and {title} are substituted" : ""],
        ],
        blocks: ofPipeline && done.length ? [{ label: "recent passes", lines: done }] : [],
        actions: [flyBtn],
      };
    }
    if (ty === "r") {
      const n = cmd.graph.nodes.find((x) => x.id === id);
      if (!n || n.ref.kind !== "run") return null;
      const e = n.ref.event;
      const s = cmd.sessions.find((x) => x.name === e.session);
      return {
        kicker: `run event · ${e.kind}`,
        title: e.message,
        color: n.color,
        badge: e.level,
        tone: LEVEL_TONE[e.level],
        rows: [
          ["Session", e.session],
          ["Task", e.task_id != null ? `#${e.task_id}` : ""],
          ["At", `${hms(e.at)} · ${rel(e.at, now)}`],
          ["Run files", e.session ? `.map/runs/${e.session}/` : ""],
        ],
        blocks: [],
        actions: [...(s ? [{ label: `Open ${s.name}`, run: () => select(`s:${s.name}`), nav: true }] : []), flyBtn],
        project: s?.project,
      };
    }
    return null;
  })();

  const blockRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // Live panes follow their last line.
    if (blockRef.current) blockRef.current.scrollTop = blockRef.current.scrollHeight;
  }, [paneText]);

  if (!d) return null;
  let actions = d.actions;
  if (cmd.all) {
    actions = actions.filter((a) => a.nav);
    if (d.project !== undefined) {
      const slug = d.project;
      actions = [
        {
          label: `Open in ${cmd.projectName(slug)} →`,
          kind: "primary",
          run: () => {
            setProject(slug);
            setScope("project");
          },
        },
        ...actions,
      ];
    }
  }
  const rows = d.rows.filter(([, v]) => v !== null && v !== undefined && v !== "" && v !== false);

  return (
    <div className="flex flex-col rounded-xl border border-line-2 bg-[color-mix(in_srgb,var(--menu)_94%,transparent)] shadow-[0_20px_60px_rgba(0,0,0,.55)] backdrop-blur-md">
      <div className="flex items-start gap-2.5 border-b border-line py-3.5 pr-3.5 pl-4">
        <div className="flex min-w-0 flex-1 flex-col gap-[7px]">
          <span className="flex items-center gap-[7px] font-mono text-[10px] leading-none font-medium tracking-[0.14em] text-muted uppercase">
            <span className="size-[7px] flex-none rounded-full" style={{ background: d.color }} />
            <span className="truncate">{d.kicker}</span>
          </span>
          <span className="text-[15px] leading-[1.3] font-semibold break-words text-pretty">{d.title}</span>
        </div>
        <Pill tone={d.tone}>{d.badge}</Pill>
        <button onClick={onClose} className="size-6 flex-none cursor-pointer rounded-md border border-line-2 text-[13px] leading-none text-nav" aria-label="Close card">
          ×
        </button>
      </div>
      {rows.length > 0 && (
        <div className="flex flex-col gap-[7px] px-4 py-3">
          {rows.map(([k, v]) => (
            <div key={k} className="grid grid-cols-[92px_minmax(0,1fr)] items-baseline gap-3">
              <span className="font-mono text-[10px] leading-[1.5] tracking-[0.08em] text-muted uppercase">{k}</span>
              <span className="text-[12.5px] leading-[1.4] break-words text-text/90">{String(v)}</span>
            </div>
          ))}
        </div>
      )}
      {d.blocks.map((b, i) => (
        <div key={b.label} className="flex flex-col gap-1.5 px-4 pb-3">
          <span className="truncate font-mono text-[10px] leading-none tracking-[0.12em] text-muted uppercase">
            {b.label}
            {b.note && <span className="tracking-[0.02em] normal-case"> · {b.note}</span>}
          </span>
          <div
            ref={i === 0 && d.live ? blockRef : undefined}
            className="flex max-h-[230px] flex-col overflow-auto rounded-lg border border-line bg-term-bg px-3 py-2.5 font-mono text-[11.5px] leading-[1.6]"
          >
            {b.lines.map((l, j) => (
              <span key={j} className={clsx("break-words whitespace-pre-wrap", LINE_TONE[l.tone ?? "text"])}>
                {l.text}
              </span>
            ))}
          </div>
        </div>
      ))}
      {d.live && !cmd.all && (
        <div className="px-4 pb-3">
          <PromptBar session={d.live} toast={toast} />
        </div>
      )}
      {actions.length > 0 && (
        <div className="flex flex-wrap gap-2 border-t border-line px-4 pt-3 pb-4">
          {actions.map((a) => (
            <Btn key={a.label} kind={a.kind} onClick={a.run}>
              {a.label}
            </Btn>
          ))}
        </div>
      )}
    </div>
  );
}
