"use client";

import clsx from "clsx";
import { ExternalLink, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { api } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { LoginState } from "@/lib/generated/LoginState";
import { attachCommand } from "@/lib/tmux";
import { refreshProviders } from "@/lib/use-dashboard";

import { useApp } from "./app-context";
import { Btn, Inset, Segmented, TextInput, errText } from "./project-ui";

type Phase = "start" | "waiting" | "done";

/** Provider sign-in (the auth modal in Projects.dc.html). "Browser sign-in" runs the provider's own login command in
 * a helper tmux session (`<prefix>login-<provider>`), shows the URL and one-time code parsed from its pane, and can
 * type into it. The token stays with the CLI; the dashboard never sees it. "API key" explains where to set one: the
 * dashboard does not take secrets. */
export function AuthModal() {
  const { authProvider, closeAuth } = useApp();
  if (!authProvider) return null;
  return <AuthDialog key={authProvider} provider={authProvider} onClose={closeAuth} />;
}

function AuthDialog({ provider, onClose }: { provider: string; onClose: () => void }) {
  const { providers, toast, now, server } = useApp();
  const info = providers.find((p) => p.name === provider);
  const login = info?.login;
  const [method, setMethod] = useState<"browser" | "key">("browser");
  const [phase, setPhase] = useState<Phase>("start");
  const [state, setState] = useState<LoginState | null>(null);
  // The credential files' newest change when the sign-in started; a later one means it worked.
  const [before, setBefore] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  useEffect(() => {
    if (phase !== "waiting") return;
    let stopped = false;
    const poll = async () => {
      try {
        const s = await api.loginState(provider);
        if (stopped) return;
        setState(s);
        const fresh = s.signed_in === true && s.credentials_updated_at !== null && s.credentials_updated_at !== before;
        if (s.success || fresh) {
          setPhase("done");
          void refreshProviders();
          toast(`${provider} signed in · new tasks can use it`);
        }
      } catch (e) {
        if (!stopped) setError(errText(e));
      }
    };
    const t = window.setInterval(poll, 1500);
    void poll();
    return () => {
      stopped = true;
      window.clearInterval(t);
    };
  }, [phase, provider, before, toast]);

  const session = state?.session ?? null;
  const live = !!session && session.state !== "exited";

  const start = async () => {
    setBusy(true);
    setError("");
    try {
      const s = await api.loginStart(provider);
      setState(s);
      setBefore(s.credentials_updated_at);
      setPhase("waiting");
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  /** Ends the helper session (unless a person is attached to it), then closes. */
  const finish = async () => {
    if (live && session) await api.deleteSession(session.name).catch((e) => toast(errText(e), "warn"));
    onClose();
  };

  const send = async (fn: () => Promise<unknown>) => {
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(errText(e));
    }
  };

  const dots = ".".repeat(1 + (Math.floor(now / 1000) % 3));
  const lines = (state?.text ?? "")
    .split("\n")
    .map((l) => l.trimEnd())
    .filter(Boolean)
    .slice(-10);
  const why =
    info?.signed_in === false
      ? `${provider} looks signed out: none of its credential files is there. Its sessions stop at a sign-in prompt until you sign in.`
      : info?.signed_in === true
        ? `${provider} is signed in. Sign in again to switch accounts or renew an expired sign-in.`
        : `The platform cannot tell whether ${provider} is signed in (no credential file is configured for it).`;

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-[rgba(3,4,6,.72)] p-5 backdrop-blur-sm" onClick={onClose}>
      <div
        role="dialog"
        aria-label={`Sign in to ${provider}`}
        onClick={(e) => e.stopPropagation()}
        className="flex max-h-[calc(100vh-40px)] w-full max-w-[480px] flex-col overflow-auto rounded-[14px] border border-white/10 bg-menu shadow-[0_30px_80px_rgba(0,0,0,.6)]"
      >
        <div className="flex items-center justify-between gap-2.5 border-b border-line px-[18px] py-4">
          <span className="flex items-center gap-2 text-[15px] font-semibold whitespace-nowrap">
            <span className="size-[9px] rounded-full" style={{ background: providerColor(provider) }} />
            Sign in to {provider}
          </span>
          <button onClick={onClose} className="flex size-[26px] items-center justify-center rounded-md border border-white/10 text-nav hover:text-text" aria-label="Close">
            <X className="size-3.5" />
          </button>
        </div>

        <div className="flex flex-col gap-3.5 p-[18px]">
          {phase === "start" && (
            <>
              <p className="text-[13px] leading-normal text-text-2">{why}</p>
              <Segmented
                value={method}
                options={[
                  ["browser", "Browser sign-in"],
                  ["key", "API key"],
                ]}
                onChange={setMethod}
              />
              {method === "browser" && login && login.command && (
                <div className="flex flex-col gap-3">
                  <Step n={1}>
                    The dashboard runs <span className="font-mono text-accent-soft">{login.command}</span>
                    {login.send && (
                      <>
                        {" "}
                        and types <span className="font-mono text-accent-soft">{login.send}</span>
                      </>
                    )}{" "}
                    in a helper tmux session, <span className="font-mono text-text-2">{login.session}</span>.
                  </Step>
                  <Step n={2}>
                    Approve the request in your browser with the URL and code it shows. The CLI keeps the token in{" "}
                    <span className="font-mono break-all text-text-2">{login.credentials.join(", ") || "its own store"}</span>; the dashboard never sees it.
                  </Step>
                  {login.warning && <p className="rounded-lg border border-warn/35 bg-warn/[.07] px-3 py-2 text-[12.5px] leading-normal text-warn">{login.warning}</p>}
                </div>
              )}
              {method === "browser" && !login?.command && (
                <p className="text-[13px] leading-normal text-text-2">
                  No login command is set for {provider}. Add <span className="font-mono">login = [&quot;…&quot;]</span> under <span className="font-mono">[providers.{provider}]</span> in map.toml,
                  or sign in from a terminal on this machine.
                </p>
              )}
              {method === "key" && (
                <div className="flex flex-col gap-2.5 text-[13px] leading-normal text-text-2">
                  <p>
                    The dashboard does not take API keys: a key typed into a web page could end up in logs and browser history. Set{" "}
                    <span className="font-mono text-accent-soft">{login?.api_key_env ?? "the provider's key variable"}</span> instead:
                  </p>
                  <Inset className="gap-0.5">
                    <span className="text-muted"># map.toml: exported into every agent session and verify command</span>
                    <span>[workspace.env]</span>
                    <span>
                      {login?.api_key_env ?? "API_KEY"} = <span className="text-accent-soft">&quot;…&quot;</span>
                    </span>
                  </Inset>
                  <p>or in the provider&apos;s own config, then restart the server.</p>
                  <p className={clsx("font-mono text-[11.5px]", login?.api_key_set ? "text-ok" : "text-muted")}>
                    {login?.api_key_env ? `${login.api_key_env} is ${login.api_key_set ? "set" : "not set"} for this server` : "No key variable is configured for this provider."}
                  </p>
                </div>
              )}
            </>
          )}

          {phase === "waiting" && (
            <>
              {state?.code ? (
                <div className="flex flex-col items-center gap-2.5 rounded-[10px] border border-accent/35 bg-accent/[.06] p-4">
                  <span className="font-mono text-[11px] tracking-[0.14em] text-muted uppercase">one-time code</span>
                  <span className="font-mono text-[30px] leading-none font-semibold tracking-[0.12em] text-accent-soft select-all">{state.code}</span>
                  {state.url && <UrlLink url={state.url} />}
                </div>
              ) : state?.url ? (
                <div className="flex flex-col items-center gap-2.5 rounded-[10px] border border-accent/35 bg-accent/[.06] p-4">
                  <span className="font-mono text-[11px] tracking-[0.14em] text-muted uppercase">open to sign in</span>
                  <UrlLink url={state.url} />
                </div>
              ) : (
                <p className={clsx("font-mono text-[12px]", state?.phase === "console" || state?.phase === "failed" ? "text-warn" : "text-info")}>
                  {live
                    ? state?.hint
                      ? `${state.hint}${state.phase === "onboarding" ? dots : ""}`
                      : `Waiting for ${login?.command} to show a sign-in URL${dots}`
                    : session
                      ? "The login session has ended."
                      : `Starting${dots}`}
                </p>
              )}
              {state?.wants_code && live && session && (
                <div className="flex gap-2">
                  <TextInput value={code} onChange={(e) => setCode(e.target.value)} placeholder="paste the code from the browser" aria-label="Authorization code" />
                  <Btn
                    tone="primary"
                    disabled={!code.trim()}
                    onClick={() =>
                      send(async () => {
                        await api.loginCode(provider, code.trim());
                        setCode("");
                      })
                    }
                  >
                    Send
                  </Btn>
                </div>
              )}
              {lines.length > 0 && (
                <Inset className={clsx("gap-0 text-[11px]", state?.phase === "console" && "ring-1 ring-warn/60")}>
                  <span className="text-muted">$ {login?.command}</span>
                  {lines.map((l, i) => (
                    <span key={i} className="break-all whitespace-pre-wrap text-text-2">
                      {l}
                    </span>
                  ))}
                </Inset>
              )}
              {live && session && (
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className="mr-1 font-mono text-[10.5px] tracking-[0.1em] text-muted uppercase">keys</span>
                  {(
                    [
                      ["Enter", "Enter"],
                      ["Up", "↑"],
                      ["Down", "↓"],
                      ["Escape", "Esc"],
                      ["C-c", "Ctrl-C"],
                    ] as const
                  ).map(([k, label]) => (
                    <Btn key={k} size="sm" tone="quiet" onClick={() => send(() => api.key(session.name, k))}>
                      {label}
                    </Btn>
                  ))}
                  <span className="font-mono text-[10.5px] text-muted">or {attachCommand(server?.tmux_socket, session.name)}</span>
                </div>
              )}
              <p className="font-mono text-[11px] text-muted">
                {state?.signed_in === true ? "a credential file is present" : "no valid credential file yet"} · {login?.credentials.join(", ")}
              </p>
            </>
          )}

          {phase === "done" && (
            <div className="flex items-center gap-3 rounded-[10px] border border-ok/35 bg-ok/[.07] px-4 py-3.5">
              <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-ok text-[15px] font-bold text-accent-text">✓</span>
              <span className="flex flex-col gap-0.5">
                <span className="text-sm font-semibold">Signed in</span>
                <span className="text-xs text-text-2">{provider} is ready · new tasks can use it again</span>
              </span>
            </div>
          )}
          {error && <p className="text-[12px] text-bad">{error}</p>}
        </div>

        <div className="flex justify-end gap-2 border-t border-line px-[18px] py-3.5">
          {phase === "start" && (
            <>
              <Btn onClick={onClose}>Cancel</Btn>
              {method === "browser" && login?.command && info?.available && (
                <Btn tone="primary" onClick={start} disabled={busy}>
                  {busy ? "Starting…" : "Start sign-in"}
                </Btn>
              )}
            </>
          )}
          {phase === "waiting" && (
            <>
              <Btn onClick={finish}>Cancel</Btn>
              <Btn
                tone="primary"
                onClick={async () => {
                  const s = await api.loginState(provider).catch(() => null);
                  if (s?.signed_in) {
                    setPhase("done");
                    void refreshProviders();
                  }
                  else setError(`${provider} has no valid credential file yet. Finish the sign-in in the browser, or check the session's terminal.`);
                }}
              >
                I&apos;ve approved it
              </Btn>
            </>
          )}
          {phase === "done" && (
            <Btn tone="primary" onClick={finish}>
              Done
            </Btn>
          )}
        </div>
      </div>
    </div>
  );
}

function Step({ n, children }: { n: number; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[22px_minmax(0,1fr)] items-start gap-2.5">
      <span className="flex size-[22px] items-center justify-center rounded-full border border-accent font-mono text-[10px] font-semibold text-accent-soft">{n}</span>
      <span className="text-[13px] leading-normal text-text">{children}</span>
    </div>
  );
}

function UrlLink({ url }: { url: string }) {
  return (
    <a href={url} target="_blank" rel="noreferrer noopener" className="flex items-start gap-1.5 text-center font-mono text-[11.5px] leading-snug break-all">
      {url}
      <ExternalLink className="mt-0.5 size-3 shrink-0" />
    </a>
  );
}
