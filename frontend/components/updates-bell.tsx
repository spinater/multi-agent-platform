"use client";

import { Bell, RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { providerColor } from "@/lib/colors";
import type { Session } from "@/lib/generated/Session";
import type { UpdateInfo } from "@/lib/generated/UpdateInfo";
import { dur, providerName } from "@/lib/use-usage";
import { checkUpdatesNow, onNewUpdate, startUpdate, useUpdates } from "@/lib/use-updates";

import { useApp } from "./app-context";
import { Btn } from "./command-ui";
import { ConsoleModal } from "./console";
import { Modal } from "./shell/page";

const WIDTH = 380;

type Tone = "ok" | "warn" | "bad" | "info" | "idle";

const BADGE: Record<Tone, string> = {
  ok: "text-ok border-ok/35",
  warn: "text-warn border-warn/45",
  bad: "text-bad border-bad/50",
  info: "text-info border-info/45",
  idle: "text-muted border-line-2",
};

/** The row's state in a word: updating, update, error, up to date, or unknown (nothing to compare). */
export function updateBadge(p: UpdateInfo): { text: string; tone: Tone } {
  if (p.updating) return { text: "updating", tone: "info" };
  if (p.available) return { text: "update", tone: "warn" };
  if (p.error && !(p.installed && p.latest)) return { text: "error", tone: "bad" };
  if (p.installed && p.latest) return { text: "up to date", tone: "ok" };
  return { text: "unknown", tone: "idle" };
}

/** "2.1.299 → 2.1.300", "2.1.299", "—". */
export function versionLine(p: UpdateInfo): string {
  if (p.installed && p.latest && p.installed !== p.latest) return `${p.installed} → ${p.latest}`;
  return p.installed ?? (p.latest ? `? → ${p.latest}` : "—");
}

function Row({ p, now, onUpdate, onConsole }: { p: UpdateInfo; now: number; onUpdate: () => void; onConsole: (() => void) | null }) {
  const badge = updateBadge(p);
  const checked = p.checked_at ? now - new Date(p.checked_at).getTime() : null;
  const canUpdate = p.update_command.length > 0 && !p.update_blocked && !p.updating;
  // Up to date: no button. Unknown installed version (the check failed): still offered.
  const offerUpdate = p.update_command.length > 0 && (p.available || p.updating || p.installed === null);
  const why = p.updating
    ? "An update is running"
    : p.update_blocked ?? (p.update_command.length ? undefined : `No update command; set providers.${p.provider}.update in map.toml`);
  return (
    <div className="flex flex-col gap-1.5 border-b border-line pb-2.5">
      <div className="flex items-center gap-2">
        <span className="size-[7px] rounded-full" style={{ background: providerColor(p.provider) }} />
        <span className="text-[12.5px] leading-none font-semibold">{providerName(p.provider, true)}</span>
        <span className="truncate font-mono text-[11px] leading-none text-text-2">{versionLine(p)}</span>
        <span
          className={`ml-auto inline-flex h-[17px] flex-none items-center rounded border px-1.5 font-mono text-[9px] leading-none font-medium tracking-[0.08em] uppercase ${BADGE[badge.tone]}`}
        >
          {badge.text}
        </span>
      </div>
      <span className="font-mono text-[10px] leading-[1.3] text-muted">
        {p.version_command.length === 0 && !p.latest_url
          ? "no version command or latest source configured"
          : checked !== null
            ? `checked ${dur(checked, false)} ago`
            : "not checked yet"}
      </span>
      {p.error && <span className="font-mono text-[10.5px] leading-[1.35] break-words text-warn">{p.error}</span>}
      {p.update_blocked && <span className="font-mono text-[10.5px] leading-[1.35] break-words text-muted">{p.update_blocked}</span>}
      {(offerUpdate || onConsole) && (
        <div className="flex items-center gap-1.5">
          {offerUpdate && (
            <Btn size="sm" kind={p.available && canUpdate ? "primary" : "ghost"} disabled={!canUpdate} title={why} onClick={onUpdate}>
              {p.updating ? "Updating…" : "Update"}
            </Btn>
          )}
          {onConsole && (
            <Btn size="sm" kind="ghost" onClick={onConsole}>
              Console
            </Btn>
          )}
        </div>
      )}
    </div>
  );
}

function ConfirmUpdate({ p, onClose, onStarted }: { p: UpdateInfo; onClose: () => void; onStarted: (s: Session) => void }) {
  const { toast } = useApp();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const command = p.update_command.join(" ");
  const run = async () => {
    setBusy(true);
    setError("");
    try {
      const s = await startUpdate(p.provider);
      toast(`Updating ${providerName(p.provider, true)} in ${s.name}`);
      onStarted(s);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal onClose={onClose} label={`Update ${p.provider}`} width={480}>
      <div className="flex items-center gap-2 border-b border-line px-[18px] py-4 text-[15px] font-semibold">
        <span className="size-[9px] rounded-full" style={{ background: providerColor(p.provider) }} />
        Update {providerName(p.provider)}
      </div>
      <div className="flex flex-col gap-3 p-[18px] text-[13px] leading-normal text-text-2">
        <p>
          {p.installed && p.latest ? (
            <>
              Installed <span className="font-mono text-text">{p.installed}</span>, latest <span className="font-mono text-text">{p.latest}</span>.{" "}
            </>
          ) : null}
          This runs the command below in a helper tmux session, <span className="font-mono text-text">{p.session}</span>, and checks the version again when
          it ends. Running sessions keep the old version until they restart.
        </p>
        <pre className="overflow-x-auto rounded-lg border border-line bg-inset px-3 py-2 font-mono text-[12px] text-accent-soft">$ {command}</pre>
        {error && <p className="text-[12px] text-bad">{error}</p>}
      </div>
      <div className="flex justify-end gap-2 border-t border-line px-[18px] py-3.5">
        <Btn onClick={onClose}>Cancel</Btn>
        <Btn kind="primary" onClick={run} disabled={busy}>
          {busy ? "Starting…" : "Run update"}
        </Btn>
      </div>
    </Modal>
  );
}

/** The top bar's bell: the number of available CLI updates, and a popover with each provider's versions, an Update
 * button (confirmed, runs in `<prefix>update-<provider>`) and Check now. */
export function UpdatesBell() {
  const { now, sessions, toast } = useApp();
  const { report, checking, error } = useUpdates();
  const button = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const [confirm, setConfirm] = useState<UpdateInfo | null>(null);
  const [consoleOf, setConsoleOf] = useState<Session | null>(null);

  const providers = report?.providers ?? [];
  const count = providers.filter((p) => p.available).length;
  const checkedAt = report?.checked_at ? new Date(report.checked_at).getTime() : null;
  const live = (s: Session) => sessions.find((x) => x.name === s.name) ?? s;

  const toggle = () => {
    if (pos) return setPos(null);
    const r = button.current?.getBoundingClientRect();
    if (!r) return;
    const w = Math.min(WIDTH, window.innerWidth - 16);
    const left = r.left + r.width / 2 < window.innerWidth / 2 ? r.left : r.right - w;
    setPos({ top: r.bottom + 6, left: Math.max(8, Math.min(window.innerWidth - w - 8, left)) });
  };

  const title = count ? `${count} CLI update${count > 1 ? "s" : ""} available` : "Agent CLI updates";

  return (
    <div className="relative">
      <button
        ref={button}
        onClick={toggle}
        title={title}
        aria-label={title}
        aria-expanded={pos !== null}
        className={`relative flex h-7 cursor-pointer items-center gap-1.5 rounded-[7px] border px-2 ${count ? "border-warn/45 text-warn" : "border-line-2 text-muted hover:text-text"} ${pos ? "bg-white/5" : "bg-transparent"}`}
      >
        <Bell className="size-3.5" />
        {count > 0 && <span className="font-mono text-[11px] leading-none font-semibold">{count}</span>}
      </button>

      {pos &&
        createPortal(
          <>
            <div className="fixed inset-0 z-[60]" onClick={() => setPos(null)} />
            <div
              className="fixed z-[61] flex max-h-[calc(100vh-80px)] w-[380px] max-w-[calc(100vw-16px)] flex-col gap-3 overflow-auto rounded-xl border border-line-2 bg-menu p-3 shadow-[0_20px_50px_rgba(0,0,0,.5)]"
              style={{ top: pos.top, left: pos.left }}
            >
              <div className="flex items-center justify-between gap-2.5">
                <span className="font-mono text-[10px] leading-none font-medium tracking-[0.14em] text-muted uppercase">Updates · agent CLIs</span>
                <button
                  onClick={() => void checkUpdatesNow()}
                  disabled={checking}
                  className="flex h-6 cursor-pointer items-center gap-1.5 rounded-md border border-line-3 bg-transparent px-[9px] font-mono text-[10.5px] leading-none font-medium whitespace-nowrap text-nav disabled:opacity-50"
                >
                  <RotateCw className={`size-[10px] ${checking ? "animate-spin" : ""}`} />
                  {checking ? "checking…" : "Check now"}
                </button>
              </div>
              {providers.map((p) => {
                const existing = sessions.find((s) => s.name === p.session);
                return (
                  <Row
                    key={p.provider}
                    p={p}
                    now={now}
                    onUpdate={() => {
                      setPos(null);
                      setConfirm(p);
                    }}
                    onConsole={
                      existing
                        ? () => {
                            setPos(null);
                            setConsoleOf(existing);
                          }
                        : null
                    }
                  />
                );
              })}
              {providers.length === 0 && <span className="font-mono text-[10.5px] text-muted">{error ?? "loading…"}</span>}
              <span className="font-mono text-[10.5px] leading-none text-muted">
                {checkedAt !== null ? `checked ${dur(now - checkedAt)} ago` : report?.enabled === false ? "periodic checks off ([updates] enabled = false)" : "not checked yet"}
                {report?.enabled && ` · every ${dur(report.interval_secs * 1000, false)}`}
              </span>
            </div>
          </>,
          document.body,
        )}

      {/* On <body>, so the top bar's stacking context does not cap the dialogs. */}
      {confirm &&
        createPortal(
          <ConfirmUpdate
            p={confirm}
            onClose={() => setConfirm(null)}
            onStarted={(s) => {
              setConfirm(null);
              setConsoleOf(s);
            }}
          />,
          document.body,
        )}
      {consoleOf && createPortal(<ConsoleModal session={live(consoleOf)} readOnly={false} onClose={() => setConsoleOf(null)} toast={toast} />, document.body)}
    </div>
  );
}

/** Toasts each update that becomes available while the dashboard is open. Rendered once by the shell. */
export function UpdateNotifier() {
  const { toast } = useApp();
  useUpdates();
  useEffect(
    () => onNewUpdate((p) => toast(`${providerName(p.provider, true)} ${p.latest} is available (installed ${p.installed ?? "?"})`, "warn")),
    [toast],
  );
  return null;
}
