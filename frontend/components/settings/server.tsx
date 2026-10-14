"use client";

/** Settings → Server: what `map.toml` set, read only (backend settings.rs). The API token shows only as set or not,
 * and nothing here edits the file: change it on the machine and restart MAP. */

import type { ReactNode } from "react";

import type { RunMode } from "@/lib/generated/RunMode";
import type { SettingsInfo } from "@/lib/generated/SettingsInfo";

import { Card, SectionTitle } from "../shell/page";
import { YesNo } from "./git-ssh";

const RUN_MODE: Record<RunMode, string> = {
  docker: "in a container (Docker Compose)",
  systemd: "as a systemd service",
  launchd: "as a launchd agent",
  manual: "by hand (map serve)",
};

const RESTART_NOTE: Record<RunMode, string> = {
  docker: "in the folder with docker-compose.yml; this ends the agent sessions in the container",
  systemd: "agent sessions in tmux keep running",
  launchd: "agent sessions in tmux keep running",
  manual: "stop the running server with Ctrl-C first; agent sessions in tmux keep running",
};

const ALL_RESTARTS = "$ docker compose restart map\n$ systemctl --user restart map\n$ launchctl kickstart -k gui/$(id -u)/com.multi-agent-platform.map";

function Row({ k, children }: { k: string; children: ReactNode }) {
  return (
    <>
      <dt className="pt-px font-mono text-[10.5px] tracking-[0.08em] text-muted uppercase">{k}</dt>
      <dd className="min-w-0 font-mono text-[12px] break-all text-text">{children}</dd>
    </>
  );
}

export function ServerSettingsView({ settings, error }: { settings: SettingsInfo | null; error: string }) {
  const s = settings?.server;
  return (
    <div className="flex min-w-0 flex-col gap-3">
      <Card className="flex flex-col gap-3 p-[18px]">
        <SectionTitle note="read only">map.toml</SectionTitle>
        {error && <p className="text-[12px] text-bad">{error}</p>}
        {!s ? (
          !error && <p className="font-mono text-[11.5px] text-muted">loading…</p>
        ) : (
          <dl className="grid grid-cols-[minmax(0,7.5rem)_minmax(0,1fr)] gap-x-4 gap-y-2 sm:grid-cols-[11rem_minmax(0,1fr)]">
            <Row k="config file">{s.config_path ?? <span className="text-muted">none</span>}</Row>
            <Row k="data dir">{s.data_dir}</Row>
            <Row k="bind">{s.bind}</Row>
            <Row k="allowed hosts">{s.allowed_hosts.length ? s.allowed_hosts.join(", ") : <span className="text-muted">none (loopback and the bound address only)</span>}</Row>
            <Row k="api token">
              <YesNo on={s.api_token_set} yes="set" no="not set" warn />
              <span className="text-muted"> · the value is never shown</span>
            </Row>
            <Row k="projects root">{s.projects_root}</Row>
            <Row k="default workspace">{s.workspace_dir}</Row>
            <Row k="session prefix">{s.session_prefix}</Row>
            <Row k="static dir">
              {s.static_dir} · <YesNo on={s.static_dir_ok} yes="dashboard found" no="no index.html" warn />
            </Row>
            <Row k="MAP version">{s.version}</Row>
            <Row k="Reflex">
              {s.reflex_binary} · {s.reflex_version ?? <span className="text-warn">not installed</span>}
            </Row>
            <Row k="running">{RUN_MODE[s.run_mode]}</Row>
          </dl>
        )}
      </Card>
      <div className="rounded-xl border border-line bg-surface px-[18px] py-3.5 text-[12.5px] leading-normal text-text-2">
        These live in <code>map.toml</code> and cannot be changed from the dashboard. Edit the file on the machine, then restart MAP
        {s ? <> ({RESTART_NOTE[s.run_mode]}):</> : ":"}
        <pre className="mt-2 overflow-x-auto rounded-lg border border-line bg-inset px-3 py-2 font-mono text-[11.5px] whitespace-pre-wrap text-accent-soft">
          {s ? `$ ${s.restart_hint}` : ALL_RESTARTS}
        </pre>
        {s?.run_mode === "manual" && (
          <p className="mt-2 text-[12px] text-muted">
            As a service: <code>systemctl --user restart map</code> (Linux, <code>deploy/install.sh</code>) or <code>launchctl kickstart -k gui/$(id -u)/com.multi-agent-platform.map</code>{" "}
            (macOS); with Docker Compose: <code>docker compose restart map</code>.
          </p>
        )}
      </div>
    </div>
  );
}
