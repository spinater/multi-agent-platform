"use client";

/** Git for projects (backend git_access.rs, project_git.rs): the "Git access" dialog with MAP's own SSH key, how to
 * add it to GitHub or GitLab, host tests and the git identity; a project's Git panel (status, fetch, pull, push,
 * connect a remote); and the compact status shown on a project's card. Only MAP's public key is ever shown, and the
 * dashboard never takes a private key, token or password. */

import clsx from "clsx";
import { Check as CheckIcon, Copy, GitBranch, KeyRound, X } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";

import { gitApi, type GitOp } from "@/lib/api";
import type { GitAccess } from "@/lib/generated/GitAccess";
import type { GitOpResult } from "@/lib/generated/GitOpResult";
import type { GitStatus } from "@/lib/generated/GitStatus";
import type { GitTestResult } from "@/lib/generated/GitTestResult";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";

import { useApp } from "./app-context";
import { Btn, Field, Inset, Segmented, TextInput, ago, errText } from "./project-ui";
import { Card, SectionTitle } from "./shell/page";

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

// ---- the Git access dialog

type Host = "github" | "gitlab";

/** The "Git access" dialog: MAP's SSH key (create, copy, replace), where to add it on GitHub or GitLab, host tests
 * and the git identity. Opened from the Projects screen, a project's Git panel and the wizard's clone step. */
export function GitAccessDialog({ onClose }: { onClose: () => void }) {
  const { toast } = useApp();
  const { access, error: loadError, reload } = useGitAccess();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [host, setHost] = useState<Host>("github");

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

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
      reload();
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
    <div className="fixed inset-0 z-[60] flex items-start justify-center overflow-y-auto bg-[rgba(3,4,6,.72)] p-3 backdrop-blur-sm sm:p-5" onClick={onClose}>
      <div
        role="dialog"
        aria-label="Git access"
        onClick={(e) => e.stopPropagation()}
        className="flex w-full max-w-[680px] min-w-0 flex-col rounded-[14px] border border-white/10 bg-menu shadow-[0_30px_80px_rgba(0,0,0,.6)]"
      >
        <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-4">
          <span className="flex items-center gap-2 text-[15px] font-semibold whitespace-nowrap">
            <KeyRound className="size-4 text-accent" />
            Git access
          </span>
          <button onClick={onClose} className="flex size-[26px] items-center justify-center rounded-md border border-white/10 text-nav hover:text-text" aria-label="Close">
            <X className="size-3.5" />
          </button>
        </div>

        <div className="flex min-w-0 flex-col gap-5 p-[18px]">
          <p className="text-[13px] leading-normal text-text-2">
            One SSH key for every project, made on this machine and kept in MAP&apos;s data folder (so it also works in Docker without mounting <code>~/.ssh</code>).
            Agents and MAP&apos;s own clone, fetch, pull and push use it. Only the public half is ever shown here; MAP never asks for a private key, token or password.
          </p>
          {loadError && <p className="text-[12px] text-bad">{loadError}</p>}
          {access && !access.ssh_available && (
            <p className="rounded-lg border border-warn/35 bg-warn/[.07] px-3 py-2 text-[12.5px] leading-normal text-warn">
              ssh or ssh-keygen is not installed where MAP runs (package openssh-client), so it cannot make or use a key.
            </p>
          )}

          <section className="flex min-w-0 flex-col gap-2.5">
            <SectionTitle note={key ? key.comment : undefined}>MAP&apos;s SSH key</SectionTitle>
            {!access ? (
              <p className="font-mono text-[11.5px] text-muted">loading…</p>
            ) : key ? (
              <>
                <div className="flex min-w-0 items-start gap-2">
                  <code className="min-w-0 flex-1 rounded-lg border border-line bg-inset px-3 py-2.5 font-mono text-[11.5px] leading-[1.5] break-all text-text select-all">{key.public_key}</code>
                  <Btn size="sm" onClick={copy} aria-label="Copy the public key" className="mt-1 shrink-0">
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
                <p className="text-[13px] leading-normal text-text-2">MAP has no key yet. It makes an ed25519 key without a passphrase, so agents never stop at a prompt.</p>
                <div>
                  <Btn tone="primary" disabled={busy || !access.ssh_available} onClick={() => create(false)}>
                    {busy ? "Creating…" : "Create key"}
                  </Btn>
                </div>
              </>
            )}
            {error && <p className="text-[12px] text-bad">{error}</p>}
          </section>

          <section className="flex min-w-0 flex-col gap-2.5">
            <SectionTitle>Add it to your host</SectionTitle>
            <Segmented<Host>
              value={host}
              options={[
                ["github", "GitHub"],
                ["gitlab", "GitLab"],
              ]}
              onChange={setHost}
            />
            {host === "github" ? (
              <ol className="flex flex-col gap-2 text-[13px] leading-normal text-text-2">
                <Step n={1}>Copy the public key above.</Step>
                <Step n={2}>
                  <b className="text-text">Every repo you can reach:</b> on github.com, your avatar → <b>Settings → SSH and GPG keys → New SSH key</b>, type
                  &quot;Authentication Key&quot;, paste, Add.
                </Step>
                <Step n={3}>
                  <b className="text-text">Or one repo only:</b> the repo → <b>Settings → Deploy keys → Add deploy key</b>, paste, and tick{" "}
                  <b>Allow write access</b> so agents can push.
                </Step>
                <Step n={4}>Test github.com below: it should say &quot;Hi &lt;you&gt;! You&apos;ve successfully authenticated&quot;.</Step>
              </ol>
            ) : (
              <ol className="flex flex-col gap-2 text-[13px] leading-normal text-text-2">
                <Step n={1}>Copy the public key above.</Step>
                <Step n={2}>
                  <b className="text-text">Every project you can reach:</b> your avatar → <b>Preferences → SSH Keys → Add new key</b>, paste, usage type
                  &quot;Authentication &amp; Signing&quot;, Add key.
                </Step>
                <Step n={3}>
                  <b className="text-text">Or one project only:</b> the project → <b>Settings → Repository → Deploy keys → Add new key</b>, paste, and tick{" "}
                  <b>Grant write permissions to this key</b> so agents can push.
                </Step>
                <Step n={4}>Test gitlab.com below (or your own GitLab&apos;s host): it should say &quot;Welcome to GitLab, @you!&quot;.</Step>
              </ol>
            )}
          </section>

          <HostTests hasKey={!!key} onTested={reload} />

          <IdentityForm key={access ? `${access.identity.name}|${access.identity.email}` : "loading"} access={access} onSaved={reload} />

          {access && (
            <details className="group rounded-lg border border-line px-3.5 py-2.5">
              <summary className="cursor-pointer font-mono text-[11px] font-semibold tracking-[0.12em] text-text-2 uppercase">How it works</summary>
              <div className="mt-2.5 flex min-w-0 flex-col gap-2 text-[12.5px] leading-normal text-text-2">
                <p>
                  MAP writes a gitconfig with its defaults (identity, and <code>core.sshCommand</code> with this key) and exports it as <code>GIT_CONFIG_GLOBAL</code> in every agent
                  session; MAP&apos;s own git commands use it too. Your own global config is included after MAP&apos;s defaults, so its settings win; a repo&apos;s own config wins
                  over both.
                </p>
                <dl className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 font-mono text-[11px]">
                  <dt className="text-muted">gitconfig</dt>
                  <dd className="break-all">{access.gitconfig_path}</dd>
                  <dt className="text-muted">includes</dt>
                  <dd className="break-all">{access.includes.length ? access.includes.join(" · ") : "none (no ~/.gitconfig here)"}</dd>
                  <dt className="text-muted">projects root</dt>
                  <dd className="break-all">{access.projects_root}</dd>
                </dl>
                <p>GitHub&apos;s and GitLab&apos;s published host keys are pinned. Other hosts are trusted on first use; check the fingerprint a test records.</p>
                <ul className="flex flex-col font-mono text-[11px]">
                  {access.known_hosts.map((k) => (
                    <li key={`${k.host}-${k.fingerprint}`} className="flex flex-wrap gap-x-2">
                      <span className="text-text">{k.host}</span>
                      <span className="text-muted">{k.key_type}</span>
                      <span className="break-all">{k.fingerprint}</span>
                      <span className={k.pinned ? "text-ok" : "text-warn"}>{k.pinned ? "pinned" : "first use"}</span>
                    </li>
                  ))}
                </ul>
              </div>
            </details>
          )}
        </div>
      </div>
    </div>
  );
}

function Step({ n, children }: { n: number; children: ReactNode }) {
  return (
    <li className="flex gap-2.5">
      <span className="mt-px flex size-5 shrink-0 items-center justify-center rounded-full border border-accent font-mono text-[10px] font-semibold text-accent-soft">{n}</span>
      <span className="min-w-0">{children}</span>
    </li>
  );
}

function HostTests({ hasKey, onTested }: { hasKey: boolean; onTested: () => void }) {
  const [custom, setCustom] = useState("");
  const [running, setRunning] = useState<string | null>(null);
  const [result, setResult] = useState<GitTestResult | null>(null);
  const [error, setError] = useState("");
  const test = async (host: string) => {
    setRunning(host);
    setError("");
    setResult(null);
    try {
      setResult(await gitApi.test(host));
      onTested();
    } catch (e) {
      setError(errText(e));
    } finally {
      setRunning(null);
    }
  };
  return (
    <section className="flex min-w-0 flex-col gap-2.5">
      <SectionTitle note="ssh -T with MAP's key alone">Test</SectionTitle>
      <div className="flex flex-wrap items-center gap-2">
        {["github.com", "gitlab.com"].map((h) => (
          <Btn key={h} size="sm" disabled={!hasKey || running !== null} onClick={() => test(h)}>
            {running === h ? `Testing ${h}…` : `Test ${h}`}
          </Btn>
        ))}
        <form
          className="flex min-w-0 flex-[1_1_220px] gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (custom.trim()) void test(custom.trim());
          }}
        >
          <TextInput className="h-7! text-[12px]" value={custom} onChange={(e) => setCustom(e.target.value)} placeholder="git.example.com:2222" aria-label="Custom host" />
          <Btn size="sm" type="submit" disabled={!hasKey || !custom.trim() || running !== null}>
            {running && running === custom.trim() ? "Testing…" : "Test"}
          </Btn>
        </form>
      </div>
      {!hasKey && <p className="font-mono text-[11px] text-muted">Create the key first.</p>}
      {error && <p className="text-[12px] text-bad">{error}</p>}
      {result && (
        <Inset className="gap-1">
          <span className={result.ok ? "text-ok" : "text-bad"}>
            {result.ok ? "✓" : "✗"} {result.host} {result.ok ? "accepts MAP's key" : "did not accept MAP's key"}
          </span>
          <span className="text-muted">$ {result.command}</span>
          <span className="break-words whitespace-pre-wrap text-text-2">{result.output || "(no output)"}</span>
          {result.host_keys.map((k) => (
            <span key={k.fingerprint} className="break-all text-muted">
              host key {k.key_type} {k.fingerprint} · {k.pinned ? "pinned (published by the host)" : "recorded on first use: compare it with the host's published fingerprint"}
            </span>
          ))}
        </Inset>
      )}
    </section>
  );
}

function IdentityForm({ access, onSaved }: { access: GitAccess | null; onSaved: () => void }) {
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
    <section className="flex min-w-0 flex-col gap-2.5">
      <SectionTitle note="not a secret">Git identity</SectionTitle>
      <p className="text-[12.5px] leading-normal text-text-2">The author of agents&apos; commits, unless your own ~/.gitconfig or the repo sets one. Leave both empty to set none.</p>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Name">
          <TextInput mono={false} value={name} onChange={(e) => setName(e.target.value)} placeholder="MAP agents" />
        </Field>
        <Field label="Email">
          <TextInput value={email} onChange={(e) => setEmail(e.target.value)} placeholder="you@example.com" type="email" />
        </Field>
      </div>
      <div className="flex items-center gap-2">
        <Btn size="sm" tone="primary" disabled={!changed || busy} onClick={save}>
          {busy ? "Saving…" : "Save identity"}
        </Btn>
        {error && <span className="text-[12px] text-bad">{error}</span>}
      </div>
    </section>
  );
}

// ---- a project's Git panel

/** `↑2 ↓1`, or `no upstream`. */
function aheadBehind(s: GitStatus): string {
  if (s.ahead === null || s.behind === null) return s.upstream ? "" : "no upstream";
  if (s.ahead === 0 && s.behind === 0) return "in sync";
  return `↑${s.ahead} ↓${s.behind}`;
}

/** The project's folder as git sees it, with Fetch, Pull (fast-forward only), Push (never forced) and "Connect a
 * remote", and the output of the last command. */
export function ProjectGit({ info, className }: { info: ProjectInfo; className?: string }) {
  const { now, toast } = useApp();
  const slug = info.project.slug;
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [loadError, setLoadError] = useState("");
  const [running, setRunning] = useState<string | null>(null);
  const [last, setLast] = useState<{ label: string; result: GitOpResult | null; error: string } | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [url, setUrl] = useState("");
  const [dialog, setDialog] = useState(false);

  useEffect(() => {
    let stale = false;
    const load = () =>
      gitApi.status(slug).then(
        (s) => {
          if (stale) return;
          setStatus(s);
          setLoadError("");
        },
        (e) => !stale && setLoadError(errText(e)),
      );
    void load();
    // Agents commit while the page is open; a cheap local status every 20 s keeps the numbers honest.
    const t = window.setInterval(load, 20_000);
    return () => {
      stale = true;
      window.clearInterval(t);
    };
  }, [slug]);

  const op = async (label: string, run: () => Promise<GitOpResult>) => {
    setRunning(label);
    try {
      const result = await run();
      setStatus(result.status);
      setLast({ label, result, error: "" });
      if (result.ok) toast(`git ${label} · ${slug}`);
      return result.ok;
    } catch (e) {
      setLast({ label, result: null, error: errText(e) });
      return false;
    } finally {
      setRunning(null);
    }
  };
  const doOp = (o: GitOp) => op(o, () => gitApi.run(slug, o));
  const connect = async () => {
    if (await op("remote", () => gitApi.setRemote(slug, url.trim()))) {
      setConnecting(false);
      setUrl("");
    }
  };

  const s = status;
  const blocked = s?.busy ?? null;
  const remote = info.project.settings.source.remote;
  return (
    <Card className={clsx("flex flex-col gap-3.5 p-[18px]", className)}>
      <SectionTitle note={<GitAccessLink onClick={() => setDialog(true)} />}>Git</SectionTitle>
      {loadError && <p className="text-[12px] text-bad">{loadError}</p>}
      {!s && !loadError && <p className="font-mono text-[11px] text-muted">reading the folder…</p>}
      {s && !s.is_repo && <p className="text-[13px] leading-normal text-text-2">The folder is not a git repo. Connect a remote to start one: MAP runs <code>git init -b main</code>, then adds it as origin.</p>}
      {s?.is_repo && (
        <dl className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 font-mono text-[11.5px]">
          <dt className="text-muted">branch</dt>
          <dd className="break-all">
            {s.branch ?? <span className="text-warn">detached HEAD</span>}
            {s.upstream && <span className="text-muted"> → {s.upstream}</span>}
            {aheadBehind(s) && <span className={clsx(" ", s.ahead || s.behind ? "text-warn" : "text-ok")}> · {aheadBehind(s)}</span>}
          </dd>
          <dt className="text-muted">origin</dt>
          <dd className="break-all">{s.remote_url ?? <span className="text-muted">none</span>}</dd>
          <dt className="text-muted">changes</dt>
          <dd className={s.dirty ? "text-warn" : "text-ok"}>{s.dirty ? `${s.dirty} changed file${s.dirty === 1 ? "" : "s"}` : "clean"}</dd>
          <dt className="text-muted">last commit</dt>
          <dd className="min-w-0 truncate">
            {s.last_commit ? (
              <>
                <span className="text-muted">{s.last_commit.sha.slice(0, 8)}</span> {s.last_commit.subject}{" "}
                <span className="text-muted">· {ago(Date.parse(s.last_commit.at), now)}</span>
              </>
            ) : (
              <span className="text-muted">no commits yet</span>
            )}
          </dd>
          <dt className="text-muted">last fetch</dt>
          <dd className="text-text-2">{s.last_fetch ? ago(Date.parse(s.last_fetch), now) : "never"}</dd>
        </dl>
      )}
      {remote && (!s?.remote_url || s.remote_url !== remote) && <p className="font-mono text-[11px] break-all text-muted">recorded remote (clone or last connect): {remote}</p>}
      {blocked && <p className="text-[12px] leading-snug text-warn">{blocked}: pull, push and remote changes wait; fetch still works.</p>}
      {s && (
        <div className="flex flex-wrap items-center gap-2">
          {s.is_repo && (
            <>
              <Btn size="sm" disabled={running !== null || !s.remote_url} onClick={() => doOp("fetch")} title="git fetch --prune">
                {running === "fetch" ? "Fetching…" : "Fetch"}
              </Btn>
              <Btn size="sm" disabled={running !== null || !!blocked || !s.upstream} onClick={() => doOp("pull")} title="Fast-forward only; refused when the folder has changes or the branch diverged">
                {running === "pull" ? "Pulling…" : "Pull"}
              </Btn>
              <Btn size="sm" disabled={running !== null || !!blocked || !s.remote_url || !s.branch} onClick={() => doOp("push")} title="Pushes the current branch, never forced; a first push sets its upstream">
                {running === "push" ? "Pushing…" : s.upstream ? "Push" : "Push · set upstream"}
              </Btn>
            </>
          )}
          {!connecting && (
            <Btn size="sm" tone={s.remote_url ? "quiet" : "primary"} disabled={running !== null || !!blocked} onClick={() => setConnecting(true)}>
              {s.remote_url ? "Change remote" : "Connect a remote"}
            </Btn>
          )}
        </div>
      )}
      {connecting && (
        <form
          className="flex flex-wrap gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (url.trim()) void connect();
          }}
        >
          <TextInput className="h-[34px]! min-w-0 flex-[1_1_240px]" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="git@github.com:you/repo.git" autoFocus aria-label="Remote URL" />
          <Btn type="submit" tone="primary" disabled={!url.trim() || running !== null}>
            {running === "remote" ? "Connecting…" : s?.remote_url ? "Set origin" : "Connect"}
          </Btn>
          <Btn type="button" tone="quiet" onClick={() => setConnecting(false)}>
            Cancel
          </Btn>
          <p className="w-full font-mono text-[11px] leading-snug text-muted">
            An ssh URL uses MAP&apos;s key; an https URL works only for public repos (MAP takes no tokens).
          </p>
        </form>
      )}
      {last && (
        <Inset className="gap-0.5">
          {last.result?.command.split("\n").map((c) => (
            <span key={c} className="break-all text-muted">
              $ {c}
            </span>
          ))}
          {last.result?.output && <span className="break-words whitespace-pre-wrap text-text-2">{last.result.output}</span>}
          {last.result && <span className={last.result.ok ? "text-ok" : "text-bad"}>{last.result.ok ? `✓ ${last.label} done` : `✗ ${last.label} failed`}</span>}
          {last.error && <span className="break-words text-bad">✗ {last.error}</span>}
        </Inset>
      )}
      {dialog && <GitAccessDialog onClose={() => setDialog(false)} />}
    </Card>
  );
}

function GitAccessLink({ onClick }: { onClick: () => void }) {
  return (
    <button onClick={onClick} className="flex items-center gap-1 text-link hover:text-link-hover">
      <KeyRound className="size-3" /> git access
    </button>
  );
}

/** One line for a project's card: `⎇ main · ↑1 ↓0 · 2 changed`. */
export function GitLine({ slug }: { slug: string }) {
  const [s, setS] = useState<GitStatus | null>(null);
  useEffect(() => {
    let stale = false;
    const load = () => gitApi.status(slug).then((x) => !stale && setS(x), () => {});
    void load();
    const t = window.setInterval(load, 60_000);
    return () => {
      stale = true;
      window.clearInterval(t);
    };
  }, [slug]);
  if (!s) return null;
  if (!s.is_repo) return <span className="font-mono text-[11px] text-muted">not a git repo</span>;
  const ab = aheadBehind(s);
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-x-1.5 font-mono text-[11px] text-text-2" title={s.remote_url ?? "no origin"}>
      <GitBranch className="size-3 shrink-0 text-muted" />
      <span className="break-all">{s.branch ?? "detached"}</span>
      {ab && <span className={s.ahead || s.behind ? "text-warn" : "text-muted"}>· {ab}</span>}
      <span className={s.dirty ? "text-warn" : "text-muted"}>· {s.dirty ? `${s.dirty} changed` : "clean"}</span>
      {!s.remote_url && <span className="text-muted">· no origin</span>}
    </span>
  );
}
