"use client";

/** Chat with a lane, as from Claude's phone app: the conversation from the lane's transcript, a message box that types
 * into its tmux session (a busy Claude Code lane runs it next), and the screen with its keys for an open dialog.
 * Polled while open; follows the bottom unless you scroll up. */

import clsx from "clsx";
import { MessageSquare, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { lanesApi } from "@/lib/api";
import type { ChatMessage } from "@/lib/generated/ChatMessage";
import type { LaneChat } from "@/lib/generated/LaneChat";
import type { LaneInfo } from "@/lib/generated/LaneInfo";

import { useApp } from "../app-context";
import { Btn, errText } from "../project-ui";
import { Badge, type Tone } from "../ui";

const TONE: Record<string, Tone> = { busy: "ok", idle: "idle", waiting: "accent", pending: "warn", missing: "bad" };
/** Keys the backend allows (`lane_chat::KEYS`), as a dialog needs them. */
const KEYS: [string, string][] = [
  ["Escape", "Esc"],
  ["1", "1"],
  ["2", "2"],
  ["3", "3"],
  ["Enter", "Enter"],
  ["Up", "↑"],
  ["Down", "↓"],
  ["BTab", "⇧Tab"],
];
const POLL_MS = 2000;

function Bubble({ m }: { m: ChatMessage }) {
  if (m.role === "tool") return <div className="truncate pl-1 font-mono text-[11px] text-muted">› {m.text}</div>;
  const owner = m.role === "owner";
  return (
    <div className={clsx("flex", owner ? "justify-end" : "justify-start")}>
      <div
        className={clsx(
          "max-w-[88%] rounded-[12px] px-3 py-2 text-[13px] leading-[1.45] break-words whitespace-pre-wrap",
          owner ? "bg-accent/20 text-text" : "border border-line bg-white/[0.03] text-text",
        )}
      >
        {m.text}
      </div>
    </div>
  );
}

function ChatPanel({ lane, onClose }: { lane: LaneInfo; onClose: () => void }) {
  const { toast } = useApp();
  const [chat, setChat] = useState<LaneChat | null>(null);
  const [session, setSession] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [sending, setSending] = useState(false);
  const [showScreen, setShowScreen] = useState(false);
  const [follow, setFollow] = useState(true);
  const [error, setError] = useState("");
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let stopped = false;
    const load = async () => {
      try {
        const got = await lanesApi.chat(lane.name, session);
        if (!stopped) {
          setChat(got);
          setError("");
        }
      } catch (e) {
        if (!stopped) setError(errText(e));
      }
    };
    void load();
    const timer = window.setInterval(() => {
      if (!document.hidden) void load();
    }, POLL_MS);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [lane.name, session]);

  useEffect(() => {
    if (follow && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [chat, follow]);

  const send = async (body: { text?: string; key?: string }) => {
    setSending(true);
    try {
      setChat(await lanesApi.sendChat(lane.name, { ...body, session: chat?.session ?? undefined }));
      if (body.text) setText("");
      setFollow(true);
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setSending(false);
    }
  };

  const noTranscript = chat !== null && chat.transcript === null;
  const screenOpen = showScreen || noTranscript || chat?.state === "waiting";
  return (
    <div className="fixed inset-0 z-50 flex justify-center bg-black/60 sm:p-4 sm:pt-[5vh]">
      <div role="dialog" aria-label={`Chat ${lane.label}`} className="flex h-full w-full max-w-[760px] flex-col border-line-2 bg-menu sm:h-[90vh] sm:rounded-[14px] sm:border">
        <div className="flex items-center gap-2 border-b border-line px-4 py-3">
          <span className="truncate font-mono text-[12.5px] font-semibold">{lane.label}</span>
          {chat && (
            <span title={chat.why}>
              <Badge tone={TONE[chat.state] ?? "idle"} pulse={chat.state === "busy"}>
                {chat.state}
              </Badge>
            </span>
          )}
          {chat?.attached && <span className="hidden font-mono text-[10.5px] text-muted sm:inline">attached in a terminal too</span>}
          {chat && chat.sessions.length > 1 && (
            <select
              value={chat.session ?? ""}
              onChange={(e) => setSession(e.target.value)}
              className="h-7 rounded-md border border-line-2 bg-inset px-1.5 font-mono text-[11px] text-text"
            >
              {chat.sessions.map((s) => (
                <option key={s}>{s}</option>
              ))}
            </select>
          )}
          <span className="flex-1" />
          <button onClick={onClose} className="size-[28px] flex-none cursor-pointer rounded-md border border-line-2 text-nav" aria-label="Close">
            <X className="mx-auto size-3.5" />
          </button>
        </div>

        <div
          ref={box}
          onScroll={(e) => {
            const el = e.currentTarget;
            setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 32);
          }}
          className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-4 py-3"
        >
          {!chat && !error && <p className="font-mono text-[11.5px] text-muted">loading…</p>}
          {error && <p className="font-mono text-[11.5px] text-bad">{error}</p>}
          {noTranscript && chat.state !== "missing" && <p className="font-mono text-[11px] text-muted">No Claude Code transcript for this session: the screen below is all there is.</p>}
          {chat?.state === "missing" && <p className="font-mono text-[11.5px] text-bad">{chat.why}</p>}
          {chat?.messages.map((m) => <Bubble key={m.id} m={m} />)}
        </div>

        {chat && chat.state !== "missing" && (
          <div className="flex flex-col gap-2 border-t border-line px-4 py-3">
            {screenOpen && chat.screen && (
              <pre className="max-h-[32vh] overflow-auto rounded-lg border border-line bg-term-bg p-2.5 font-mono text-[11px] leading-[1.4] whitespace-pre text-term-text">
                {chat.screen}
              </pre>
            )}
            <div className="flex flex-wrap items-center gap-[5px]">
              {KEYS.map(([k, label]) => (
                <button
                  key={k}
                  disabled={sending}
                  onClick={() => void send({ key: k })}
                  className="h-7 cursor-pointer rounded-[5px] border border-line-2 bg-white/[0.03] px-2 font-mono text-[11px] font-medium text-nav hover:border-line-3"
                  title={`Press ${k}`}
                >
                  {label}
                </button>
              ))}
              {!noTranscript && chat.state !== "waiting" && (
                <button onClick={() => setShowScreen(!showScreen)} className="ml-auto font-mono text-[11px] text-link hover:underline">
                  {showScreen ? "Hide screen" : "Screen"}
                </button>
              )}
            </div>
            <form
              className="flex items-end gap-1.5"
              onSubmit={(e) => {
                e.preventDefault();
                if (text.trim()) void send({ text });
              }}
            >
              <textarea
                value={text}
                rows={Math.min(6, Math.max(1, text.split("\n").length))}
                onChange={(e) => setText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                    e.preventDefault();
                    if (text.trim() && !sending) void send({ text });
                  }
                }}
                placeholder={chat.state === "waiting" ? "A dialog is open: answer it with the keys above" : `Message ${lane.label} (Enter sends, Shift+Enter new line)`}
                className="min-w-0 flex-1 resize-none rounded-[8px] border border-line-2 bg-inset px-2.5 py-2 text-[13px] text-text outline-none placeholder:text-muted/80 focus:border-accent"
              />
              <Btn tone="primary" type="submit" className="h-9" disabled={!text.trim() || sending}>
                Send
              </Btn>
            </form>
          </div>
        )}
      </div>
    </div>
  );
}

/** The lane card's Chat button and its panel. */
export function LaneChatButton({ lane }: { lane: LaneInfo }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Btn size="sm" type="button" className="self-start" onClick={() => setOpen(true)}>
        <MessageSquare className="size-3.5" /> Chat
      </Btn>
      {open && <ChatPanel lane={lane} onClose={() => setOpen(false)} />}
    </>
  );
}
