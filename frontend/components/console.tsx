"use client";

import { Send, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { ApiError, api } from "@/lib/api";
import type { Session } from "@/lib/generated/Session";

import { Button, Empty, Input, Panel, SessionBadge } from "./ui";

const KEYS: [string, string][] = [
  ["C-c", "Ctrl-C"],
  ["Escape", "Esc"],
  ["Enter", "Enter"],
  ["Up", "↑"],
  ["Down", "↓"],
  ["y", "y"],
  ["n", "n"],
];

/** Live view of one pane plus the controls an operator needs: type a prompt, press a key, close the session. */
export function SessionConsole({ session, onClosed }: { session: Session | null; onClosed: () => void }) {
  const [text, setText] = useState("");
  const [pane, setPane] = useState("");
  const [message, setMessage] = useState("");
  const [follow, setFollow] = useState(true);
  const box = useRef<HTMLPreElement>(null);
  const name = session?.name;
  const live = session ? session.state !== "exited" : false;

  useEffect(() => {
    if (!name) return;
    let stopped = false;
    const load = async () => {
      try {
        const res = await api.pane(name);
        if (!stopped) setPane(res.text.replace(/\s+$/, ""));
      } catch {
        // Keep the last capture on a transient error.
      }
    };
    void load();
    // Exited sessions show their saved final pane once; live ones refresh.
    const timer = live ? window.setInterval(load, 1500) : undefined;
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [name, live]);

  useEffect(() => {
    if (follow && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [pane, follow]);

  if (!session) {
    return (
      <Panel title="Console">
        <Empty>Select a session to watch its terminal.</Empty>
      </Panel>
    );
  }

  const run = async (action: () => Promise<unknown>, ok?: string) => {
    setMessage("");
    try {
      await action();
      if (ok) setMessage(ok);
    } catch (err) {
      setMessage((err as Error).message);
      throw err;
    }
  };

  const send = async () => {
    if (!text.trim()) return;
    await run(() => api.prompt(session.name, text)).then(() => setText("")).catch(() => {});
  };

  const close = async () => {
    const verb = live ? "Close" : "Remove";
    if (!confirm(`${verb} ${session.name}?`)) return;
    try {
      await api.deleteSession(session.name);
      onClosed();
    } catch (err) {
      if (err instanceof ApiError && err.status === 409 && confirm(`${err.message}.\n\nClose it anyway?`)) {
        await run(() => api.deleteSession(session.name, true)).then(onClosed).catch(() => {});
      } else {
        setMessage((err as Error).message);
      }
    }
  };

  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <span className="font-mono">{session.name}</span>
          <SessionBadge state={session.state} />
        </span>
      }
      actions={
        <Button size="sm" variant="danger" onClick={close}>
          <Trash2 className="size-3.5" /> {live ? "Close" : "Remove"}
        </Button>
      }
    >
      <div className="flex flex-col gap-2 p-3">
        <pre
          ref={box}
          onScroll={(e) => {
            const el = e.currentTarget;
            setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
          }}
          className="h-[420px] overflow-auto rounded-md bg-term-bg p-3 font-mono text-[12px] leading-[1.45] whitespace-pre text-term-text"
        >
          {pane || "(empty pane)"}
        </pre>
        {live && (
          <>
            <form
              className="flex gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                void send();
              }}
            >
              <Input value={text} onChange={(e) => setText(e.target.value)} placeholder={`Message ${session.provider}…`} />
              <Button variant="primary" type="submit" disabled={!text.trim()}>
                <Send className="size-3.5" /> Send
              </Button>
            </form>
            <div className="flex flex-wrap gap-1.5">
              {KEYS.map(([key, label]) => (
                <Button key={key} size="sm" onClick={() => run(() => api.key(session.name, key)).catch(() => {})}>
                  {label}
                </Button>
              ))}
            </div>
          </>
        )}
        {message && <p className="text-xs text-bad">{message}</p>}
        <p className="text-[11px] text-muted">
          Attach from a terminal: <code className="font-mono">{`tmux attach -t '=${session.name}'`}</code>
        </p>
      </div>
    </Panel>
  );
}
