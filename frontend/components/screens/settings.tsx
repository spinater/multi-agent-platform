"use client";

import clsx from "clsx";
import { Bot, KeyRound, Send, Server, Zap } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

import { gitApi } from "@/lib/api";
import type { SettingsInfo } from "@/lib/generated/SettingsInfo";

import { useT } from "@/lib/i18n";

import { useApp } from "../app-context";
import { errText } from "../project-ui";
import { AgentClis } from "../settings/agent-clis";
import { GitSshSettings } from "../settings/git-ssh";
import { ReflexGatesSettings } from "../settings/reflex-gates";
import { ServerSettingsView } from "../settings/server";
import { TelegramSettings } from "../settings/telegram";
import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";

const SECTIONS = [
  { id: "git", label: "Git & SSH", sub: "GitHub, GitLab, any ssh host", icon: KeyRound },
  { id: "agents", label: "Agent CLIs", sub: "versions, sign-in, updates", icon: Bot },
  { id: "gates", label: "Reflex gates", sub: "shared Reflex, threshold, corpus", icon: Zap },
  { id: "telegram", label: "Telegram", sub: "notifications, /watch", icon: Send },
  { id: "server", label: "Server", sub: "map.toml, read only", icon: Server },
] as const;

type SectionId = (typeof SECTIONS)[number]["id"];

const isSection = (s: string | null): s is SectionId => SECTIONS.some((x) => x.id === s);

/** Settings (`#settings`, `#settings/git`, `#settings/agents`, `#settings/gates`, `#settings/telegram`, `#settings/server`): what someone who installs MAP
 * sets up once. Git & SSH is editable (the SSH method, MAP's key, hosts, identity); Agent CLIs reuse the sign-in
 * modal and the update flow; Telegram and Server are `map.toml`, read only. No secret is ever taken or shown here. */
export function SettingsScreen() {
  const { route, go } = useApp();
  const t = useT();
  const [settings, setSettings] = useState<SettingsInfo | null>(null);
  const [error, setError] = useState("");
  const active: SectionId = isSection(route.arg) ? route.arg : "git";

  useEffect(() => {
    let stale = false;
    gitApi.settings().then(
      (s) => !stale && setSettings(s),
      (e) => !stale && setError(errText(e)),
    );
    return () => {
      stale = true;
    };
  }, []);

  // A sub-route scrolls to its section (after `go` scrolled to the top).
  useEffect(() => {
    if (!isSection(route.arg)) return;
    const t = window.setTimeout(() => document.getElementById(`settings-${route.arg}`)?.scrollIntoView({ behavior: "smooth", block: "start" }), 30);
    return () => window.clearTimeout(t);
  }, [route.arg]);

  return (
    <>
      <TopBar />
      <Main className="gap-7">
        <PageHeader
          kicker="Configuration"
          title={t("nav.settings")}
          lead="Set up this MAP once: how agents reach GitHub, GitLab or your own git host, the agent CLIs they run, the Reflex gates, and the server's own settings. Keys, tokens and passwords are never typed or shown here: only public keys, and whether a secret is set."
        />
        <div className="grid min-w-0 gap-6 lg:grid-cols-[220px_minmax(0,1fr)] lg:gap-8">
          <nav aria-label="Settings sections" className="flex min-w-0 flex-wrap gap-1.5 self-start lg:sticky lg:top-20 lg:flex-col lg:gap-1">
            {SECTIONS.map((s) => (
              <a
                key={s.id}
                href={`#settings/${s.id}`}
                onClick={(e) => {
                  e.preventDefault();
                  go("settings", s.id);
                }}
                aria-current={active === s.id ? "true" : undefined}
                className={clsx(
                  "flex min-w-0 items-center gap-2.5 rounded-lg border px-3 py-2 no-underline lg:py-2.5",
                  active === s.id ? "border-accent/40 bg-accent-tint text-accent-soft hover:text-accent-soft" : "border-line text-nav hover:border-line-3 hover:text-text",
                )}
              >
                <s.icon className="size-3.5 shrink-0" />
                <span className="flex min-w-0 flex-col">
                  <span className="text-[12.5px] font-semibold whitespace-nowrap">{t(`set.${s.id}`)}</span>
                  <span className="hidden font-mono text-[10.5px] text-muted lg:block">{s.sub}</span>
                </span>
              </a>
            ))}
          </nav>
          <div className="flex min-w-0 flex-col gap-12">
            <Section id="git" title="Git & SSH" lead="One SSH setup for every project, so agents can clone, pull and push. MAP's own key is the default; you can use this machine's ~/.ssh instead.">
              <GitSshSettings settings={settings} showCli />
            </Section>
            <Section id="agents" title="Agent CLIs" lead="The coding-agent CLIs MAP runs: where each binary is, its version against the latest release, and its sign-in.">
              <AgentClis settings={settings} />
            </Section>
            <Section
              id="gates"
              title="Reflex gates"
              lead="Before an LLM, the orchestrator can ask Reflex: is this task clear, does it need planning, which model, does it need an audit. These gates have their own Reflex, shared by every project; each project turns them on in its settings."
            >
              <ReflexGatesSettings />
            </Section>
            <Section id="telegram" title="Telegram" lead="Lane reports and alerts go to one Telegram chat, and /watch or /status there talk back to MAP. Read only: these come from map.toml's [telegram].">
              <TelegramSettings />
            </Section>
            <Section id="server" title="Server" lead="How this server is set up. Read only: these come from map.toml.">
              <ServerSettingsView settings={settings} error={error} />
            </Section>
          </div>
        </div>
      </Main>
    </>
  );
}

function Section({ id, title, lead, children }: { id: SectionId; title: string; lead: string; children: ReactNode }) {
  return (
    <section id={`settings-${id}`} className="flex min-w-0 scroll-mt-[124px] flex-col gap-4 lg:scroll-mt-20">
      <div className="flex flex-col gap-1.5">
        <h2 className="text-[20px] leading-tight font-semibold tracking-[-0.01em]">{title}</h2>
        <p className="max-w-[760px] text-[13px] leading-normal text-text-2">{lead}</p>
      </div>
      {children}
    </section>
  );
}
