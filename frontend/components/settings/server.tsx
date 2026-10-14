"use client";

/** Settings → Server: what `map.toml` set, read only (backend settings.rs). The API token shows only as set or not,
 * and nothing here edits the file: change it on the machine and restart MAP. */

import type { ReactNode } from "react";

import type { RunMode } from "@/lib/generated/RunMode";
import type { ServerSettings } from "@/lib/generated/ServerSettings";
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

/** The two folders an install is laid out around: the config folder (map.toml, and with the installer's layout the
 * data dir) and the project folder, the only place projects can be created (backend roots.rs). */
function Folders({ s }: { s: ServerSettings }) {
  const roots = s.project_roots;
  return (
    <Card className="flex flex-col gap-3.5 p-[18px]">
      <SectionTitle note="read only">Folders</SectionTitle>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex min-w-0 flex-col gap-1 rounded-lg border border-line bg-inset px-3.5 py-3">
          <span className="font-mono text-[10.5px] tracking-[0.1em] text-muted uppercase">Config folder</span>
          <span className="font-mono text-[13px] break-all text-text">{s.config_dir ?? <span className="text-muted">none (no config file)</span>}</span>
          <span className="font-mono text-[11px] break-all text-muted">map.toml · data {s.data_dir}</span>
        </div>
        {roots.map((r) => (
          <div key={r.name} className="flex min-w-0 flex-col gap-1 rounded-lg border border-line bg-inset px-3.5 py-3">
            <span className="font-mono text-[10.5px] tracking-[0.1em] text-muted uppercase">
              Project folder{roots.length > 1 ? ` · ${r.name}${r.is_default ? " (clone default)" : ""}` : ""}
            </span>
            <span className="font-mono text-[13px] break-all text-text">{r.path}</span>
            {r.host_path && <span className="font-mono text-[11px] break-all text-muted">on the host {r.host_path}</span>}
            <span className="font-mono text-[11px] text-muted">
              {r.exists ? <span className="text-ok">exists</span> : <span className="text-warn">does not exist yet</span>} ·{" "}
              {r.allow_root_itself ? "it or its sub-folders" : "projects only in its sub-folders"}
            </span>
          </div>
        ))}
      </div>
      <p className="text-[12px] leading-normal text-text-2">
        Projects can only be created in sub-folders of the project folder, and the New project wizard browses nothing else. It does not limit what an
        agent can read or run once started: that is the CLI&apos;s own permissions (or, in Docker, the container&apos;s mounts). To change the folders, run{" "}
        <code>deploy/install.sh --config-dir DIR --projects-dir DIR</code> again (or <code>--home DIR</code>), or set <code>[projects] path</code> in{" "}
        <code>map.toml</code> and restart MAP; in Docker, change <code>MAP_PROJECTS_DIR</code> in <code>.env</code> and the path in <code>map.toml</code> together.
      </p>
    </Card>
  );
}

export function ServerSettingsView({ settings, error }: { settings: SettingsInfo | null; error: string }) {
  const s = settings?.server;
  return (
    <div className="flex min-w-0 flex-col gap-3">
      {s && <Folders s={s} />}
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
