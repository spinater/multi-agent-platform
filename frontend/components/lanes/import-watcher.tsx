"use client";

/** The "Import agent-watcher config" form (`POST /api/lanes/import-watcher`): fills lane settings and the Telegram
 * chat from an agent-watcher `config.json` on this machine. */

import { useState } from "react";

import { lanesApi } from "@/lib/api";
import type { LaneImport } from "@/lib/generated/LaneImport";

import { Btn, TextInput, errText } from "../project-ui";

export function ImportWatcher({ onDone }: { onDone: () => void }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<LaneImport | null>(null);

  const submit = async () => {
    const p = path.trim();
    if (!p) return;
    setBusy(true);
    setError("");
    try {
      setResult(await lanesApi.importWatcher(p));
      onDone();
    } catch (e) {
      setError(errText(e));
      setResult(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-2.5">
      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label className="flex min-w-0 flex-1 basis-[280px] flex-col gap-1.5 font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">
          agent-watcher config.json
          <TextInput value={path} onChange={(e) => setPath(e.target.value)} placeholder="~/…/agent-watcher/config.json" aria-label="Path to the agent-watcher config.json" />
        </label>
        <Btn tone="primary" type="submit" disabled={busy || !path.trim()}>
          {busy ? "Importing…" : "Import"}
        </Btn>
      </form>
      <p className="font-mono text-[11px] leading-snug text-muted">
        Copies each watcher target&apos;s settings (status prompt, thresholds) into the lane with the same session, and the Telegram chat and users. Lanes
        themselves still come from map.toml.
      </p>
      {error && <p className="text-[12px] text-bad">{error}</p>}
      {result && (
        <div className="flex flex-col gap-1 rounded-lg border border-line bg-inset px-3 py-2.5 font-mono text-[11.5px] leading-snug">
          <span className={result.updated.length ? "text-ok" : "text-muted"}>
            updated {result.updated.length ? result.updated.join(", ") : "none"}
          </span>
          {result.skipped.map((s) => (
            <span key={s} className="break-words text-warn">
              skipped {s}
            </span>
          ))}
          {(result.chat_id !== null || result.allowed_user_ids.length > 0) && (
            <span className="text-text-2">
              telegram chat {result.chat_id ?? "unchanged"}
              {result.allowed_user_ids.length > 0 && ` · users ${result.allowed_user_ids.join(", ")}`}
            </span>
          )}
        </div>
      )}
    </div>
  );
}
