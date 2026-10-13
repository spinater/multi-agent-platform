"use client";

import clsx from "clsx";
import { Pause, Play } from "lucide-react";
import { useEffect, useState } from "react";

import { api, setToken } from "@/lib/api";
import { useDashboard, type Connection } from "@/lib/use-dashboard";

import { SessionConsole } from "./console";
import { EventFeed } from "./events";
import { NewSessionForm, SessionList } from "./sessions";
import { NewTaskForm, TaskBoard } from "./tasks";
import { Badge, Button, Input, relative } from "./ui";

const connectionLabel: Record<Connection, [string, string]> = {
  connecting: ["Connecting", "bg-info"],
  live: ["Live", "bg-ok"],
  offline: ["Offline — retrying", "bg-bad"],
  unauthorized: ["Token required", "bg-warn"],
};

function TokenGate({ onSaved }: { onSaved: () => void }) {
  const [value, setValue] = useState("");
  return (
    <form
      className="mx-auto mt-24 flex max-w-sm flex-col gap-3 rounded-lg border border-line bg-panel p-6"
      onSubmit={(e) => {
        e.preventDefault();
        setToken(value.trim());
        onSaved();
      }}
    >
      <h1 className="text-base font-semibold">API token</h1>
      <p className="text-sm text-muted">This server requires the token from its config file (server.api_token).</p>
      <Input type="password" value={value} onChange={(e) => setValue(e.target.value)} autoFocus />
      <Button variant="primary" type="submit" disabled={!value.trim()}>
        Connect
      </Button>
    </form>
  );
}

export function Dashboard() {
  const { sessions, tasks, providers, orchestrator, events, connection } = useDashboard();
  const [selected, setSelected] = useState<string | null>(null);
  const [form, setForm] = useState<"session" | "task" | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);

  if (connection === "unauthorized") return <TokenGate onSaved={() => location.reload()} />;

  const current = sessions.find((s) => s.name === selected) ?? null;
  const [label, color] = connectionLabel[connection];
  const open = (name: string) => {
    setSelected(name);
    setForm(null);
  };

  return (
    <div className="mx-auto flex w-full max-w-[1600px] flex-col gap-4 px-4 py-4 sm:px-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-semibold tracking-tight">Multi-Agent Platform</h1>
          <span className="flex items-center gap-1.5 text-xs text-muted">
            <span className={clsx("size-2 rounded-full", color)} />
            {label}
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {providers.map((p) => (
            <Badge key={p.name} tone={!p.available ? "idle" : p.cooldown_until ? "warn" : "ok"}>
              {p.name}
              {!p.available ? " · missing" : p.cooldown_until ? ` · back ${relative(p.cooldown_until, now)}` : ""}
            </Badge>
          ))}
          {orchestrator && (
            <Button
              variant={orchestrator.running ? "ghost" : "primary"}
              onClick={() => api.setOrchestrator(!orchestrator.running)}
              title={orchestrator.running ? "Stop starting new tasks (running ones continue)" : "Start running ready tasks"}
            >
              {orchestrator.running ? <Pause className="size-4" /> : <Play className="size-4" />}
              {orchestrator.running ? "Pause orchestrator" : "Run orchestrator"}
              <span className="text-xs opacity-75">
                {orchestrator.active}/{orchestrator.max_parallel}
              </span>
            </Button>
          )}
        </div>
      </header>

      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_380px]">
        <div className="flex min-w-0 flex-col gap-4">
          {form === "session" && <NewSessionForm providers={providers} onClose={() => setForm(null)} onDone={open} />}
          {form === "task" && <NewTaskForm tasks={tasks} providers={providers} onClose={() => setForm(null)} />}
          <TaskBoard tasks={tasks} onOpenSession={open} onCreate={() => setForm("task")} />
          <SessionList sessions={sessions} selected={selected} onSelect={open} now={now} onCreate={() => setForm("session")} />
        </div>
        <div className="min-w-0">
          <SessionConsole key={current?.name ?? "none"} session={current} onClosed={() => setSelected(null)} />
        </div>
        <div className="min-w-0">
          <EventFeed events={events} onOpenSession={open} />
        </div>
      </div>
    </div>
  );
}
