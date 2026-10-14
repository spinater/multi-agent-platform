"use client";

/** Git for projects (backend project_git.rs): a project's Git panel (status, fetch, pull, push, connect a remote),
 * the compact status shown on a project's card, and the wizard's "Git access" dialog, which shows Settings → Git &
 * SSH in place so the wizard keeps what was typed. Everywhere else links to `#settings/git`. */

import clsx from "clsx";
import { GitBranch, KeyRound, X } from "lucide-react";
import { useEffect, useState } from "react";

import { gitApi, type GitOp } from "@/lib/api";
import type { GitOpResult } from "@/lib/generated/GitOpResult";
import type { GitStatus } from "@/lib/generated/GitStatus";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";

import { useApp } from "./app-context";
import { Btn, Inset, TextInput, ago, errText } from "./project-ui";
import { GitSshSettings } from "./settings/git-ssh";
import { Card, SectionTitle } from "./shell/page";

/** Settings → Git & SSH over the New project wizard (its clone step), so the wizard keeps its state. */
export function GitAccessDialog({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-[60] flex items-start justify-center overflow-y-auto bg-[rgba(3,4,6,.72)] p-3 backdrop-blur-sm sm:p-5" onClick={onClose}>
      <div
        role="dialog"
        aria-label="Git access"
        onClick={(e) => e.stopPropagation()}
        className="flex w-full max-w-[760px] min-w-0 flex-col rounded-[14px] border border-white/10 bg-menu shadow-[0_30px_80px_rgba(0,0,0,.6)]"
      >
        <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-4">
          <span className="flex items-center gap-2 text-[15px] font-semibold whitespace-nowrap">
            <KeyRound className="size-4 text-accent" />
            Git &amp; SSH
          </span>
          <button onClick={onClose} className="flex size-[26px] items-center justify-center rounded-md border border-white/10 text-nav hover:text-text" aria-label="Close">
            <X className="size-3.5" />
          </button>
        </div>
        <div className="min-w-0 p-[18px]">
          <GitSshSettings />
        </div>
      </div>
    </div>
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
  const { now, toast, go } = useApp();
  const slug = info.project.slug;
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [loadError, setLoadError] = useState("");
  const [running, setRunning] = useState<string | null>(null);
  const [last, setLast] = useState<{ label: string; result: GitOpResult | null; error: string } | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [url, setUrl] = useState("");

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
      <SectionTitle note={<GitAccessLink onClick={() => go("settings", "git")} />}>Git</SectionTitle>
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
    </Card>
  );
}

function GitAccessLink({ onClick }: { onClick: () => void }) {
  return (
    <a
      href="#settings/git"
      onClick={(e) => {
        e.preventDefault();
        onClick();
      }}
      className="flex items-center gap-1 text-link no-underline hover:text-link-hover"
    >
      <KeyRound className="size-3" /> git &amp; ssh settings
    </a>
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
