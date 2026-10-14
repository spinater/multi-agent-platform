"use client";

/** Settings → Git & SSH (backend git_access.rs, settings.rs): which SSH setup git uses (MAP's own key or this
 * machine's `~/.ssh` and ssh-agent), MAP's key, how to add a key on GitHub, GitLab.com or a self-hosted host with a
 * test for each, MAP's known hosts, the git identity, and whether `gh` / `glab` can sign in. Only public keys are
 * ever shown; the dashboard never takes or shows a private key, token or password. */

import clsx from "clsx";
import { Check as CheckIcon, Copy, KeyRound } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";

import { gitApi } from "@/lib/api";
import type { EnvVarState } from "@/lib/generated/EnvVarState";
import type { GitAccess } from "@/lib/generated/GitAccess";
import type { GitTestResult } from "@/lib/generated/GitTestResult";
import type { KnownHost } from "@/lib/generated/KnownHost";
import type { SettingsInfo } from "@/lib/generated/SettingsInfo";
import type { SshMethod } from "@/lib/generated/SshMethod";
import type { SystemSsh } from "@/lib/generated/SystemSsh";

import { useApp } from "../app-context";
import { Btn, Field, Inset, Segmented, TextInput, errText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";

/** MAP's git access, loaded once per mount; `reload` after a change. */
export function useGitAccess(): { access: GitAccess | null; error: string; reload: () => void } {
  const [access, setAccess] = useState<GitAccess | null>(null);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);
  useEffect(() => {
    let stale = false;
    gitApi.access().then(
      (a) => {
        if (stale) return;
        setAccess(a);
        setError("");
      },
      (e) => !stale && setError(errText(e)),
    );
    return () => {
      stale = true;
    };
  }, [version]);
  return { access, error, reload: useCallback(() => setVersion((v) => v + 1), []) };
}

// ---- small pieces

export function Step({ n, children }: { n: number; children: ReactNode }) {
  return (
    <li className="flex gap-2.5">
      <span className="mt-px flex size-5 shrink-0 items-center justify-center rounded-full border border-accent font-mono text-[10px] font-semibold text-accent-soft">{n}</span>
      <span className="min-w-0">{children}</span>
    </li>
  );
}

/** A yes/no fact in words and colour. */
export function YesNo({ on, yes = "yes", no = "no", warn = false }: { on: boolean; yes?: string; no?: string; warn?: boolean }) {
  return <span className={on ? "text-ok" : warn ? "text-warn" : "text-muted"}>{on ? yes : no}</span>;
}

function Note({ tone = "warn", children }: { tone?: "warn" | "info"; children: ReactNode }) {
  return (
    <div
      className={clsx(
        "rounded-lg border px-3 py-2 text-[12.5px] leading-normal",
        tone === "warn" ? "border-warn/35 bg-warn/[.07] text-warn" : "border-info/30 bg-info/[.06] text-text-2",
      )}
    >
      {children}
    </div>
  );
}

const B = ({ children }: { children: ReactNode }) => <b className="font-semibold text-text">{children}</b>;

// ---- the section

/** Everything git needs, as one column of cards. `settings` (from `GET /api/settings`) adds the gh / glab card;
 * leave it out where only the key matters (the wizard's dialog). */
export function GitSshSettings({ settings, showCli = false }: { settings?: SettingsInfo | null; showCli?: boolean }) {
  const { access, error, reload } = useGitAccess();
  const method: SshMethod = access?.settings.ssh ?? "map_key";
  return (
    <div className="@container flex min-w-0 flex-col gap-4">
      {error && <p className="text-[12px] text-bad">{error}</p>}
      {access && !access.ssh_available && (
        <Note>ssh or ssh-keygen is not installed where MAP runs (package openssh-client), so git cannot use SSH at all.</Note>
      )}
      <MethodCard access={access} onChanged={reload} />
      {method === "map_key" ? <MapKeyCard access={access} onChanged={reload} /> : access && <SystemSshCard ssh={access.system_ssh} />}
      <HostCards access={access} onTested={reload} />
      <KnownHostsCard access={access} onChanged={reload} />
      <IdentityCard key={access ? `${access.identity.name}|${access.identity.email}` : "loading"} access={access} onSaved={reload} />
      {showCli && <GitCliCard settings={settings ?? null} />}
      {access && <HowItWorks access={access} />}
    </div>
  );
}

// ---- SSH method

function MethodCard({ access, onChanged }: { access: GitAccess | null; onChanged: () => void }) {
  const { toast } = useApp();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const current = access?.settings.ssh ?? null;
  const ssh = access?.system_ssh;
  const pick = async (ssh: SshMethod) => {
    if (ssh === current || busy) return;
    setBusy(true);
    setError("");
    try {
      await gitApi.setSettings({ ssh });
      toast(ssh === "system" ? "git now uses this machine's SSH setup" : "git now uses MAP's own key");
      onChanged();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card className="flex flex-col gap-3 p-[18px]">
      <SectionTitle note="for agents and MAP's own git commands">SSH method</SectionTitle>
      <div role="radiogroup" aria-label="SSH method" className="grid gap-2.5 @xl:grid-cols-2">
        <MethodOption
          on={current === "map_key"}
          disabled={!access || busy}
          onClick={() => pick("map_key")}
          title="MAP's own key"
          tag="default"
          sub={
            <>
              An ed25519 key in MAP&apos;s data folder, used alone with MAP&apos;s own known_hosts. Works the same as a service and in Docker, with no <code>~/.ssh</code> mount.
            </>
          }
        />
        <MethodOption
          on={current === "system"}
          disabled={!access || busy}
          onClick={() => pick("system")}
          title="This machine's SSH setup"
          tag="~/.ssh, ssh-agent"
          sub={
            <>
              Plain <code>ssh</code> with your keys, <code>~/.ssh/config</code>, known_hosts and ssh-agent, as in your shell. MAP&apos;s gitconfig then sets no <code>sshCommand</code>{" "}
              (it keeps the identity and includes).
            </>
          }
        />
      </div>
      {ssh?.in_container && !ssh.dir_exists && (
        <Note>
          MAP runs in a container, and <code>{ssh.dir}</code> is not mounted there, so &quot;This machine&apos;s SSH setup&quot; finds no keys. Keep MAP&apos;s own key, or mount your{" "}
          <code>~/.ssh</code> read-only as the comment about it in <code>docker-compose.yml</code> shows (<code>- $&#123;MAP_HOME&#125;/.ssh:$&#123;MAP_HOME&#125;/.ssh:ro</code>), then{" "}
          <code>docker compose up -d</code>.
        </Note>
      )}
      {error && <p className="text-[12px] text-bad">{error}</p>}
    </Card>
  );
}

function MethodOption({ on, disabled, onClick, title, tag, sub }: { on: boolean; disabled: boolean; onClick: () => void; title: string; tag: string; sub: ReactNode }) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={on}
      disabled={disabled}
      onClick={onClick}
      className={clsx(
        "flex min-w-0 items-start gap-3 rounded-[10px] border p-3.5 text-left transition-colors disabled:cursor-default",
        on ? "border-accent bg-accent-tint" : "border-line-2 hover:border-line-3",
      )}
    >
      <span className={clsx("mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border", on ? "border-accent" : "border-line-3")}>
        {on && <span className="size-2 rounded-full bg-accent" />}
      </span>
      <span className="flex min-w-0 flex-col gap-1">
        <span className="flex flex-wrap items-baseline gap-x-2 text-[13.5px] font-semibold text-text">
          {title}
          <span className="font-mono text-[10.5px] font-normal text-muted">{tag}</span>
        </span>
        <span className="text-[12.5px] leading-normal text-text-2">{sub}</span>
      </span>
    </button>
  );
}

// ---- MAP's key

function MapKeyCard({ access, onChanged }: { access: GitAccess | null; onChanged: () => void }) {
  const { toast } = useApp();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const key = access?.key ?? null;
  const create = async (replace: boolean) => {
    if (
      replace &&
      !confirm(
        "Replace MAP's SSH key?\n\nHosts that know the current key will refuse the new one until you add it there. The old key is moved aside (id_ed25519.old-…), not deleted.",
      )
    )
      return;
    setBusy(true);
    setError("");
    try {
      const k = await gitApi.createKey(replace);
      toast(`SSH key ${replace ? "replaced" : "created"} · ${k.fingerprint}`);
      onChanged();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const copy = async () => {
    if (!key) return;
    try {
      await navigator.clipboard.writeText(key.public_key);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      toast("Copy failed: select the key and copy it by hand", "warn");
    }
  };
  return (
    <Card className="flex flex-col gap-3 p-[18px]">
      <SectionTitle note={key ? key.comment : undefined}>MAP&apos;s SSH key</SectionTitle>
      {!access ? (
        <p className="font-mono text-[11.5px] text-muted">loading…</p>
      ) : key ? (
        <>
          <div className="flex min-w-0 flex-col gap-2 @lg:flex-row @lg:items-start">
            <code className="min-w-0 flex-1 rounded-lg border border-line bg-inset px-3 py-2.5 font-mono text-[11.5px] leading-[1.5] break-all text-text select-all">{key.public_key}</code>
            <Btn size="sm" onClick={copy} aria-label="Copy the public key" className="shrink-0 self-start @lg:mt-1">
              {copied ? <CheckIcon className="size-3 text-ok" /> : <Copy className="size-3" />}
              {copied ? "Copied" : "Copy"}
            </Btn>
          </div>
          <dl className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11.5px]">
            <dt className="text-muted">fingerprint</dt>
            <dd className="break-all">{key.fingerprint}</dd>
            <dt className="text-muted">created</dt>
            <dd>{new Date(key.created_at).toLocaleString()}</dd>
          </dl>
          <div>
            <Btn size="sm" tone="danger" disabled={busy} onClick={() => create(true)}>
              {busy ? "Replacing…" : "Replace key"}
            </Btn>
          </div>
        </>
      ) : (
        <>
          <p className="text-[13px] leading-normal text-text-2">
            MAP has no key yet. It makes an ed25519 key without a passphrase (so agents never stop at a prompt) in its data folder. Only the public half is ever shown.
          </p>
          <div>
            <Btn tone="primary" disabled={busy || !access.ssh_available} onClick={() => create(false)}>
              <KeyRound className="size-3.5" />
              {busy ? "Creating…" : "Create key"}
            </Btn>
          </div>
        </>
      )}
      {error && <p className="text-[12px] text-bad">{error}</p>}
    </Card>
  );
}

// ---- this machine's ~/.ssh

function SystemSshCard({ ssh }: { ssh: SystemSsh }) {
  return (
    <Card className="flex flex-col gap-3 p-[18px]">
      <SectionTitle note="public keys only">This machine&apos;s SSH setup</SectionTitle>
      <dl className="grid grid-cols-[7.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11.5px]">
        <dt className="text-muted">folder</dt>
        <dd className="break-all">
          {ssh.dir} · <YesNo on={ssh.dir_exists} yes="found" no="missing" warn />
        </dd>
        <dt className="text-muted">ssh config</dt>
        <dd>
          <YesNo on={ssh.config_exists} yes="~/.ssh/config" no="none" />
        </dd>
        <dt className="text-muted">known_hosts</dt>
        <dd>
          <YesNo on={ssh.known_hosts_exists} yes="~/.ssh/known_hosts" no="none yet" warn />
        </dd>
        <dt className="text-muted">ssh-agent</dt>
        <dd>
          <YesNo on={ssh.agent_socket} yes="SSH_AUTH_SOCK is set for MAP" no="no SSH_AUTH_SOCK for MAP" />
        </dd>
      </dl>
      {ssh.public_keys.length > 0 ? (
        <ul className="flex flex-col divide-y divide-line rounded-lg border border-line">
          {ssh.public_keys.map((k) => (
            <li key={k.file} className="flex min-w-0 flex-col gap-0.5 px-3 py-2 font-mono text-[11.5px]">
              <span className="flex flex-wrap gap-x-2">
                <span className="text-text">{k.file}</span>
                <span className="text-muted">{k.key_type}</span>
                <span className="break-all text-muted">{k.comment}</span>
              </span>
              <span className="break-all text-text-2">{k.fingerprint}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-[12.5px] leading-normal text-text-2">
          No public keys (<code>*.pub</code>) in {ssh.dir}. Make one in a shell as the user MAP runs as: <code>ssh-keygen -t ed25519</code>.
        </p>
      )}
      <p className="text-[12px] leading-normal text-muted">
        MAP lists <code>*.pub</code> files only; it never reads a private key or changes <code>~/.ssh</code>. Agents run unattended: a key with a passphrase works only through an
        ssh-agent MAP can reach, and a host must already be in <code>~/.ssh/known_hosts</code> (connect once from a shell with <code>ssh -T git@&lt;host&gt;</code>).
      </p>
    </Card>
  );
}

// ---- hosts

type HostKind = "github" | "gitlab" | "custom";
type KeyKind = "account" | "deploy";

function HostCards({ access, onTested }: { access: GitAccess | null; onTested: () => void }) {
  const system = access?.settings.ssh === "system";
  const ready = !!access && (system || !!access.key);
  return (
    <div className="flex min-w-0 flex-col gap-2.5">
      <div className="px-0.5">
        <SectionTitle note={system ? "ssh -T with your own ssh setup" : "ssh -T with MAP's key alone"}>Add the key to your git host</SectionTitle>
      </div>
      <div className="grid min-w-0 gap-3 @4xl:grid-cols-3">
        {(["github", "gitlab", "custom"] as HostKind[]).map((k) => (
          <HostCard key={k} kind={k} system={system} ready={ready} hasAccess={!!access} onTested={onTested} />
        ))}
      </div>
    </div>
  );
}

const HOST_TITLE: Record<HostKind, string> = { github: "GitHub", gitlab: "GitLab.com", custom: "Self-hosted GitLab / other git host" };

function HostCard({ kind, system, ready, hasAccess, onTested }: { kind: HostKind; system: boolean; ready: boolean; hasAccess: boolean; onTested: () => void }) {
  const [keyKind, setKeyKind] = useState<KeyKind>("account");
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<GitTestResult | null>(null);
  const [error, setError] = useState("");
  const target = kind === "github" ? "github.com" : kind === "gitlab" ? "gitlab.com" : host.trim() ? `${host.trim()}${port.trim() ? `:${port.trim()}` : ""}` : "";
  const test = async () => {
    if (!target) return;
    setRunning(true);
    setError("");
    setResult(null);
    try {
      setResult(await gitApi.test(target));
      onTested();
    } catch (e) {
      setError(errText(e));
    } finally {
      setRunning(false);
    }
  };
  const copy = system ? (
    <>
      Copy the public key ssh uses, from a shell: <code>cat ~/.ssh/id_ed25519.pub</code> (skip this if it is already on your account).
    </>
  ) : (
    <>Copy MAP&apos;s public key from the card above.</>
  );
  const steps: ReactNode[] =
    kind === "github"
      ? keyKind === "account"
        ? [
            copy,
            <>
              On github.com: your avatar → <B>Settings → SSH and GPG keys → New SSH key</B>. Key type &quot;Authentication Key&quot;, paste, <B>Add SSH key</B>.
            </>,
            <>Agents can then reach every repo your account can. Test: &quot;Hi &lt;you&gt;! You&apos;ve successfully authenticated&quot;.</>,
          ]
        : [
            copy,
            <>
              The repo → <B>Settings → Deploy keys → Add deploy key</B>, paste, and tick <B>Allow write access</B> so agents can push.
            </>,
            <>A deploy key opens that one repo only, and GitHub takes a key as a deploy key on one repo only. Test: &quot;Hi &lt;owner/repo&gt;! …&quot;.</>,
          ]
      : kind === "gitlab"
        ? keyKind === "account"
          ? [
              copy,
              <>
                On gitlab.com: your avatar → <B>Edit profile → SSH Keys → Add new key</B>, paste, usage type &quot;Authentication &amp; Signing&quot;, <B>Add key</B>.
              </>,
              <>Agents can then reach every project you can. Test: &quot;Welcome to GitLab, @you!&quot;.</>,
            ]
          : [
              copy,
              <>
                The project → <B>Settings → Repository → Deploy keys → Add new key</B>, paste, and tick <B>Grant write permissions to this key</B> so agents can push.
              </>,
              <>A deploy key opens that one project only. Test: it should answer &quot;Welcome to GitLab, …!&quot;.</>,
            ]
        : keyKind === "account"
          ? [
              copy,
              <>
                Self-managed GitLab: your avatar → <B>Edit profile → SSH Keys</B>, as on gitlab.com. Gitea / Forgejo: <B>Settings → SSH / GPG Keys → Add key</B>.
              </>,
              <>
                Enter the host below (the port only when ssh is not on 22, e.g. 2222 for GitLab in Docker) and test. Its host key is recorded on first use: compare the fingerprint with the
                one your admin publishes.
              </>,
            ]
          : [
              copy,
              <>
                Self-managed GitLab: the project → <B>Settings → Repository → Deploy keys</B>, with write permission. Gitea / Forgejo: the repo → <B>Settings → Deploy Keys</B>, with
                &quot;Enable write access&quot;.
              </>,
              <>Enter the host below and test. Its host key is recorded on first use: compare the fingerprint with the one your admin publishes.</>,
            ];
  return (
    <Card className="flex min-w-0 flex-col gap-3 p-[18px]">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-[14px] font-semibold text-text">{HOST_TITLE[kind]}</span>
        {kind !== "custom" && <span className="font-mono text-[10.5px] text-muted">host key pinned</span>}
      </div>
      <Segmented<KeyKind>
        value={keyKind}
        options={[
          ["account", "Account key"],
          ["deploy", "Deploy key (one repo)"],
        ]}
        onChange={setKeyKind}
      />
      <ol className="flex flex-col gap-2 text-[12.5px] leading-normal text-text-2">
        {steps.map((s, i) => (
          <Step key={i} n={i + 1}>
            {s}
          </Step>
        ))}
      </ol>
      <form
        className="mt-auto flex min-w-0 flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void test();
        }}
      >
        {kind === "custom" && (
          <>
            <div className="min-w-0 flex-[1_1_160px]">
              <Field label="Host">
                <TextInput className="h-8! text-[12px]" value={host} onChange={(e) => setHost(e.target.value)} placeholder="git.example.com" aria-label="Git host" />
              </Field>
            </div>
            <div className="w-[84px]">
              <Field label="Port">
                <TextInput className="h-8! text-[12px]" value={port} onChange={(e) => setPort(e.target.value.replace(/[^0-9]/g, ""))} placeholder="22" inputMode="numeric" aria-label="SSH port" />
              </Field>
            </div>
          </>
        )}
        <Btn size="sm" type="submit" disabled={!ready || !target || running}>
          {running ? "Testing…" : "Test"}
          {!running && kind !== "custom" && <span className="font-mono font-normal text-text-2">{target}</span>}
        </Btn>
      </form>
      {hasAccess && !ready && <p className="font-mono text-[11px] text-muted">Create MAP&apos;s key first.</p>}
      {error && <p className="text-[12px] break-words text-bad">{error}</p>}
      {result && <TestResult result={result} system={system} />}
    </Card>
  );
}

function TestResult({ result, system }: { result: GitTestResult; system: boolean }) {
  const whose = system ? "your SSH key" : "MAP's key";
  return (
    <Inset className="gap-1">
      <span className={result.ok ? "text-ok" : "text-bad"}>
        {result.ok ? "✓" : "✗"} {result.host} {result.ok ? `accepts ${whose}` : `did not accept ${whose}`}
      </span>
      <span className="break-all text-muted">$ {result.command}</span>
      <span className="break-words whitespace-pre-wrap text-text-2">{result.output || "(no output)"}</span>
      {result.host_keys.map((k) => (
        <span key={k.fingerprint} className="break-all text-muted">
          host key {k.key_type} {k.fingerprint} · {k.pinned ? "pinned (published by the host)" : "recorded on first use: compare it with the host's published fingerprint"}
        </span>
      ))}
    </Inset>
  );
}

// ---- known hosts

function KnownHostsCard({ access, onChanged }: { access: GitAccess | null; onChanged: () => void }) {
  const { toast } = useApp();
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const system = access?.settings.ssh === "system";
  const forget = async (k: KnownHost) => {
    if (!confirm(`Forget ${k.host}'s host key?\n\nThe next connection records the key it then offers (trust on first use). Do this only when the host really changed its key.`)) return;
    setBusy(k.host);
    setError("");
    try {
      await gitApi.removeKnownHost(k.host);
      toast(`forgot ${k.host}`);
      onChanged();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  };
  const hosts = access?.known_hosts ?? [];
  return (
    <Card className="flex flex-col gap-3 p-[18px]">
      <SectionTitle note={`${hosts.length} key${hosts.length === 1 ? "" : "s"} · MAP's known_hosts`}>Known hosts</SectionTitle>
      <p className="text-[12.5px] leading-normal text-text-2">
        GitHub&apos;s and GitLab.com&apos;s published host keys are pinned and cannot be removed. Other hosts are learned on first use; forget one only when it really changed its key.
        {system && <> With this machine&apos;s SSH setup, ssh uses <code>~/.ssh/known_hosts</code> instead, which MAP does not change.</>}
      </p>
      {!access ? (
        <p className="font-mono text-[11.5px] text-muted">loading…</p>
      ) : (
        <ul className="flex flex-col divide-y divide-line rounded-lg border border-line">
          {hosts.map((k, i) => (
            <li key={`${k.host}-${k.fingerprint}`} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 font-mono text-[11.5px]">
              <span className="min-w-0 break-all text-text">{k.host}</span>
              <span className="text-muted">{k.key_type}</span>
              <span className="min-w-0 flex-[1_1_260px] break-all text-text-2">{k.fingerprint}</span>
              <span className={clsx("rounded border px-1.5 text-[9.5px] tracking-[0.08em] uppercase", k.pinned ? "border-ok/35 text-ok" : "border-warn/45 text-warn")}>{k.pinned ? "pinned" : "learned"}</span>
              {!k.pinned && hosts.findIndex((x) => x.host === k.host) === i && (
                <Btn size="sm" tone="quiet" className="h-6! px-2!" disabled={busy !== null} onClick={() => forget(k)}>
                  {busy === k.host ? "Forgetting…" : "Forget"}
                </Btn>
              )}
            </li>
          ))}
        </ul>
      )}
      {error && <p className="text-[12px] text-bad">{error}</p>}
    </Card>
  );
}

// ---- identity

function IdentityCard({ access, onSaved }: { access: GitAccess | null; onSaved: () => void }) {
  const { toast } = useApp();
  const [name, setName] = useState(access?.identity.name ?? "");
  const [email, setEmail] = useState(access?.identity.email ?? "");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const changed = !!access && (name !== access.identity.name || email !== access.identity.email);
  const save = async () => {
    setBusy(true);
    setError("");
    try {
      await gitApi.setIdentity({ name, email });
      toast("git identity saved");
      onSaved();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card className="flex flex-col gap-3 p-[18px]">
      <SectionTitle note="not a secret">Git identity</SectionTitle>
      <p className="text-[12.5px] leading-normal text-text-2">The author of agents&apos; commits, unless your own ~/.gitconfig or the repo sets one. Leave both empty to set none.</p>
      <div className="grid gap-3 @lg:grid-cols-2">
        <Field label="Name">
          <TextInput mono={false} value={name} onChange={(e) => setName(e.target.value)} placeholder="MAP agents" />
        </Field>
        <Field label="Email">
          <TextInput value={email} onChange={(e) => setEmail(e.target.value)} placeholder="you@example.com" type="email" />
        </Field>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Btn size="sm" tone="primary" disabled={!changed || busy} onClick={save}>
          {busy ? "Saving…" : "Save identity"}
        </Btn>
        {error && <span className="text-[12px] text-bad">{error}</span>}
      </div>
    </Card>
  );
}

// ---- gh / glab

function VarState({ v }: { v: EnvVarState }) {
  const where = [v.in_workspace_env && "map.toml", v.in_server_env && "server env"].filter(Boolean).join(" + ");
  return (
    <li className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5 font-mono text-[11.5px]">
      <span className="text-text">{v.name}</span>
      <span className={where ? "text-ok" : "text-muted"}>{where ? `set · ${where}` : "not set"}</span>
      <span className="min-w-0 basis-full font-sans text-[11.5px] text-muted sm:basis-auto">{v.purpose}</span>
    </li>
  );
}

function GitCliCard({ settings }: { settings: SettingsInfo | null }) {
  return (
    <Card className="flex flex-col gap-3 p-[18px]">
      <SectionTitle note="optional">GitHub / GitLab CLI</SectionTitle>
      <p className="text-[12.5px] leading-normal text-text-2">
        SSH covers clone, pull and push. Agents that open pull or merge requests and issues use <code>gh</code> or <code>glab</code>, which sign in with a token instead:{" "}
        <code>GH_TOKEN</code> for GitHub, <code>GITLAB_TOKEN</code> for GitLab (plus <code>GITLAB_HOST</code> for a self-hosted one). Set them in <code>[workspace.env]</code> of{" "}
        <code>map.toml</code> or in the service&apos;s environment, then restart MAP. The dashboard never takes or shows a token: below is only whether each is set.
      </p>
      <pre className="overflow-x-auto rounded-lg border border-line bg-inset px-3 py-2 font-mono text-[11.5px] leading-[1.6] text-text-2">
        {`[workspace.env]
GH_TOKEN = "…"        # fine-grained: Contents, Pull requests, Issues (read & write)
GITLAB_TOKEN = "…"    # scope: api
GITLAB_HOST = "gitlab.example.com"   # self-hosted GitLab only`}
      </pre>
      {!settings ? (
        <p className="font-mono text-[11.5px] text-muted">loading…</p>
      ) : (
        <div className="grid gap-2.5 @xl:grid-cols-2">
          {settings.git_cli.map((t) => (
            <div key={t.name} className="flex min-w-0 flex-col gap-2 rounded-lg border border-line px-3.5 py-3">
              <span className="flex flex-wrap items-baseline gap-x-2">
                <span className="font-mono text-[12.5px] font-semibold text-text">{t.name}</span>
                <span className="text-[12px] text-muted">{t.title}</span>
              </span>
              <span className="font-mono text-[11.5px] break-all">
                {t.path ? <span className="text-ok">on the agents&apos; PATH · {t.path}</span> : <span className="text-muted">not on the agents&apos; PATH</span>}
              </span>
              <ul className="flex flex-col gap-1.5">
                {t.vars.map((v) => (
                  <VarState key={v.name} v={v} />
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

// ---- how it works

function HowItWorks({ access }: { access: GitAccess }) {
  return (
    <details className="group rounded-xl border border-line px-[18px] py-3">
      <summary className="cursor-pointer font-mono text-[11px] font-semibold tracking-[0.12em] text-text-2 uppercase">How it works</summary>
      <div className="mt-2.5 flex min-w-0 flex-col gap-2 text-[12.5px] leading-normal text-text-2">
        <p>
          MAP writes a gitconfig with its defaults (the identity, and with MAP&apos;s own key <code>core.sshCommand</code> with that key and MAP&apos;s known_hosts) and exports it
          as <code>GIT_CONFIG_GLOBAL</code> in every agent session; MAP&apos;s own git commands use it too. Your own global config is included after MAP&apos;s defaults, so its
          settings win; a repo&apos;s own config wins over both.
        </p>
        <dl className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11px]">
          <dt className="text-muted">gitconfig</dt>
          <dd className="break-all">{access.gitconfig_path}</dd>
          <dt className="text-muted">includes</dt>
          <dd className="break-all">{access.includes.length ? access.includes.join(" · ") : "none (no ~/.gitconfig here)"}</dd>
          <dt className="text-muted">project folder</dt>
          <dd className="break-all">{access.projects_root}</dd>
        </dl>
      </div>
    </details>
  );
}
