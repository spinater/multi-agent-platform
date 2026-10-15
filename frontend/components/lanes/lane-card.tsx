"use client";

/** One lane on the Lanes screen: its sessions, the agent's status file, cards, git, the switches, queued tells, what
 * MAP did, the screen tail and the status prompt. Nothing here types into tmux directly: a tell is queued and the
 * backend does it when the session is idle and nobody is attached. */

import clsx from "clsx";
import { ChevronDown, ChevronRight, RefreshCw, X } from "lucide-react";
import { useState, type ReactNode } from "react";

import { lanesApi, managerApi, type LanePatch } from "@/lib/api";
import type { LaneInfo } from "@/lib/generated/LaneInfo";
import type { LaneScreen } from "@/lib/generated/LaneScreen";
import type { LaneSession } from "@/lib/generated/LaneSession";
import type { LaneState } from "@/lib/generated/LaneState";
import type { TellVerb } from "@/lib/generated/TellVerb";

import { useApp } from "../app-context";
import { Btn, TextInput, Toggle, errText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";
import { Badge, relative, type Tone } from "../ui";

const STATE_TONE: Record<LaneState, Tone> = {
  busy: "ok",
  idle: "idle",
  waiting: "accent",
  pending_input: "warn",
  rate_limited: "warn",
  missing: "bad",
};

/** States that need someone to look. */
const LOUD: LaneState[] = ["waiting", "pending_input", "rate_limited", "missing"];

/** A status file this old (minutes) is likely older than what the screen shows. */
const STALE_STATUS_MINS = 90;
const HISTORY_SHOWN = 8;

/** `81608` → `81.6k`, `200000` → `200k`. */
export const kTokens = (n: number) => {
  const k = n / 1000;
  return k >= 100 || Math.abs(k - Math.round(k)) < 0.05 ? `${Math.round(k)}k` : `${k.toFixed(1)}k`;
};

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

const clock = (iso: string) => {
  const d = new Date(iso);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getDate())}.${two(d.getMonth() + 1)} ${two(d.getHours())}:${two(d.getMinutes())}`;
};

/** A click-to-open block with a spaced-capital title. */
function Disclosure({ title, note, children, onOpen }: { title: string; note?: ReactNode; children: ReactNode; onOpen?: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex min-w-0 flex-col gap-2 border-t border-line pt-3">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => {
          if (!open) onOpen?.();
          setOpen(!open);
        }}
        className="flex items-center gap-1.5 self-start font-mono text-[10.5px] font-semibold tracking-[0.14em] text-text-2 uppercase hover:text-text"
      >
        {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
        {title}
        {note && <span className="font-normal tracking-normal text-muted normal-case">· {note}</span>}
      </button>
      {open && children}
    </div>
  );
}

function Meta({ k, children }: { k: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 gap-2 font-mono text-[11.5px] leading-snug">
      <span className="w-[52px] shrink-0 text-[10px] tracking-[0.08em] text-muted uppercase">{k}</span>
      <span className="min-w-0 break-words text-text-2">{children}</span>
    </div>
  );
}

function SessionRow({ s, now }: { s: LaneSession; now: number }) {
  const loud = LOUD.includes(s.state);
  return (
    <div className={clsx("flex min-w-0 flex-col gap-1 rounded-lg border bg-inset px-3 py-2.5", loud ? "border-warn/45" : "border-line")}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="font-mono text-[12.5px] font-semibold text-text">{s.name}</span>
        <Badge tone={STATE_TONE[s.state]} pulse={s.state === "busy"}>
          {s.state.replace("_", " ")}
        </Badge>
        {s.attached && <Badge tone="info">attached</Badge>}
        {s.context_pct !== null && (
          <span
            className={clsx("font-mono text-[11px]", s.context_pct >= 80 ? "text-warn" : "text-muted")}
            title={s.context_source === "transcript" ? "read from the Claude transcript (the last assistant message's usage)" : "printed by the CLI"}
          >
            {s.context_tokens !== null && s.context_window !== null ? `${kTokens(s.context_tokens)} / ${kTokens(s.context_window)} (${s.context_pct}%)` : `${s.context_pct}%`} context
          </span>
        )}
        {s.clear_hint && <span className="font-mono text-[11px] text-muted">suggests /clear</span>}
        <span className="font-mono text-[11px] text-muted">unchanged {s.unchanged_mins} min</span>
      </div>
      {s.why && <p className="font-mono text-[11.5px] leading-snug break-words text-text-2">{s.why}</p>}
      {s.rate_limit_line && (
        <p className="font-mono text-[11.5px] leading-snug break-words text-warn">
          {s.rate_limit_line}
          {s.rate_limit_resets && <span className="text-text-2"> · resets {s.rate_limit_resets}</span>}
        </p>
      )}
      {s.stall && <p className="rounded-md border border-warn/40 bg-warn/[.07] px-2 py-1 font-mono text-[11.5px] leading-snug break-words text-warn">{s.stall}</p>}
      {s.paused_until && <p className="font-mono text-[11px] text-info">paused until {clock(s.paused_until)} ({relative(s.paused_until, now)})</p>}
      {(s.last_clear || s.last_ask) && (
        <p className="font-mono text-[11px] text-muted">
          {s.last_clear && <>last /clear {relative(s.last_clear, now)}</>}
          {s.last_clear && s.last_ask && " · "}
          {s.last_ask && <>last ask {relative(s.last_ask, now)}</>}
        </p>
      )}
      {s.sub_agents.length > 0 && (
        <ul className="flex flex-col font-mono text-[11px] leading-snug text-muted">
          {s.sub_agents.map((a, i) => (
            <li key={i} className="truncate">
              ↳ {a}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Clear / Ask / Pause: a small inline form, then a queued tell. */
function TellForm({ lane, onDone }: { lane: LaneInfo; onDone: () => void }) {
  const { toast } = useApp();
  const [verb, setVerb] = useState<TellVerb | null>(null);
  const [note, setNote] = useState("");
  const [mins, setMins] = useState("60");
  const [session, setSession] = useState("");
  const [busy, setBusy] = useState(false);

  const pick = (v: TellVerb) => {
    setVerb(verb === v ? null : v);
    setNote("");
  };
  const minsN = Number(mins);
  const valid = verb !== "pause" || (Number.isInteger(minsN) && minsN > 0);

  const send = async () => {
    if (!verb || !valid) return;
    setBusy(true);
    try {
      await lanesApi.tell(lane.name, {
        verb,
        ...(note.trim() && verb !== "pause" ? { note: note.trim() } : {}),
        ...(verb === "pause" ? { mins: minsN } : {}),
        ...(session ? { session } : {}),
      });
      toast(`${lane.name}: ${verb} queued · done when the session is idle and nobody is attached`);
      setVerb(null);
      setNote("");
      onDone();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  const verbBtn = (v: TellVerb, label: string) => (
    <Btn size="sm" tone={verb === v ? "primary" : "ghost"} onClick={() => pick(v)} aria-pressed={verb === v}>
      {label}
    </Btn>
  );

  return (
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="mr-1 font-mono text-[10.5px] tracking-[0.1em] text-muted uppercase">tell</span>
        {verbBtn("clear", "Clear")}
        {verbBtn("ask", "Ask")}
        {verbBtn("pause", "Pause")}
      </div>
      {verb && (
        <form
          className="flex flex-col gap-2 rounded-lg border border-line bg-inset p-3"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          {verb === "pause" ? (
            <label className="flex items-center gap-2 font-mono text-[11.5px] text-text-2">
              no clear or ask for
              <TextInput value={mins} onChange={(e) => setMins(e.target.value)} inputMode="numeric" aria-label="Pause minutes" className={clsx("h-8 w-[80px]", !valid && "border-bad")} />
              min
            </label>
          ) : (
            <TextInput
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={verb === "clear" ? "note (optional, for the log)" : "text to type · empty: the status prompt"}
              aria-label={verb === "clear" ? "Note" : "Text to type"}
              className="h-8"
              autoFocus
            />
          )}
          <div className="flex flex-wrap items-center gap-2">
            {lane.sessions.length > 1 && (
              <select
                value={session}
                onChange={(e) => setSession(e.target.value)}
                aria-label="Session"
                className="h-7 rounded-md border border-line-2 bg-inset px-2 font-mono text-[11.5px] text-text outline-none focus:border-accent"
              >
                <option value="">any session (first idle)</option>
                {lane.sessions.map((s) => (
                  <option key={s.name} value={s.name}>
                    {s.name}
                  </option>
                ))}
              </select>
            )}
            <Btn size="sm" tone="primary" type="submit" disabled={busy || !valid}>
              {busy ? "Queuing…" : `Queue ${verb}`}
            </Btn>
            <Btn size="sm" tone="quiet" type="button" onClick={() => setVerb(null)}>
              Cancel
            </Btn>
          </div>
          {!lane.actions && verb !== "pause" && <p className="font-mono text-[11px] text-warn">Actions are off for this lane: it stays queued until you turn them on.</p>}
        </form>
      )}
    </div>
  );
}

function ScreenTail({ lane }: { lane: LaneInfo }) {
  const { now } = useApp();
  const names = lane.sessions.length ? lane.sessions.map((s) => s.name) : [null];
  const [screens, setScreens] = useState<Record<string, LaneScreen | { error: string }>>({});
  const [busy, setBusy] = useState(false);

  const load = async () => {
    setBusy(true);
    const got = await Promise.all(names.map((n) => lanesApi.screen(lane.name, n).catch((e: unknown) => ({ error: errText(e) }))));
    setScreens(Object.fromEntries(names.map((n, i) => [n ?? "", got[i]])));
    setBusy(false);
  };

  return (
    <Disclosure title="Screen" note={plural(names.length, "session")} onOpen={() => void load()}>
      <Btn size="sm" className="self-start" onClick={() => void load()} disabled={busy}>
        <RefreshCw className={clsx("size-3", busy && "animate-spin")} />
        Refresh
      </Btn>
      {names.map((n) => {
        const s = screens[n ?? ""];
        return (
          <div key={n ?? ""} className="flex min-w-0 flex-col gap-1">
            {n && names.length > 1 && <span className="font-mono text-[11px] text-text-2">{n}</span>}
            {!s ? (
              <p className="font-mono text-[11.5px] text-muted">loading…</p>
            ) : "error" in s ? (
              <p className="text-[12px] text-bad">{s.error}</p>
            ) : (
              <>
                <pre className="max-h-[420px] overflow-auto rounded-lg border border-line bg-inset px-3 py-2 font-mono text-[11px] leading-[1.45] whitespace-pre text-text-2">
                  {s.text || "(empty)"}
                </pre>
                {s.at && <span className="font-mono text-[10.5px] text-muted">captured {relative(s.at, now)}</span>}
              </>
            )}
          </div>
        );
      })}
    </Disclosure>
  );
}

function StatusPrompt({ lane, patch, busy }: { lane: LaneInfo; patch: (b: LanePatch, ok: string) => Promise<boolean>; busy: boolean }) {
  const [draft, setDraft] = useState<string | null>(null);
  const text = draft ?? lane.status_prompt;
  return (
    <Disclosure title="Status prompt" note={`${lane.status_prompt.length} chars`}>
      <p className="text-[12px] leading-normal text-text-2">What Ask types when the session sits idle. It is typed as one line: new lines become spaces.</p>
      <textarea
        rows={8}
        value={text}
        onChange={(e) => setDraft(e.target.value)}
        aria-label={`Status prompt of ${lane.name}`}
        className="w-full resize-y rounded-[7px] border border-line-2 bg-inset px-3 py-2.5 font-mono text-[12px] leading-[1.6] text-text outline-none focus:border-accent"
      />
      <div className="flex flex-wrap items-center gap-2">
        <Btn
          size="sm"
          tone="primary"
          disabled={busy || draft === null || draft === lane.status_prompt}
          onClick={() => void patch({ status_prompt: text }, `${lane.name}: status prompt saved`).then((ok) => ok && setDraft(null))}
        >
          Save prompt
        </Btn>
        <Btn size="sm" tone="quiet" disabled={draft === null} onClick={() => setDraft(null)}>
          Undo changes
        </Btn>
      </div>
    </Disclosure>
  );
}

function Thresholds({ lane }: { lane: LaneInfo }) {
  const c = lane.clear_when;
  const a = lane.ask_when;
  const s = lane.stall_when;
  return (
    <div className="flex min-w-0 flex-col gap-1 font-mono text-[11px] leading-snug text-muted">
      <span>
        <span className="text-text-2">clear</span> {c.enabled ? "on" : "off"} · idle {c.idle_mins}m · context ≥ {c.context_pct}% · cooldown {c.cooldown_mins}m
        {c.only_when_done && " · only when done"} · status ≤ {c.status_max_age_mins}m old
      </span>
      <span>
        <span className="text-text-2">ask</span> idle {a.idle_stale_mins ? `${a.idle_stale_mins}m` : "never"} · cooldown {a.cooldown_mins}m · after done {a.resume_cooldown_mins}m · status ≤{" "}
        {a.status_max_age_mins}m old · {a.max_sessions_per_round}/tick
      </span>
      <span>
        <span className="text-text-2">stall</span> after {s.stall_mins}m · repeat {s.repeat_mins}m
      </span>
    </div>
  );
}

/** A lane added on the dashboard: rename it (its history and settings move along) or remove it. Neither touches the
 * tmux session. */
function AddedActions({ lane, project, onReload }: { lane: LaneInfo; project: string; onReload: () => void }) {
  const { toast, go } = useApp();
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(lane.name);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);

  const run = async (what: () => Promise<unknown>, ok: string, after?: () => void) => {
    setBusy(true);
    try {
      await what();
      toast(ok);
      after?.();
      onReload();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };

  const valid = /^[a-z0-9._-]{1,48}$/.test(name.trim()) && name.trim() !== lane.name;
  return (
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Btn size="sm" disabled={busy} onClick={() => setRenaming(!renaming)} aria-expanded={renaming}>
          Rename
        </Btn>
        {confirm ? (
          <>
            <span className="font-mono text-[11.5px] text-text-2">Stop watching? The tmux session itself is left untouched.</span>
            <Btn size="sm" tone="danger" disabled={busy} onClick={() => void run(() => lanesApi.unwatch(project, lane.name), `${lane.name} removed · its tmux session is untouched`)}>
              Remove
            </Btn>
            <Btn size="sm" tone="quiet" disabled={busy} onClick={() => setConfirm(false)}>
              Keep
            </Btn>
          </>
        ) : (
          <Btn size="sm" tone="danger" disabled={busy} onClick={() => setConfirm(true)}>
            Remove
          </Btn>
        )}
      </div>
      {renaming && (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (!valid) return;
            const next = name.trim();
            void run(() => lanesApi.update(lane.name, { name: next }), `${lane.name} renamed to ${next}`, () => {
              setRenaming(false);
              go("lanes", next);
            });
          }}
        >
          <TextInput value={name} onChange={(e) => setName(e.target.value)} aria-label={`New name for ${lane.name}`} className={clsx("h-8 max-w-[260px]", name && !valid && name.trim() !== lane.name && "border-bad")} />
          <Btn size="sm" tone="primary" type="submit" disabled={busy || !valid}>
            Save name
          </Btn>
        </form>
      )}
    </div>
  );
}

export function LaneCard({ lane, onChange, onReload, focused }: { lane: LaneInfo; onChange: (l: LaneInfo) => void; onReload: () => void; focused: boolean }) {
  const { now, toast } = useApp();
  const [busy, setBusy] = useState("");
  const [allHistory, setAllHistory] = useState(false);

  const patch = async (body: LanePatch, ok: string) => {
    setBusy(Object.keys(body)[0]);
    try {
      onChange(await lanesApi.update(lane.name, body));
      toast(ok);
      return true;
    } catch (e) {
      toast(errText(e), "bad");
      return false;
    } finally {
      setBusy("");
    }
  };

  const cancel = async (verb: TellVerb, session: string) => {
    setBusy(`tell-${verb}-${session}`);
    try {
      await lanesApi.cancelTell(lane.name, verb, session);
      toast(`${lane.name}: ${verb} cancelled`);
      onReload();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy("");
    }
  };

  const owned = lane.owned !== null && lane.project !== null;
  /** A persistent lane: Start when stopped, Close (refused while someone uses it; Force from the owner). */
  const ownedAction = async (what: "start" | "close" | "force") => {
    if (!lane.project) return;
    setBusy(what);
    try {
      if (what === "start") await managerApi.startLane(lane.project, lane.name);
      else await managerApi.closeLane(lane.project, lane.name, what === "force");
      toast(`${lane.name}: ${what === "start" ? "started" : "closed"}`);
      onReload();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy("");
    }
  };

  const loud = lane.sessions.some((s) => LOUD.includes(s.state) || s.stall);
  const st = lane.status;
  const age = lane.status_age_mins;
  const git = lane.git;
  const tasks = lane.tasks;
  const history = allHistory ? lane.history : lane.history.slice(0, HISTORY_SHOWN);

  const toggle = (label: string, on: boolean, key: keyof LanePatch, sub: string) => (
    <label className="flex items-center gap-2">
      <Toggle on={on} disabled={!!busy} label={`${label} ${lane.name}`} onClick={() => void patch({ [key]: !on }, `${lane.name}: ${label} ${on ? "off" : "on"}`)} />
      <span className="flex flex-col">
        <span className="text-[12.5px] font-semibold text-text">{label}</span>
        <span className="font-mono text-[10.5px] text-muted">{sub}</span>
      </span>
    </label>
  );

  return (
    <Card className={clsx("flex flex-col gap-3.5 p-[18px]", loud && "border-warn/45", focused && "border-accent/60")}>
      <div id={`lane-${lane.name}`} className="flex min-w-0 scroll-mt-20 flex-wrap items-start justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-0.5">
          <h2 className="flex flex-wrap items-center gap-2 font-mono text-[15px] font-semibold text-text">
            {lane.role && <span className="font-mono text-[10px] font-medium tracking-[0.12em] text-accent-soft uppercase">{lane.role}</span>}
            <span title={lane.label !== lane.name ? lane.name : undefined}>{lane.label}</span>
            {owned && <Badge tone="accent">{lane.owned === "manager" ? "MAP manager" : "MAP lane"}</Badge>}
            {lane.added && <Badge tone="info">watched</Badge>}
            {lane.stopped && <Badge tone="bad">stopped</Badge>}
          </h2>
          <span className="font-mono text-[11px] break-all text-muted">
            {lane.label !== lane.name && `${lane.name} · `}
            {owned ? "MAP's tmux server" : lane.socket ? `tmux -L ${lane.socket}` : "default server"}
            {lane.path && ` · ${lane.path}`}
            {lane.session_match.length > 0 && ` · sessions ${lane.session_match.join(", ")}`}
          </span>
        </div>
        <span className="font-mono text-[10.5px] text-muted">{lane.checked_at ? `checked ${relative(lane.checked_at, now)}` : "not checked yet"}</span>
      </div>

      {owned && lane.owned === "lane" && (
        <div className="flex flex-wrap items-center gap-2">
          {lane.stopped ? (
            <>
              <span className="font-mono text-[11.5px] text-warn">The session is gone; MAP does not relaunch it by itself.</span>
              <Btn size="sm" tone="primary" disabled={!!busy} onClick={() => void ownedAction("start")}>
                Start
              </Btn>
            </>
          ) : (
            <>
              <Btn size="sm" disabled={!!busy} onClick={() => void ownedAction("close")}>
                Close lane
              </Btn>
              <Btn size="sm" tone="danger" disabled={!!busy} onClick={() => void ownedAction("force")}>
                Force close
              </Btn>
            </>
          )}
        </div>
      )}

      {lane.added && lane.project && <AddedActions lane={lane} project={lane.project} onReload={onReload} />}

      {lane.error && <p className="rounded-lg border border-bad/40 bg-bad/[.07] px-3 py-2 font-mono text-[11.5px] break-words text-bad">{lane.error}</p>}

      <div className="flex flex-wrap gap-x-6 gap-y-2.5">
        {toggle("watch", lane.watch, "watch", "notify; off: read only")}
        {toggle("actions", lane.actions, "actions", "may type into tmux")}
        {toggle("clear", lane.clear_when.enabled, "clear_enabled", "/clear when due")}
      </div>

      <div className="flex min-w-0 flex-col gap-2">
        {lane.sessions.length === 0 ? (
          <p className="font-mono text-[11.5px] text-muted">No matching tmux session.</p>
        ) : (
          lane.sessions.map((s) => <SessionRow key={s.name} s={s} now={now} />)
        )}
      </div>

      <div className="flex min-w-0 flex-col gap-1">
        <Meta k="status">
          {st ? (
            <>
              {st.card ?? "no card"}
              {st.state && <span className={st.state === "blocked" ? "text-warn" : st.state === "done" ? "text-ok" : "text-text"}> · {st.state}</span>}
              {st.need_clear && <span className="text-accent-soft"> · wants /clear</span>}
              {age !== null && <span className="text-muted"> · {age} min old</span>}
              {age !== null && age >= STALE_STATUS_MINS && <span className="text-warn"> · likely older than the screen</span>}
              {st.note && <span className="block text-muted">{st.note}</span>}
            </>
          ) : (
            <span className="text-muted">no status file</span>
          )}
        </Meta>
        {tasks && (
          <Meta k="cards">
            todo {tasks.todo} / todo-human {tasks.todo_human} / done {tasks.done}
            {tasks.last_done && <span className="text-muted"> · latest {tasks.last_done}</span>}
          </Meta>
        )}
        {git && (
          <Meta k="git">
            {git.branch} <span className="text-muted">{git.sha}</span> &quot;{git.subject}&quot; <span className="text-muted">({git.when})</span>
            {git.dirty > 0 && <span className="text-warn"> · dirty {plural(git.dirty, "file")}</span>}
            {git.ahead !== null && git.ahead > 0 && <span className="text-warn"> · unpushed {git.ahead}</span>}
            {git.behind !== null && git.behind > 0 && <span className="text-muted"> · behind {git.behind}</span>}
            {git.ahead === null && <span className="text-muted"> · no upstream</span>}
          </Meta>
        )}
      </div>

      <TellForm lane={lane} onDone={onReload} />

      {lane.tells.length > 0 && (
        <div className="flex min-w-0 flex-col gap-1.5">
          <SectionTitle note="done when idle, nobody attached">Queued</SectionTitle>
          {lane.tells.map((t) => (
            <div key={`${t.verb}-${t.session}`} className="flex min-w-0 items-start gap-2 rounded-md border border-line bg-inset px-2.5 py-1.5 font-mono text-[11.5px] leading-snug">
              <span className="min-w-0 flex-1 break-words">
                <span className="font-semibold text-accent-soft">{t.verb}</span>
                <span className="text-text-2"> · {t.session || "any session"}</span>
                {t.mins !== null && <span className="text-text-2"> · {t.mins} min</span>}
                {t.note && <span className="text-text"> · {t.note}</span>}
                <span className="block text-muted">
                  queued {relative(t.at, now)} · expires {relative(t.expires_at, now)}
                </span>
              </span>
              <button
                type="button"
                onClick={() => void cancel(t.verb, t.session)}
                disabled={!!busy}
                aria-label={`Cancel ${t.verb}`}
                className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted hover:bg-white/5 hover:text-bad disabled:opacity-40"
              >
                <X className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      )}

      {lane.history.length > 0 && (
        <div className="flex min-w-0 flex-col gap-1.5">
          <SectionTitle note={`${lane.history.length} ${lane.history.length === 1 ? "entry" : "entries"}`}>Last actions</SectionTitle>
          <ul className="flex min-w-0 flex-col gap-1">
            {history.map((h) => (
              <li key={h.id} className="flex min-w-0 gap-2 font-mono text-[11.5px] leading-snug">
                <span className="w-[78px] shrink-0 text-muted" title={h.at}>
                  {clock(h.at)}
                </span>
                <span className={clsx("w-[86px] shrink-0", h.kind === "stall" || h.kind === "rate_limited" ? "text-warn" : "text-accent-soft")}>{h.kind}</span>
                <span className="min-w-0 break-words text-text-2">
                  {h.session && <span className="text-muted">{h.session} · </span>}
                  {h.message}
                </span>
              </li>
            ))}
          </ul>
          {lane.history.length > HISTORY_SHOWN && (
            <button type="button" onClick={() => setAllHistory(!allHistory)} className="self-start font-mono text-[11px] text-link hover:underline">
              {allHistory ? "Show fewer" : `Show all ${lane.history.length}`}
            </button>
          )}
        </div>
      )}

      <ScreenTail lane={lane} />
      <StatusPrompt lane={lane} patch={patch} busy={!!busy} />
      <Disclosure title="Thresholds" note="map.toml">
        <Thresholds lane={lane} />
      </Disclosure>
    </Card>
  );
}
