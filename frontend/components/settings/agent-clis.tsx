"use client";

/** Settings → Agent CLIs: per provider, the binary and where it is found, the installed and latest version (from the
 * update check), the sign-in state with the shared sign-in modal, and Update through the same flow as the bell. */

import { RotateCw } from "lucide-react";

import { providerColor } from "@/lib/colors";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { SettingsInfo } from "@/lib/generated/SettingsInfo";
import { checkUpdatesNow, updateOf, useUpdates } from "@/lib/use-updates";
import { dur, providerName } from "@/lib/use-usage";

import { useApp } from "../app-context";
import { Btn, signInText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";
import { BADGE, updateActions, updateBadge, useUpdateDialogs, versionLine } from "../updates-bell";

const TONE = { ok: "text-ok", warn: "text-warn", bad: "text-bad", muted: "text-muted" };

export function AgentClis({ settings }: { settings: SettingsInfo | null }) {
  const { providers, now, openAuth } = useApp();
  const { report, checking, error } = useUpdates();
  const flow = useUpdateDialogs();
  const checkedAt = report?.checked_at ? new Date(report.checked_at).getTime() : null;
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <Card className="flex flex-col p-[18px] pb-1">
        <div className="flex flex-wrap items-center justify-between gap-2 pb-3">
          <SectionTitle note={checkedAt !== null ? `versions checked ${dur(now - checkedAt, false)} ago` : report?.enabled === false ? "update checks off" : undefined}>
            Providers
          </SectionTitle>
          <Btn size="sm" tone="quiet" onClick={() => void checkUpdatesNow()} disabled={checking}>
            <RotateCw className={`size-3 ${checking ? "animate-spin" : ""}`} />
            {checking ? "Checking…" : "Check versions"}
          </Btn>
        </div>
        {providers.length === 0 && <p className="pb-3 font-mono text-[11.5px] text-muted">{error ?? "loading…"}</p>}
        {providers.map((p) => (
          <ProviderRow key={p.name} p={p} settings={settings} report={report} flow={flow} onSignIn={() => openAuth(p.name)} />
        ))}
      </Card>
      <p className="px-0.5 text-[12px] leading-normal text-muted">
        Binaries, versions, sign-in and update commands come from <code>[providers.&lt;name&gt;]</code> in <code>map.toml</code>. API keys are never taken here: set the
        provider&apos;s key variable in <code>[workspace.env]</code>.
      </p>
      {flow.dialogs}
    </div>
  );
}

function ProviderRow({
  p,
  settings,
  report,
  flow,
  onSignIn,
}: {
  p: ProviderInfo;
  settings: SettingsInfo | null;
  report: ReturnType<typeof useUpdates>["report"];
  flow: ReturnType<typeof useUpdateDialogs>;
  onSignIn: () => void;
}) {
  const bin = settings?.agents.find((a) => a.provider === p.name);
  const u = updateOf(report, p.name);
  const sign = signInText(p);
  const canSignIn = p.available && p.signed_in !== true && p.login.command.length > 0;
  const actions = u ? updateActions(u) : null;
  const helper = u ? flow.helper(u) : null;
  const badge = u ? updateBadge(u) : null;
  return (
    <div className="grid min-w-0 gap-x-5 gap-y-2 border-t border-line py-3.5 md:grid-cols-[minmax(0,1.1fr)_minmax(0,1fr)_minmax(0,1fr)_10.5rem] md:items-start">
      <div className="flex min-w-0 flex-col gap-1">
        <span className="flex items-center gap-2 text-[13.5px] font-semibold">
          <span className="size-2 shrink-0 rounded-full" style={{ background: providerColor(p.name) }} />
          {providerName(p.name)}
          <span className="font-mono text-[11px] font-normal text-muted">{p.name}</span>
        </span>
        <span className="font-mono text-[11px] break-all text-text-2">
          {bin ? (bin.path ?? <span className="text-bad">{bin.binary} · not on the agents&apos; PATH</span>) : (p.available ? "found" : "not found")}
        </span>
      </div>
      <div className="flex min-w-0 flex-col gap-1">
        <span className="label">version</span>
        <span className="flex flex-wrap items-center gap-2 font-mono text-[12px]">
          {u ? versionLine(u) : "—"}
          {badge && (
            <span className={`inline-flex h-[17px] items-center rounded border px-1.5 font-mono text-[9px] leading-none font-medium tracking-[0.08em] uppercase ${BADGE[badge.tone]}`}>
              {badge.text}
            </span>
          )}
        </span>
        {u?.error && <span className="font-mono text-[10.5px] leading-[1.35] break-words text-warn">{u.error}</span>}
        {u?.update_blocked && <span className="font-mono text-[10.5px] leading-[1.35] break-words text-muted">{u.update_blocked}</span>}
      </div>
      <div className="flex min-w-0 flex-col gap-1">
        <span className="label">sign-in</span>
        <span className={`font-mono text-[12px] ${TONE[sign.tone]}`}>{sign.text}</span>
        {p.login.api_key_env && (
          <span className="font-mono text-[10.5px] text-muted">
            or {p.login.api_key_env} in [workspace.env] · {p.login.api_key_set ? <span className="text-ok">set</span> : "not set"}
          </span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-1.5 md:justify-end md:pt-3">
        {canSignIn && (
          <Btn size="sm" tone={p.signed_in === false ? "primary" : "ghost"} onClick={onSignIn}>
            Sign in
          </Btn>
        )}
        {u && actions?.offerUpdate && (
          <Btn size="sm" tone={u.available && actions.canUpdate ? "primary" : "ghost"} disabled={!actions.canUpdate} title={actions.why} onClick={() => flow.ask(u)}>
            {u.updating ? "Updating…" : "Update"}
          </Btn>
        )}
        {helper && (
          <Btn size="sm" tone="quiet" onClick={() => flow.watch(helper)}>
            Console
          </Btn>
        )}
      </div>
    </div>
  );
}
