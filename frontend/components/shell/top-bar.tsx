"use client";

import clsx from "clsx";
import { Hexagon, Moon, Settings, Sun } from "lucide-react";
import { useState, type ReactNode } from "react";

import { setLang, useLang, useT, type MsgKey } from "@/lib/i18n";
import { NAV } from "@/lib/nav";

import { useApp } from "../app-context";
import { UpdatesBell } from "../updates-bell";
import { UsageMini } from "../usage-mini";

/** The default workspace's display name: the last segment of `workspace.dir`. */
export function workspaceName(dir: string | undefined): string {
  if (!dir) return "workspace";
  return dir.replace(/\/+$/, "").split("/").pop() || dir;
}

export function Clock() {
  const { now } = useApp();
  const d = new Date(now);
  const two = (n: number) => String(n).padStart(2, "0");
  return (
    <span className="font-mono text-[10px] font-medium tracking-[0.12em] whitespace-nowrap text-muted max-sm:hidden">
      {two(d.getDate())}.{two(d.getMonth() + 1)} {two(d.getHours())}:{two(d.getMinutes())}:{two(d.getSeconds())}
    </span>
  );
}

function ProjectSwitcher() {
  const { projects, project, setProject, setScope, scope, server, go, route } = useApp();
  const t = useT();
  const [open, setOpen] = useState(false);
  const current = scope === "all" ? t("sw.all") : project === null ? workspaceName(server?.workspace_dir) : (projects.find((p) => p.project.slug === project)?.project.name ?? project);
  const pick = (slug: string | null) => {
    setProject(slug);
    setScope("project");
    setOpen(false);
  };
  const row = (key: string, label: string, sub: string, active: boolean, onClick: () => void, dot = "var(--accent)") => (
    <button key={key} onClick={onClick} className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left hover:bg-white/5">
      <span className="size-1.5 shrink-0 rounded-full" style={{ background: dot }} />
      <span className="min-w-0 flex-1">
        <span className="block truncate font-mono text-[11.5px] font-medium text-text">{label}</span>
        <span className="block truncate font-mono text-[10px] text-muted">{sub}</span>
      </span>
      {active && <span className="text-accent-soft">✓</span>}
    </button>
  );
  return (
    <div className="relative">
      <button
        onClick={() => setOpen(!open)}
        className="flex h-7 items-center gap-2 rounded-[7px] border border-line-2 px-2.5 font-mono text-[11.5px] font-medium whitespace-nowrap text-text hover:border-line-3"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span className="size-1.5 rounded-full bg-accent" />
        {current}
        <span className="text-muted">▾</span>
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div role="menu" className="absolute top-9 left-0 z-50 w-[290px] max-w-[calc(100vw-32px)] rounded-[10px] border border-line-2 bg-menu p-1.5 shadow-[0_20px_50px_rgba(0,0,0,.55)]">
            <p className="label px-2.5 pt-1.5 pb-1">{t("sw.title")}</p>
            {row("", workspaceName(server?.workspace_dir), `${server?.workspace_dir ?? ""} · ${t("sw.default")}`, scope === "project" && project === null, () => pick(null), "var(--muted)")}
            {projects.map((p) => row(p.project.slug, p.project.name, `${p.workspace} · reflex ${p.reflex.state.replace("_", " ")}`, scope === "project" && project === p.project.slug, () => pick(p.project.slug)))}
            <div className="my-1 border-t border-line" />
            {row("*", t("sw.all"), t("sw.allSub"), scope === "all", () => {
              setScope("all");
              setOpen(false);
              if (route.screen !== "command") go("command");
            }, "var(--text-2)")}
            <button
              onClick={() => {
                setOpen(false);
                go("projects", "new");
              }}
              className="flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left font-mono text-[11.5px] text-link hover:bg-white/5"
            >
              {t("sw.new")}
            </button>
          </div>
        </>
      )}
    </div>
  );
}

/** EN ⇄ TH: one button that shows the current interface language and switches it (design: LangSwitch.dc.html). */
export function LangSwitch() {
  const lang = useLang();
  const t = useT();
  return (
    <button
      onClick={() => setLang(lang === "en" ? "th" : "en")}
      title={`${t("lang.title")} · ${t("lang.toggle")}`}
      aria-label={t("lang.toggle")}
      className="flex h-7 min-w-[38px] cursor-pointer items-center justify-center rounded-[7px] border border-line-2 px-2 font-mono text-[10.5px] font-semibold text-accent-soft hover:border-line-3"
    >
      {lang.toUpperCase()}
    </button>
  );
}

function Nav() {
  const { route, go } = useApp();
  const t = useT();
  return (
    <nav className="flex max-w-full min-w-0 gap-0.5 overflow-x-auto">
      {NAV.map((s) => (
        <a
          key={s}
          href={`#${s}`}
          onClick={(e) => {
            e.preventDefault();
            go(s);
          }}
          className={clsx(
            "flex h-7 items-center rounded-md px-2.5 text-xs font-medium whitespace-nowrap no-underline",
            route.screen === s || (s === "configure" && (route.screen === "skills" || route.screen === "agents")) ? "bg-accent-tint text-accent-soft hover:text-accent-soft" : "text-nav hover:bg-white/5 hover:text-text",
          )}
        >
          {t(`nav.${s}` as MsgKey)}
        </a>
      ))}
    </nav>
  );
}

/** Sticky bar on every screen: logo, project switcher, nav, then `extra` (e.g. the command centre's status items),
 * the CLI updates bell, the usage pill, the clock, Settings and the theme toggle. */
export function TopBar({ extra }: { extra?: ReactNode }) {
  const { go, route, theme, toggleTheme } = useApp();
  const t = useT();
  const onSettings = route.screen === "settings";
  return (
    <header className="sticky top-0 z-30 flex min-h-11 flex-wrap items-center justify-between gap-x-5 gap-y-2 border-b border-line bg-[color-mix(in_srgb,var(--bg)_97%,transparent)] px-5 py-2">
      <div className="flex max-w-full min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
        <a
          href="#projects"
          onClick={(e) => {
            e.preventDefault();
            go("projects");
          }}
          className="flex items-center gap-1.5 text-sm font-bold tracking-[0.08em] whitespace-nowrap text-text no-underline hover:text-text"
        >
          <Hexagon className="size-3.5 text-accent" strokeWidth={2.5} />MAP
        </a>
        <ProjectSwitcher />
        <Nav />
      </div>
      <div className="flex items-center gap-3">
        <UpdatesBell />
        <LangSwitch />
        <UsageMini />
        <Clock />
        <a
          href="#settings"
          onClick={(e) => {
            e.preventDefault();
            go("settings");
          }}
          title={t("top.settings")}
          aria-label="Settings"
          aria-current={onSettings ? "page" : undefined}
          className={clsx(
            "flex size-7 items-center justify-center rounded-md no-underline",
            onSettings ? "bg-accent-tint text-accent-soft hover:text-accent-soft" : "text-muted hover:bg-white/5 hover:text-text",
          )}
        >
          <Settings className="size-3.5" />
        </a>
        <button onClick={toggleTheme} className="flex size-7 items-center justify-center rounded-md text-muted hover:bg-white/5 hover:text-text" aria-label={t("top.theme")} title={t("top.theme")}>
          {theme === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />}
        </button>
      </div>
      {/* A screen's status strip gets its own row, so the nav and the right-hand controls stay on one line. */}
      {extra && <div className="w-full min-w-0 overflow-x-auto">{extra}</div>}
    </header>
  );
}
