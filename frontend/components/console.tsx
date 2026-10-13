"use client";

import { useEffect, useRef, useState } from "react";

import { ApiError, api } from "@/lib/api";
import type { Session } from "@/lib/generated/Session";

import { Btn, Pill, SESSION_TONE } from "./command-ui";
import { Modal } from "./shell/page";

/** Keys the API allows (`POST /sessions/{name}/key`). */
export const KEYS: [string, string][] = [
  ["C-c", "C-c"],
  ["Escape", "Esc"],
  ["Enter", "Enter"],
  ["Up", "↑"],
  ["Down", "↓"],
  ["Tab", "Tab"],
  ["y", "y"],
  ["n", "n"],
];

/** The session's pane text: polled while the session is live, the saved final pane once it has exited. */
export function usePane(name: string | null, live: boolean, lines = 400): string {
  const [pane, setPane] = useState({ name: "", text: "" });
  useEffect(() => {
    if (!name) return;
    let stopped = false;
    const load = async () => {
      try {
        const res = await api.pane(name, lines);
        if (!stopped) setPane({ name, text: res.text.replace(/\s+$/, "") });
      } catch {
        // Keep the last capture on a transient error.
      }
    };
    void load();
    const timer = live ? window.setInterval(load, 1500) : undefined;
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [name, live, lines]);
  return pane.name === name ? pane.text : "";
}

/** Closes a live session (asking again when a person is using it) or forgets an exited one. */
export async function closeSession(s: Session, toast: (text: string, tone?: "ok" | "warn" | "bad") => void): Promise<boolean> {
  const live = s.state !== "exited";
  if (!confirm(`${live ? "Close" : "Remove"} ${s.name}?`)) return false;
  try {
    const res = await api.deleteSession(s.name);
    toast(res.message);
    return true;
  } catch (err) {
    if (err instanceof ApiError && err.status === 409 && confirm(`${err.message}.\n\nClose it anyway?`)) {
      try {
        const res = await api.deleteSession(s.name, true);
        toast(res.message, "warn");
        return true;
      } catch (e) {
        toast((e as Error).message, "bad");
        return false;
      }
    }
    toast((err as Error).message, "bad");
    return false;
  }
}

/** Type to the agent (Enter sends) and press its keys. */
export function PromptBar({ session, toast }: { session: Session; toast: (text: string, tone?: "ok" | "warn" | "bad") => void }) {
  const [text, setText] = useState("");
  const send = async () => {
    const t = text.trim();
    if (!t) return;
    try {
      await api.prompt(session.name, t);
      setText("");
    } catch (err) {
      toast((err as Error).message, "bad");
    }
  };
  const key = (k: string) => api.key(session.name, k).catch((err: Error) => toast(err.message, "bad"));
  return (
    <div className="flex flex-col gap-2">
      <form
        className="flex gap-1.5"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Type to the agent, Enter to send"
          className="h-8 min-w-0 flex-1 rounded-[7px] border border-line-2 bg-inset px-2.5 font-mono text-[12px] text-text outline-none placeholder:text-muted/80 focus:border-accent"
        />
        <Btn kind="primary" type="submit" className="h-8" disabled={!text.trim()}>
          Send
        </Btn>
      </form>
      <div className="flex flex-wrap gap-[5px]">
        {KEYS.map(([k, label]) => (
          <button
            key={k}
            onClick={() => void key(k)}
            className="h-6 cursor-pointer rounded-[5px] border border-line-2 bg-white/[0.03] px-2 font-mono text-[10.5px] font-medium text-nav hover:border-line-3"
            title={`Send ${k}`}
          >
            {label}
          </button>
        ))}
      </div>
    </div>
  );
}

/** The whole terminal of a session in a modal: follows the bottom unless you scroll up. */
export function ConsoleModal({ session, readOnly, onClose, toast }: { session: Session; readOnly: boolean; onClose: () => void; toast: (text: string, tone?: "ok" | "warn" | "bad") => void }) {
  const live = session.state !== "exited";
  const pane = usePane(session.name, live, 1000);
  const [follow, setFollow] = useState(true);
  const box = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (follow && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [pane, follow]);
  return (
    <Modal onClose={onClose} label={`Console ${session.name}`} width={920}>
      <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-3.5">
        <span className="flex min-w-0 items-center gap-2.5">
          <span className="truncate font-mono text-[12.5px] font-semibold">{session.name}</span>
          <Pill tone={SESSION_TONE[session.state]}>{session.state.replace("_", " ")}</Pill>
          <span className="hidden truncate font-mono text-[10.5px] text-muted sm:inline">
            {live ? `tmux attach -t '=${session.name}'` : "final pane"}
          </span>
        </span>
        <button onClick={onClose} className="size-[26px] flex-none cursor-pointer rounded-md border border-line-2 text-nav" aria-label="Close">
          ×
        </button>
      </div>
      <div className="flex flex-col gap-3 p-[18px]">
        <pre
          ref={box}
          onScroll={(e) => {
            const el = e.currentTarget;
            setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
          }}
          className="h-[min(60vh,560px)] overflow-auto rounded-lg border border-line bg-term-bg p-3 font-mono text-[12px] leading-[1.45] whitespace-pre text-term-text"
        >
          {pane || "(empty pane)"}
        </pre>
        {live && !readOnly && <PromptBar session={session} toast={toast} />}
      </div>
    </Modal>
  );
}
