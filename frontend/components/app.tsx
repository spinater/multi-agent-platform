"use client";

import clsx from "clsx";
import { useEffect, useState } from "react";

import { setToken } from "@/lib/api";

import { AppProvider, useApp } from "./app-context";
import { AuthModal } from "./auth-modal";
import { AgentsScreen } from "./screens/agents";
import { CommandScreen } from "./screens/command";
import { DecisionsScreen } from "./screens/decisions";
import { ProjectsScreen } from "./screens/projects";
import { ReflexScreen } from "./screens/reflex";
import { SettingsScreen } from "./screens/settings";
import { SkillsScreen } from "./screens/skills";
import { UsageScreen } from "./screens/usage";
import { Button, Input } from "./ui";
import { UpdateNotifier } from "./updates-bell";

function TokenGate() {
  const [value, setValue] = useState("");
  return (
    <form
      className="mx-auto mt-24 flex w-[calc(100%-32px)] max-w-sm flex-col gap-3 rounded-xl border border-line-2 bg-menu p-6"
      onSubmit={(e) => {
        e.preventDefault();
        setToken(value.trim());
        location.reload();
      }}
    >
      <h1 className="label text-text">API token</h1>
      <p className="text-[12px] text-text-2">This server requires the token from its config file (server.api_token).</p>
      <Input type="password" value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
      <Button variant="primary" type="submit" disabled={!value.trim()}>
        Connect
      </Button>
    </form>
  );
}

function Toasts() {
  const { toasts } = useApp();
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-5 z-[60] flex flex-col items-center gap-2 px-4">
      {toasts.map((t) => (
        <div
          key={t.id}
          role="status"
          className={clsx(
            "rounded-lg border bg-menu px-4 py-2 font-mono text-[12px] shadow-[0_12px_36px_rgba(0,0,0,.45)]",
            t.tone === "ok" ? "border-ok/40 text-ok" : t.tone === "warn" ? "border-warn/40 text-warn" : "border-bad/40 text-bad",
          )}
        >
          {t.text}
        </div>
      ))}
    </div>
  );
}

function Router() {
  const { route, connection } = useApp();
  if (connection === "unauthorized") return <TokenGate />;
  switch (route.screen) {
    case "projects":
      return <ProjectsScreen />;
    case "skills":
      return <SkillsScreen />;
    case "agents":
      return <AgentsScreen />;
    case "decisions":
      return <DecisionsScreen />;
    case "reflex":
      return <ReflexScreen />;
    case "usage":
      return <UsageScreen />;
    case "settings":
      return <SettingsScreen />;
    default:
      return <CommandScreen />;
  }
}

/** The dashboard. Nothing renders until mounted: the static export is built ahead of time, and the clock, the hash
 * route and `localStorage` only exist in the browser. */
export function App() {
  const [now, setNow] = useState<number | null>(null);
  useEffect(() => {
    const tick = () => setNow(Date.now());
    const first = window.setTimeout(tick, 0);
    const t = window.setInterval(tick, 1000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(t);
    };
  }, []);
  if (now === null) return <p className="label p-6">loading…</p>;
  return (
    <AppProvider now={now}>
      <Router />
      <AuthModal />
      <UpdateNotifier />
      <Toasts />
    </AppProvider>
  );
}
