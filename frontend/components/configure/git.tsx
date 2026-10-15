"use client";

/** Configure › Git (design: Configure.dc.html `gitVals`): Git & SSH (machine: `git_access.rs`), Repo & branches (the
 * project folder's git state, `settings.branches`), Parallel (`settings.parallel`, the machine-wide cap and the
 * worktree estimate). The SSH method and the git identity are drafts for the save bar; making a key, testing a host
 * and fetch / pull / push act at once. */

import { useEffect, useState } from "react";

import { api, gitApi, type GitOp } from "@/lib/api";
import type { GitAccess } from "@/lib/generated/GitAccess";
import type { GitIdentity } from "@/lib/generated/GitIdentity";
import type { GitStatus } from "@/lib/generated/GitStatus";
import type { GitTestResult } from "@/lib/generated/GitTestResult";
import type { LandMode } from "@/lib/generated/LandMode";
import type { SshMethod } from "@/lib/generated/SshMethod";
import type { WorktreeEstimate } from "@/lib/generated/WorktreeEstimate";

import { useApp } from "../app-context";
import { useProjectSettings } from "./project-draft";
import { Box, Lbl, Note, RadioCard, Seg, Stepper, Switch, TabHead, btnCls, inputCls, same, useConfigure } from "./ui";

const mono = "font-mono";
const GB = 1024 ** 3;
const fmtGB = (b: number) => `${(b / GB).toFixed(1)} GB`;
const fmtMB = (b: number) => (b >= GB ? fmtGB(b) : `${Math.round(b / 1024 ** 2)} MB`);

function ago(iso: string | null | undefined, now: number) {
  if (!iso) return "never";
  const m = Math.max(0, Math.round((now - new Date(iso).getTime()) / 60000));
  return m < 60 ? `${m}m ago` : m < 1440 ? `${Math.round(m / 60)}h ago` : `${Math.round(m / 1440)}d ago`;
}

/** Output of a command, in the design's code box. */
function Out({ text, ok }: { text: string; ok: boolean }) {
  return (
    <code className={`rounded-lg border border-line bg-inset px-3 py-2.5 ${mono} text-[11.5px] leading-normal whitespace-pre-wrap`} style={{ color: ok ? "var(--nav)" : "var(--bad)" }}>
      {text}
    </code>
  );
}

// ---- Git & SSH

export function GitTab() {
  const { setPending, flash } = useConfigure();
  const { now } = useApp();
  const [access, setAccess] = useState<GitAccess | null>(null);
  const [error, setError] = useState("");
  const [method, setMethod] = useState<SshMethod | null>(null);
  const [ident, setIdent] = useState<GitIdentity | null>(null);
  const [tests, setTests] = useState<Record<string, GitTestResult | "running">>({});
  const [newHost, setNewHost] = useState("");
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    gitApi.access().then(
      (a) => {
        if (!live) return;
        setAccess(a);
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [tick]);

  const m = method ?? access?.settings.ssh ?? "map_key";
  const id = ident ?? access?.identity ?? { name: "", email: "" };
  const changed = (access && method !== null && method !== access.settings.ssh ? 1 : 0) + (access && ident && !same(ident, access.identity) ? 1 : 0);
  useEffect(() => {
    setPending(
      "git",
      changed
        ? {
            count: changed,
            files: ["<data_dir>/git/settings.json", "<data_dir>/git/gitconfig"],
            save: async () => {
              if (method !== null && method !== access?.settings.ssh) await gitApi.setSettings({ ssh: method });
              if (ident && !same(ident, access?.identity)) await gitApi.setIdentity(ident);
              setMethod(null);
              setIdent(null);
              setTick((t) => t + 1);
            },
            discard: () => {
              setMethod(null);
              setIdent(null);
            },
          }
        : null,
    );
  }, [changed, method, JSON.stringify(ident)]); // eslint-disable-line react-hooks/exhaustive-deps

  const makeKey = async () => {
    if (access?.key && !window.confirm("Make a new key? Agents lose access until you add the new public key to GitHub/GitLab again.")) return;
    setBusy(true);
    try {
      await gitApi.createKey(!!access?.key);
      flash(access?.key ? "New key made · add it to GitHub/GitLab again" : "Key made · add it to your git host");
      setTick((t) => t + 1);
    } catch (e) {
      flash((e as Error).message, "bad");
    } finally {
      setBusy(false);
    }
  };
  const test = async (host: string) => {
    setTests((t) => ({ ...t, [host]: "running" }));
    try {
      const r = await gitApi.test(host);
      setTests((t) => ({ ...t, [host]: r }));
      if (r.host_keys.length) setTick((t) => t + 1);
    } catch (e) {
      setTests((t) => {
        const n = { ...t };
        delete n[host];
        return n;
      });
      flash((e as Error).message, "bad");
    }
  };

  const hosts = access?.known_hosts ?? [];
  const pending = Object.keys(tests).filter((h) => !hosts.some((k) => k.host === h));
  return (
    <>
      <TabHead
        title="Git & SSH"
        desc="One SSH setup for every project, so agents can clone, pull and push. MAP’s own key is the default; you can use this machine’s ~/.ssh instead."
        savesTo="<data_dir>/git/ (settings.json, id_ed25519, known_hosts, gitconfig)"
      />
      {error && <Note tone="bad">Could not read MAP&apos;s git access: {error}</Note>}
      {access && (
        <div className="flex flex-col gap-3.5">
          <Box>
            <Lbl>ssh method · used by agents and by MAP&apos;s own git commands</Lbl>
            <div className="grid gap-2 [grid-template-columns:repeat(auto-fit,minmax(min(260px,100%),1fr))]">
              <RadioCard
                on={m === "map_key"}
                onClick={() => setMethod("map_key")}
                title="MAP’s own key"
                desc="A key MAP made for itself in <data_dir>/git/, with known_hosts pinned for GitHub and GitLab. Agents get MAP’s gitconfig as GIT_CONFIG_GLOBAL."
              />
              <RadioCard on={m === "system"} onClick={() => setMethod("system")} title="This machine’s ~/.ssh" desc="Plain ssh, as in your shell: ~/.ssh keys, config, known_hosts and ssh-agent. MAP sets no sshCommand." />
            </div>
          </Box>
          {m === "map_key" ? (
            <Box>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <span className="text-[14px] leading-none font-semibold">MAP&apos;s key</span>
                <span className={`${mono} text-[10.5px] leading-none text-muted`}>{access.key ? `${access.key.fingerprint} · made ${ago(access.key.created_at, now)}` : "no key yet"}</span>
                <div className="ml-auto flex gap-1.5">
                  {access.key && (
                    <button
                      onClick={() => {
                        void navigator.clipboard?.writeText(access.key!.public_key);
                        setCopied(true);
                        window.setTimeout(() => setCopied(false), 1500);
                      }}
                      className={btnCls}
                    >
                      {copied ? "Copied ✓" : "Copy public key"}
                    </button>
                  )}
                  <button onClick={makeKey} disabled={busy} className={btnCls}>
                    {access.key ? "Make a new key" : "Make a key"}
                  </button>
                </div>
              </div>
              {access.key && <code className={`rounded-lg border border-line bg-inset px-3 py-2.5 ${mono} text-[12px] leading-normal [overflow-wrap:anywhere] text-nav`}>{access.key.public_key}</code>}
              <span className="text-[11.5px] leading-normal text-text-2">
                Paste this public key into GitHub (Settings › SSH and GPG keys) or GitLab (Preferences › SSH Keys). The private half never leaves the server.
              </span>
            </Box>
          ) : (
            <Box>
              <span className="text-[14px] leading-none font-semibold">This machine&apos;s ssh · {access.system_ssh.dir}</span>
              <div className={`flex flex-wrap gap-x-[18px] gap-y-1.5 ${mono} text-[11.5px] leading-[1.4]`}>
                {(
                  [
                    [access.system_ssh.dir_exists, `${access.system_ssh.dir} found`, `${access.system_ssh.dir} missing`],
                    [access.system_ssh.config_exists, "config", "no config"],
                    [access.system_ssh.known_hosts_exists, "known_hosts", "no known_hosts"],
                    [access.system_ssh.agent_socket, "ssh-agent reachable (SSH_AUTH_SOCK)", "no ssh-agent"],
                  ] as const
                ).map(([ok, yes, no]) => (
                  <span key={yes} style={{ color: ok ? "#7ee2a8" : "var(--warn)" }}>
                    {ok ? "●" : "○"} {ok ? yes : no}
                  </span>
                ))}
              </div>
              {access.system_ssh.public_keys.map((k) => (
                <div key={k.file} className={`flex flex-wrap gap-x-3.5 gap-y-1.5 rounded-[7px] border border-line px-2.5 py-2 ${mono} text-[11.5px] leading-[1.3]`}>
                  <span className="font-semibold">{k.file}</span>
                  <span className="text-text-2">{k.key_type}</span>
                  <span className="text-muted">{k.fingerprint}</span>
                  <span className="text-muted">{k.comment}</span>
                </div>
              ))}
              <span className={`${mono} text-[11px] leading-[1.4] text-muted`}>public keys only; MAP never reads a private key or writes ~/.ssh</span>
            </Box>
          )}
          <Box>
            <Lbl>hosts · known_hosts</Lbl>
            {[
              // One row per host: its key types together (known_hosts has a line per type).
              ...[...new Set(hosts.map((h) => h.host))].map((host) => {
                const ks = hosts.filter((k) => k.host === host);
                return { host, type: ks.map((k) => k.key_type).join(" · "), fp: ks[0].fingerprint, pinned: ks.every((k) => k.pinned) };
              }),
              ...pending.map((h) => ({ host: h, type: "", fp: "recorded on first test", pinned: false })),
            ].map((h) => {
              const r = tests[h.host];
              return (
                <div key={h.host} className="flex flex-col gap-2 rounded-[9px] border border-line px-3 py-2.5">
                  <div className="flex flex-wrap items-center gap-x-3.5 gap-y-1.5">
                    <span className={`${mono} text-[13px] leading-none font-semibold`}>{h.host}</span>
                    <span className={`${mono} text-[11px] leading-none text-text-2`}>{h.type}</span>
                    <span className={`${mono} text-[11px] leading-none [overflow-wrap:anywhere] text-muted`}>{h.fp}</span>
                    <span
                      className={`rounded-[3px] border px-1.5 py-0.5 ${mono} text-[9px] leading-none font-medium tracking-[0.08em] uppercase`}
                      style={{ borderColor: h.pinned ? "rgba(74,222,128,.35)" : "rgba(242,193,78,.4)", color: h.pinned ? "#7ee2a8" : "var(--warn)" }}
                    >
                      {h.pinned ? "pinned" : "first use"}
                    </span>
                    <span className="ml-auto flex gap-1.5">
                      {!h.pinned && hosts.some((k) => k.host === h.host) && (
                        <button
                          onClick={() => gitApi.removeKnownHost(h.host).then(() => setTick((t) => t + 1), (e: Error) => flash(e.message, "bad"))}
                          className={btnCls}
                          title="Forget this host key; the next test records it again"
                        >
                          Forget
                        </button>
                      )}
                      <button onClick={() => test(h.host)} disabled={r === "running"} className={btnCls}>
                        {r === "running" ? "Testing…" : "Test"}
                      </button>
                    </span>
                  </div>
                  {r && r !== "running" && <Out text={`$ ${r.command}\n${r.output}`} ok={r.ok} />}
                </div>
              );
            })}
            <div className="flex flex-wrap gap-1.5">
              <input value={newHost} onChange={(e) => setNewHost(e.target.value)} placeholder="git.example.com or [host]:2222" className={`${inputCls} flex-[1_1_220px]`} />
              <button
                onClick={() => {
                  const h = newHost.trim();
                  if (!h) return;
                  setNewHost("");
                  void test(h);
                }}
                className={btnCls}
              >
                + Add host
              </button>
            </div>
            <span className={`${mono} text-[10.5px] leading-[1.4] text-muted`}>github.com and gitlab.com are pinned to their published keys; other hosts are trusted on first use (added by their first test)</span>
          </Box>
          <Box>
            <Lbl>git identity · author of agents&apos; and MAP&apos;s commits</Lbl>
            <div className="flex flex-wrap gap-2">
              <input value={id.name} onChange={(e) => setIdent({ ...id, name: e.target.value })} className={`${inputCls} flex-[1_1_200px]`} aria-label="Name" />
              <input value={id.email} onChange={(e) => setIdent({ ...id, email: e.target.value })} className={`${inputCls} flex-[1_1_240px]`} aria-label="Email" />
            </div>
            <span className={`${mono} text-[10.5px] leading-normal text-muted`}>
              gitconfig: {access.gitconfig_path}
              {access.includes.length ? ` · includes ${access.includes.join(", ")} after MAP’s defaults, so your settings win; a repo’s own config wins over both` : ""}
            </span>
          </Box>
        </div>
      )}
    </>
  );
}

// ---- project-only tabs

function ProjectOnly() {
  const { setScope } = useConfigure();
  const { project } = useApp();
  return (
    <Note>
      This is a per-project setting, stored with the project.{" "}
      {project !== null ? (
        <>
          Switch to{" "}
          <button onClick={() => setScope("project")} className="cursor-pointer font-semibold text-accent-soft">
            Project
          </button>{" "}
          to change it.
        </>
      ) : (
        "Pick a project in the switcher to change it."
      )}
    </Note>
  );
}

const LAND: Record<LandMode, [string, string]> = {
  merge: ["Merge", "Rebase onto the base branch and fast-forward it, one landing at a time. A conflict goes back to the agent."],
  pull_request: ["Pull request", "Push the branch to origin and open a pull request (gh) or merge request (glab) against the base."],
  keep: ["Keep", "Leave the branch for you to review and land from the dashboard."],
};

function ListEdit({ label, hint, items, placeholder, onChange }: { label: string; hint: string; items: string[]; placeholder: string; onChange: (v: string[]) => void }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Lbl>{label}</Lbl>
      <span className="text-[11px] leading-[1.4] text-muted">{hint}</span>
      {items.map((v, i) => (
        <div key={i} className="flex gap-1.5">
          <input value={v} placeholder={placeholder} onChange={(e) => onChange(items.map((x, j) => (j === i ? e.target.value : x)))} className={`${inputCls} flex-1`} />
          <button onClick={() => onChange(items.filter((_, j) => j !== i))} className="h-8 w-7 cursor-pointer rounded-md text-muted hover:text-bad" aria-label="Remove">
            ×
          </button>
        </div>
      ))}
      <button onClick={() => onChange([...items, ""])} className="h-[26px] cursor-pointer self-start rounded-md border border-dashed border-line-3 px-2.5 text-[11.5px] font-medium text-nav">
        + Add
      </button>
    </div>
  );
}

export function BranchesTab() {
  const { scope, slug, flash } = useConfigure();
  const { now } = useApp();
  const { value, patch } = useProjectSettings("branches", ["branches"], ["project settings (branches)"]);
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [out, setOut] = useState<{ text: string; ok: boolean } | null>(null);
  const [op, setOp] = useState<GitOp | null>(null);

  useEffect(() => {
    if (!slug) return;
    let live = true;
    gitApi.status(slug).then((s) => live && setStatus(s), () => {});
    return () => {
      live = false;
    };
  }, [slug]);

  if (scope === "machine" || !slug || !value)
    return (
      <>
        <TabHead title="Repo & branches" desc="The project folder’s git state, and whether tasks share that folder or each get their own branch and worktree." savesTo="project settings in the platform database (branches)" />
        <ProjectOnly />
      </>
    );
  const B = value.branches;
  const set = (p: Partial<typeof B>) => patch({ branches: { ...B, ...p } });
  const run = async (o: GitOp) => {
    if (status?.busy) {
      flash(status.busy, "warn");
      return;
    }
    setOp(o);
    setOut(null);
    try {
      const r = await gitApi.run(slug, o);
      setStatus(r.status);
      setOut({ text: `$ ${r.command}\n${r.output}`, ok: r.ok });
    } catch (e) {
      setOut({ text: (e as Error).message, ok: false });
    } finally {
      setOp(null);
    }
  };
  return (
    <>
      <TabHead title="Repo & branches" desc="The project folder’s git state, and whether tasks share that folder or each get their own branch and worktree." savesTo="project settings in the platform database (branches)" />
      <div className="flex flex-col gap-3.5">
        <Box>
          {status && !status.is_repo ? (
            <span className="text-[12.5px] text-text-2">The project folder is not a git repository yet.</span>
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-x-3.5 gap-y-2">
                <span className={`${mono} text-[15px] leading-none font-semibold`}>⑂ {status?.branch ?? "…"}</span>
                <span className={`${mono} text-[11.5px] leading-none text-text-2`}>→ {status?.upstream ?? "no upstream"}</span>
                <div className="ml-auto flex gap-1.5">
                  {(
                    [
                      ["fetch", "Fetch"],
                      ["pull", "Pull"],
                      ["push", "Push"],
                    ] as const
                  ).map(([o, l]) => (
                    <button key={o} onClick={() => run(o)} disabled={!!op} className={btnCls}>
                      {op === o ? `${l}ing…` : l}
                    </button>
                  ))}
                </div>
              </div>
              {status && (
                <div className={`flex flex-wrap gap-x-[18px] gap-y-1.5 ${mono} text-[11.5px] leading-[1.4]`}>
                  <span style={{ color: status.ahead ? "var(--warn)" : "var(--muted)" }}>↑{status.ahead ?? 0} ahead</span>
                  <span style={{ color: status.behind ? "var(--warn)" : "var(--muted)" }}>↓{status.behind ?? 0} behind</span>
                  <span style={{ color: status.dirty ? "var(--warn)" : "#7ee2a8" }}>{status.dirty} changed files</span>
                  <span className="text-text-2">fetched {ago(status.last_fetch, now)}</span>
                </div>
              )}
              {status && (
                <span className={`${mono} text-[11.5px] leading-[1.4] [overflow-wrap:anywhere] text-muted`}>
                  {status.remote_url ?? "no remote"}
                  {status.last_commit ? ` · ${status.last_commit.sha.slice(0, 7)} ${status.last_commit.subject} · ${ago(status.last_commit.at, now)}` : ""}
                </span>
              )}
              {out && <Out text={out.text} ok={out.ok} />}
            </>
          )}
        </Box>
        <Box>
          <Lbl>where tasks run</Lbl>
          <div className="grid gap-2 [grid-template-columns:repeat(auto-fit,minmax(min(260px,100%),1fr))]">
            <RadioCard
              on={B.mode === "shared"}
              onClick={() => set({ mode: "shared" })}
              title="Shared folder"
              desc="Every task works in the project folder on its checked-out branch. One task at a time, unless the parallel analyst clears two."
            />
            <RadioCard
              on={B.mode === "worktree"}
              onClick={() => set({ mode: "worktree" })}
              title="A branch and worktree per task"
              desc="Each task gets branch map/task-<id> in its own worktree, so several run at once. MAP commits and lands each one when it passes."
            />
          </div>
        </Box>
        {B.mode === "worktree" && (
          <Box>
            <div className="flex flex-wrap gap-x-6 gap-y-3.5">
              <label className="flex flex-col gap-1.5">
                <Lbl>base branch</Lbl>
                <input value={B.base ?? ""} onChange={(e) => set({ base: e.target.value.trim() || null })} placeholder="the branch checked out" className={`${inputCls} w-[200px]`} />
              </label>
              <div className="flex flex-col gap-1.5">
                <Lbl>when a task passes</Lbl>
                <Seg value={B.land} options={(Object.keys(LAND) as LandMode[]).map((k) => [k, LAND[k][0]] as [LandMode, string])} onChange={(land) => set({ land })} size="md" />
              </div>
            </div>
            <span className="text-[11.5px] leading-normal text-text-2">{LAND[B.land][1]}</span>
            <span className={`${mono} text-[10.5px] leading-[1.4] text-muted`}>each task: &lt;data_dir&gt;/projects/{slug}/worktrees/task-&lt;id&gt; on branch map/task-&lt;id&gt;</span>
            <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fit,minmax(min(300px,100%),1fr))]">
              <ListEdit label="setup · run in each new worktree" hint="before the agent starts, e.g. bun install" placeholder="bun install" items={B.setup} onChange={(setup) => set({ setup })} />
              <ListEdit label="copy · untracked files into each worktree" hint="e.g. .env; never committed by MAP" placeholder=".env" items={B.copy} onChange={(copy) => set({ copy })} />
            </div>
          </Box>
        )}
      </div>
    </>
  );
}

export function ParallelTab() {
  const { scope, slug } = useConfigure();
  const { orchestrator, providers } = useApp();
  const { value, patch } = useProjectSettings("parallel", ["parallel"], ["project settings (parallel)"]);
  const { value: other } = useProjectSettings("parallel-ctx", ["branches"], []);
  const [est, setEst] = useState<WorktreeEstimate | null>(null);
  const n = value?.parallel.max ?? orchestrator?.max_parallel ?? 4;

  useEffect(() => {
    if (!slug || scope === "machine") return;
    let live = true;
    const t = window.setTimeout(() => {
      api.worktreeEstimate(slug, { parallel: n }).then((e) => live && setEst(e), () => {});
    }, 300);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [slug, scope, n]);

  const machineCap = (
    <Box>
      <Lbl>machine · tasks at once across every project ([orchestrator] max_parallel)</Lbl>
      <div className="flex flex-wrap items-center gap-2.5">
        <Stepper
          value={orchestrator?.max_parallel ?? "…"}
          onMinus={() => orchestrator && api.updateOrchestrator({ max_parallel: Math.max(1, orchestrator.max_parallel - 1) })}
          onPlus={() => orchestrator && api.updateOrchestrator({ max_parallel: Math.min(16, orchestrator.max_parallel + 1) })}
        />
        <span className="text-[11.5px] text-text-2">saved at once{orchestrator && orchestrator.max_parallel !== orchestrator.config_max_parallel ? ` · map.toml says ${orchestrator.config_max_parallel}` : ""}</span>
      </div>
    </Box>
  );
  if (scope === "machine" || !slug || !value || !other)
    return (
      <>
        <TabHead title="Parallel" desc="How many of this project’s tasks run at once, and who decides which tasks may share the folder." savesTo="project settings (parallel) · machine cap: [orchestrator] max_parallel" />
        {machineCap}
        <ProjectOnly />
      </>
    );
  const Q = value.parallel;
  const set = (p: Partial<typeof Q>) => patch({ parallel: { ...Q, ...p } });
  const worktree = other.branches.mode === "worktree";
  const mem = est ? est.total_memory : 0;
  const memAvail = est?.machine.memory_available ?? est?.machine.memory_total ?? 0;
  const disk = est ? est.total_disk : 0;
  const diskFree = est?.machine.disk_free ?? 0;
  const bar = (label: string, used: number, avail: number, note: string) => {
    const c = avail && used > avail ? "#F26D6D" : avail && used > avail * 0.7 ? "#F2C14E" : "#7ee2a8";
    return (
      <div key={label} className="flex flex-col gap-1.5">
        <div className="flex justify-between gap-2.5 text-[12px] leading-[1.2] font-medium">
          <span>{label}</span>
          <span className={`${mono} text-[12.5px] leading-none font-semibold`} style={{ color: c }}>
            {fmtGB(used)} of {avail ? fmtGB(avail) : "?"} free
          </span>
        </div>
        <div className="h-2 overflow-hidden rounded bg-white/[.06]">
          <span className="block h-full rounded" style={{ width: `${avail ? Math.min(100, (used / avail) * 100) : 0}%`, background: c }} />
        </div>
        <span className={`${mono} text-[10.5px] leading-[1.4] text-muted`}>{note}</span>
      </div>
    );
  };
  return (
    <>
      <TabHead title="Parallel" desc="How many of this project’s tasks run at once, and who decides which tasks may share the folder." savesTo="project settings (parallel) · machine cap: [orchestrator] max_parallel" />
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fit,minmax(min(360px,100%),1fr))]">
        <Box>
          <Lbl>tasks at once in this project</Lbl>
          <div className="flex flex-wrap items-center gap-2.5">
            <Stepper value={Q.max ?? "∞"} width={44} onMinus={() => set({ max: Math.max(1, (Q.max ?? 4) - 1) })} onPlus={() => set({ max: Math.min(12, (Q.max ?? 3) + 1) })} />
            <button
              onClick={() => set({ max: null })}
              className={`h-7 cursor-pointer rounded-md border px-2.5 ${mono} text-[11.5px] font-medium`}
              style={{ borderColor: Q.max === null ? "var(--accent)" : "var(--line-3)", color: Q.max === null ? "var(--accent-soft)" : "var(--text-2)" }}
            >
              no project limit
            </button>
          </div>
          <span className="text-[11.5px] leading-normal text-text-2">
            {worktree
              ? "Worktree mode: every task has its own branch, so this cap is the only limit (plus the machine-wide max_parallel)."
              : "Shared folder: a second task starts beside a running one only when the analyst has cleared both."}
          </span>
          <div className="h-px bg-line" />
          <div className="flex items-center gap-2.5">
            <Switch on={Q.analyst} onClick={() => set({ analyst: !Q.analyst })} />
            <span className="text-[13px] leading-[1.3] font-semibold">Parallel analyst</span>
          </div>
          <span className="text-[11.5px] leading-normal text-text-2">
            In the shared folder, an agent reads waiting tasks and the code, declares the paths each will touch, and lets two run side by side only when those paths do not overlap. Off: one
            task at a time.
          </span>
          <label className="flex flex-col gap-1.5">
            <Lbl>analyst provider</Lbl>
            <select value={Q.analyst_provider ?? ""} onChange={(e) => set({ analyst_provider: e.target.value || null })} className={`${inputCls} font-medium`}>
              <option value="">audit provider (default)</option>
              {providers.map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
          </label>
        </Box>
        <Box>
          <Lbl>worktree estimate · {n} tasks at once</Lbl>
          {!est && <span className="text-[12px] text-muted">measuring the checkout…</span>}
          {est && (
            <>
              {bar("Memory", mem, memAvail, `per task ${fmtGB(est.per_task_memory)}: ${est.agents.map((a) => `${a.provider} ${fmtMB(a.bytes)}${a.measured ? " (measured)" : ""}`).join(" + ")} + build ${fmtGB(est.build_memory)} (${est.build_note})`)}
              {bar("Disk", disk, diskFree, `per task ${fmtGB(est.per_task_disk)}: checkout ${fmtMB(est.checkout_bytes)} (${est.tracked_files.toLocaleString()} files)${est.extra.map((x) => ` + ${x.path} ${fmtMB(x.bytes)}`).join("")}`)}
              <span className={`${mono} text-[10.5px] leading-normal text-muted`}>
                machine: {est.machine.cpus} CPUs · {fmtGB(est.machine.memory_total)} RAM · recommended {est.recommended_parallel} at once ({est.limited_by}) · rough by design
              </span>
              {est.drawbacks.map((d) => (
                <span key={d.title} className="text-[11.5px] leading-[1.45]" style={{ color: d.level === "error" ? "var(--bad)" : d.level === "warning" ? "var(--warn)" : "var(--text-2)" }}>
                  {d.title}: {d.detail}
                </span>
              ))}
            </>
          )}
        </Box>
      </div>
      {machineCap}
    </>
  );
}
