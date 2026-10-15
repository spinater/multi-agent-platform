"use client";

/** Configure (`#configure`, `#configure/<tab>`): one place for what agents run with (design: Configure.dc.html).
 * Scope switch Project | Machine; every item carries a source badge (machine, project, override). Left tabs in
 * groups; each tab has its header (title, description, "saves to", add action). Edits stay local until the save
 * bar's Save, which saves every tab with unsaved changes. Tabs keep their state while you switch between them. */

import clsx from "clsx";
import { useCallback, useMemo, useState, type ReactNode } from "react";

import { useT, type MsgKey } from "@/lib/i18n";

import { useApp } from "../app-context";
import { AgentsTab } from "../configure/agents";
import { BranchesTab, GitTab, ParallelTab } from "../configure/git";
import { EnvTab } from "../configure/env";
import { HooksTab } from "../configure/hooks";
import { McpTab } from "../configure/mcp";
import { PermsTab } from "../configure/perms";
import { ProvidersTab } from "../configure/providers";
import { RulesTab } from "../configure/rules";
import { SkillsTab } from "../configure/skills";
import { Ctx, SourceBadge, btnCls, primaryCls, type ConfigureCtx, type Pending, type Scope } from "../configure/ui";
import { VerifyTab } from "../configure/verify";
import { WorkflowsTab } from "../configure/workflows";
import { Main } from "../shell/page";
import { TopBar, workspaceName } from "../shell/top-bar";

export const TABS = [
  { group: "Agents", tabs: [["agents", "Agents", "○", "var(--p-codex)"], ["workflows", "Workflows", "⬡", "var(--accent)"]] },
  { group: "Context", tabs: [["rules", "Rules", "▤", "var(--text-2)"], ["skills", "Skills", "✦", "var(--p-gemini)"]] },
  { group: "Safety", tabs: [["perms", "Permissions", "⊘", "var(--bad)"], ["hooks", "Hooks", "⚑", "var(--warn)"], ["verify", "Verify & audit", "✓", "var(--ok)"]] },
  { group: "Git", tabs: [["git", "Git & SSH", "⚷", "var(--text-2)"], ["branches", "Repo & branches", "⑂", "var(--ok)"], ["parallel", "Parallel", "⫴", "var(--p-codex)"]] },
  { group: "Connections", tabs: [["mcp", "MCP servers", "⇄", "var(--info)"], ["env", "Env & secrets", "⚿", "var(--text-2)"], ["providers", "Providers", "◉", "var(--p-claude)"]] },
] as const;
export type TabId = (typeof TABS)[number]["tabs"][number][0];
const TAB_IDS = TABS.flatMap((g) => g.tabs.map((t) => t[0])) as TabId[];
const isTab = (s: string | null | undefined): s is TabId => !!s && (TAB_IDS as string[]).includes(s);

const BODY: Record<TabId, () => ReactNode> = {
  agents: () => <AgentsTab />,
  workflows: () => <WorkflowsTab />,
  rules: () => <RulesTab />,
  skills: () => <SkillsTab />,
  perms: () => <PermsTab />,
  hooks: () => <HooksTab />,
  verify: () => <VerifyTab />,
  git: () => <GitTab />,
  branches: () => <BranchesTab />,
  parallel: () => <ParallelTab />,
  mcp: () => <McpTab />,
  env: () => <EnvTab />,
  providers: () => <ProvidersTab />,
};

function storedTab(): TabId {
  try {
    const t = localStorage.getItem("map.configure.tab");
    return isTab(t) ? t : "agents";
  } catch {
    return "agents";
  }
}

function storedLang(): "en" | "th" {
  try {
    return localStorage.getItem("map.cfgLang") === "th" ? "th" : "en";
  } catch {
    return "en";
  }
}

export function ConfigureScreen() {
  const { route, go, project, current, server, scope: appScope, toast } = useApp();
  const tab: TabId = isTab(route.arg) ? route.arg : storedTab();
  const [scope, setScope] = useState<Scope>(project === null || appScope === "all" ? "machine" : "project");
  const [cfgLang, setCfgLang] = useState<"en" | "th">(storedLang);
  const [visited, setVisited] = useState<TabId[]>([tab]);
  const [pending, setPendingState] = useState<Record<string, Pending>>({});
  const [saving, setSaving] = useState(false);
  const t = useT();

  if (!visited.includes(tab)) setVisited([...visited, tab]);

  const slug = scope === "project" ? project : null;
  const projectName = current?.project.name ?? (project === null ? workspaceName(server?.workspace_dir) : project);

  const setPending = useCallback((t: string, p: Pending | null) => {
    setPendingState((prev) => {
      if (!p || p.count === 0) {
        if (!(t in prev)) return prev;
        const next = { ...prev };
        delete next[t];
        return next;
      }
      return { ...prev, [t]: p };
    });
  }, []);

  const ctx: ConfigureCtx = useMemo(
    () => ({ scope, slug, setScope, setPending, flash: (m, tone = "ok") => toast(m, tone), cfgLang }),
    [scope, slug, setPending, toast, cfgLang],
  );

  const setTab = (t: TabId) => {
    try {
      localStorage.setItem("map.configure.tab", t);
    } catch {
      // Storage blocked: the tab lasts for this page.
    }
    go("configure", t);
  };
  const setLang = (l: "en" | "th") => {
    setCfgLang(l);
    try {
      localStorage.setItem("map.cfgLang", l);
    } catch {
      // Storage blocked: the choice lasts for this page.
    }
  };

  const all = Object.values(pending);
  const count = all.reduce((n, p) => n + p.count, 0);
  const files = [...new Set(all.flatMap((p) => p.files))];
  const saveAll = async () => {
    setSaving(true);
    try {
      for (const p of Object.values(pending)) await p.save();
      toast(t("cfg.saved"));
    } catch (e) {
      toast((e as Error).message, "bad");
    } finally {
      setSaving(false);
    }
  };
  const discardAll = () => {
    for (const p of Object.values(pending)) p.discard();
    toast(t("cfg.discarded"), "warn");
  };
  // Switching scope with unsaved edits would mix them up: ask first.
  const changeScope = (s: Scope) => {
    if (s === scope) return;
    if (count && !window.confirm(t("cfg.switchConfirm", { n: count }))) return;
    discardAll();
    setScope(s);
  };

  return (
    <Ctx.Provider value={{ ...ctx, setScope: changeScope }}>
      <TopBar />
      <Main className="gap-[22px] pb-[90px]">
        <div className="flex flex-wrap items-end justify-between gap-[18px]">
          <div className="flex max-w-[720px] flex-col gap-2.5">
            <span className="label">{scope === "project" ? t("cfg.kProject", { p: projectName }) : t("cfg.kMachine")}</span>
            <h1 className="text-[30px] leading-[1.05] font-semibold tracking-[-0.02em] sm:text-[38px]">{t("cfg.title")}</h1>
            <p className="text-sm leading-[1.55] text-text-2">
              {t("cfg.intro")}
            </p>
          </div>
          <div className="flex flex-col items-end gap-2">
            <div className="flex gap-0.5 rounded-[9px] border border-line-2 p-[3px]">
              {(
                [
                  ["project", t("cfg.project"), projectName],
                  ["machine", t("cfg.machine"), t("cfg.allProjects")],
                ] as const
              ).map(([s, l, sub]) => (
                <button
                  key={s}
                  onClick={() => changeScope(s)}
                  disabled={s === "project" && project === null}
                  title={s === "project" && project === null ? t("cfg.pickFirst") : undefined}
                  className="flex cursor-pointer flex-col items-start gap-1 rounded-[7px] px-3 py-[7px] text-left disabled:cursor-not-allowed disabled:opacity-50"
                  style={{ background: scope === s ? "var(--accent-tint)" : "transparent", color: scope === s ? "var(--accent-soft)" : "var(--nav)" }}
                >
                  <span className="text-[12.5px] leading-none font-semibold whitespace-nowrap">{l}</span>
                  <span className="font-mono text-[10px] leading-none whitespace-nowrap text-muted">{sub}</span>
                </button>
              ))}
            </div>
            <div className="flex items-center gap-1.5 font-mono text-[10.5px] text-muted">
              <span>{t("cfg.cfgLang")}</span>
              {(
                [
                  ["en", t("cfg.english")],
                  ["th", "ไทย"],
                ] as const
              ).map(([l, label]) => {
                const on = cfgLang === l;
                const c = l === "th" ? "var(--warn)" : "var(--accent)";
                return (
                  <button
                    key={l}
                    onClick={() => setLang(l)}
                    className="h-[22px] cursor-pointer rounded-[5px] border px-2 font-mono text-[10.5px] font-medium whitespace-nowrap"
                    style={{
                      borderColor: on ? `color-mix(in srgb, ${c} 50%, transparent)` : "var(--line-2)",
                      background: on ? `color-mix(in srgb, ${c} 10%, transparent)` : "transparent",
                      color: on ? (l === "th" ? "var(--warn)" : "var(--accent-soft)") : "var(--text-2)",
                    }}
                  >
                    {label}
                  </button>
                );
              })}
            </div>
          </div>
        </div>

        {cfgLang === "th" && (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2.5 rounded-[10px] border px-3.5 py-3" style={{ borderColor: "color-mix(in srgb, var(--warn) 40%, transparent)", background: "color-mix(in srgb, var(--warn) 6%, transparent)" }}>
            <span className="flex-[1_1_420px] text-[13px] leading-normal text-text">
              {t("cfg.thaiWarn")}
            </span>
            <button onClick={() => setLang("en")} className="h-[30px] cursor-pointer rounded-[7px] border px-3 text-[12px] font-semibold whitespace-nowrap text-warn" style={{ borderColor: "color-mix(in srgb, var(--warn) 50%, transparent)" }}>
              {t("cfg.backToEn")}
            </button>
          </div>
        )}

        <div className="flex flex-wrap items-start gap-5">
          <aside className="flex max-w-full flex-[1_1_200px] flex-col gap-3.5 lg:sticky lg:top-16 lg:max-w-[240px]">
            {TABS.map((g) => (
              <div key={g.group} className="flex flex-col gap-0.5">
                <span className="px-2.5 pb-1.5 font-mono text-[9.5px] leading-none font-medium tracking-[0.16em] text-muted/80 uppercase">{t(`cfg.group.${g.group}` as MsgKey)}</span>
                {g.tabs.map(([id, label, glyph, color]) => {
                  const on = tab === id;
                  return (
                    <button
                      key={id}
                      onClick={() => setTab(id)}
                      className={clsx("flex h-[34px] cursor-pointer items-center gap-2 rounded-[7px] border-l-2 px-2.5 text-left text-[13px] font-medium", !on && "hover:bg-white/[.04]")}
                      style={{ borderLeftColor: on ? "var(--accent)" : "transparent", background: on ? "color-mix(in srgb, var(--accent) 8%, transparent)" : undefined, color: on ? "var(--text)" : "var(--text-2)" }}
                    >
                      <span className="w-4 font-mono text-[12px] font-semibold" style={{ color }}>
                        {glyph}
                      </span>
                      <span className="flex-1 whitespace-nowrap">{label}</span>
                      {pending[id] && <span title="unsaved changes" className="size-1.5 rounded-full bg-accent" />}
                    </button>
                  );
                })}
              </div>
            ))}
            <div className="flex flex-col gap-1.5 rounded-[9px] border border-line p-2.5 font-mono text-[10.5px] leading-[1.3] text-muted">
              <span className="flex items-center gap-1.5">
                <SourceBadge source="machine" />
                {t("cfg.lgMachine")}
              </span>
              <span className="flex items-center gap-1.5">
                <SourceBadge source="project" />
                {t("cfg.lgProject")}
              </span>
              <span className="flex items-center gap-1.5">
                <SourceBadge source="override" />
                {t("cfg.lgOverride")}
              </span>
            </div>
          </aside>

          <section className="flex min-w-0 flex-[999_1_640px] flex-col gap-4">
            {visited.map((t) => (
              <div key={`${t}:${scope}:${slug ?? ""}`} className={clsx("flex min-w-0 flex-col gap-4", t !== tab && "hidden")}>
                {BODY[t]()}
              </div>
            ))}
          </section>
        </div>
      </Main>

      {count > 0 && (
        <div className="fixed bottom-5 left-1/2 z-50 flex max-w-[calc(100vw-24px)] -translate-x-1/2 flex-wrap items-center gap-x-4 gap-y-2.5 rounded-xl border border-accent/40 bg-menu py-2.5 pr-3 pl-4 shadow-[0_14px_40px_rgba(0,0,0,.55)]">
          <span className="text-[12.5px] leading-[1.3] font-medium">
            {t("cfg.unsaved", { n: count })}
          </span>
          <span className="font-mono text-[10.5px] leading-[1.3] text-muted">{files.join(" · ")}</span>
          <div className="flex gap-1.5">
            <button onClick={discardAll} className={btnCls}>
              {t("cfg.discard")}
            </button>
            <button onClick={saveAll} disabled={saving} className={clsx(primaryCls, "h-[30px]")}>
              {saving ? t("cfg.saving") : t("cfg.save")}
            </button>
          </div>
        </div>
      )}
    </Ctx.Provider>
  );
}
