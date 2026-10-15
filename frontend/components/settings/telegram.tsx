"use client";

/** Settings → Telegram (backend telegram.rs): lane reports and `/watch`, `/status` commands in one chat. Read only:
 * it all comes from map.toml's `[telegram]`. The bot token only shows as set or not, never its value. */

import clsx from "clsx";
import { Send } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";

import { lanesApi } from "@/lib/api";
import type { TelegramInfo } from "@/lib/generated/TelegramInfo";

import { useApp } from "../app-context";
import { Btn, errText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";
import { relative } from "../ui";
import { YesNo } from "./git-ssh";

const REFRESH_MS = 30_000;

function Row({ k, children }: { k: string; children: ReactNode }) {
  return (
    <>
      <dt className="pt-px font-mono text-[10.5px] tracking-[0.08em] text-muted uppercase">{k}</dt>
      <dd className="min-w-0 font-mono text-[12px] break-all text-text">{children}</dd>
    </>
  );
}

export function TelegramSettings() {
  const { now } = useApp();
  const [info, setInfo] = useState<TelegramInfo | null>(null);
  const [error, setError] = useState("");
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; message: string } | null>(null);

  const load = useCallback(() => {
    lanesApi.telegram().then(
      (t) => {
        setInfo(t);
        setError("");
      },
      (e) => setError(errText(e)),
    );
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const sendTest = async () => {
    setTesting(true);
    try {
      setTest(await lanesApi.telegramTest());
    } catch (e) {
      setTest({ ok: false, message: errText(e) });
    } finally {
      setTesting(false);
      load();
    }
  };

  if (!info) {
    return <Card className="p-[18px]">{error ? <p className="text-[12px] text-bad">{error}</p> : <p className="font-mono text-[11.5px] text-muted">loading…</p>}</Card>;
  }

  const t = info;
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {error && <p className="text-[12px] text-bad">{error}</p>}
      <Card className="flex flex-col gap-3 p-[18px]">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <SectionTitle note="read only">[telegram]</SectionTitle>
          <Btn size="sm" onClick={() => void sendTest()} disabled={testing || !t.enabled || !t.token_set}>
            <Send className="size-3" />
            {testing ? "Sending…" : "Send test"}
          </Btn>
        </div>
        {test && <p className={clsx("font-mono text-[12px] break-words", test.ok ? "text-ok" : "text-bad")}>{test.message}</p>}
        <dl className="grid grid-cols-[minmax(0,7.5rem)_minmax(0,1fr)] gap-x-4 gap-y-2 sm:grid-cols-[11rem_minmax(0,1fr)]">
          <Row k="enabled">
            <YesNo on={t.enabled} warn />
          </Row>
          <Row k="bot token">
            <YesNo on={t.token_set} yes="set" no="not set" warn />
            <span className="text-muted">
              {" "}
              · read from ${t.token_env}
              {t.env_file && <> or {t.env_file}</>} · the value is never shown
            </span>
          </Row>
          <Row k="chat id">{t.chat_id || <span className="text-warn">not set</span>}</Row>
          <Row k="allowed users">{t.allowed_user_ids.length ? t.allowed_user_ids.join(", ") : <span className="text-muted">anyone in the chat</span>}</Row>
          <Row k="commands">
            <YesNo on={t.commands} yes="on" no="off" />
            <span className="text-muted"> · /watch, /status</span>
            {t.commands && (
              <>
                {" · "}
                <YesNo on={t.polling} yes="listening" no="not connected (retrying)" warn />
              </>
            )}
          </Row>
          <Row k="last send">
            {t.last_send_ok === null ? (
              <span className="text-muted">nothing sent yet</span>
            ) : (
              <>
                <YesNo on={t.last_send_ok} yes="ok" no="failed" warn />
                {t.last_sent_at && <span className="text-muted"> · {relative(t.last_sent_at, now)}</span>}
              </>
            )}
          </Row>
          {t.last_error && (
            <Row k="last error">
              <span className="text-bad">{t.last_error}</span>
            </Row>
          )}
          <Row k="report">
            every hour at :{String(t.report_minute).padStart(2, "0")}
            <span className="text-muted"> · last {t.last_report_at ? relative(t.last_report_at, now) : "never"}</span>
          </Row>
        </dl>
      </Card>
      <p className="text-[12px] leading-normal text-text-2">
        To change these, edit <code>[telegram]</code> in <code>map.toml</code> and restart MAP; the chat and the users can also come from an agent-watcher
        config (Lanes → Import watcher settings). Put the token in the environment (or the env file), never in map.toml. Only one process per bot token
        may listen for commands.
      </p>
    </div>
  );
}
