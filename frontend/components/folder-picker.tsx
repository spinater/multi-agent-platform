"use client";

/** The wizard's folder picker: browses only the project folder (`[projects] path`, or each of `[projects] roots`)
 * through `/api/fs/*` (backend roots.rs), lists sub-folders only, and hands the chosen absolute path to the wizard,
 * whose inspect call checks it again on the server. An advanced field takes a typed path instead. */

import clsx from "clsx";
import { ChevronRight, Folder, FolderGit2, FolderPlus, Link2 } from "lucide-react";
import { useEffect, useState } from "react";

import { api } from "@/lib/api";
import type { FsListing } from "@/lib/generated/FsListing";
import type { FsRoot } from "@/lib/generated/FsRoot";

import { Btn, TextInput, errText } from "./project-ui";

/** A new folder's name: one path segment, no slashes, not `.` or `..`. Returns the problem, or "" when fine. */
export function folderNameProblem(name: string, taken: string[]): string {
  const n = name.trim();
  if (!n) return "give the new folder a name";
  if (n.includes("/") || n.includes("\\")) return "a name, not a path: no slashes";
  if (n === "." || n === "..") return "not . or ..";
  if (n.length > 120) return "at most 120 characters";
  if (taken.includes(n)) return `${n} already exists here`;
  return "";
}

/** `path` as the host sees it, when the root is mounted elsewhere in a container. */
export function hostOf(root: FsRoot | undefined, path: string): string | null {
  if (!root?.host_path || !path.startsWith(root.path)) return null;
  return root.host_path + path.slice(root.path.length);
}

const join = (dir: string, name: string) => `${dir.replace(/\/+$/, "")}/${name.trim()}`;

export function FolderPicker({
  mode,
  value,
  onChange,
  suggestedName = "",
}: {
  /** `existing`: pick a folder that is there. `new`: name a folder to create (or pick an empty one). */
  mode: "existing" | "new";
  value: string;
  onChange: (path: string) => void;
  suggestedName?: string;
}) {
  const [roots, setRoots] = useState<FsRoot[] | null>(null);
  const [rootName, setRootName] = useState("");
  const [rel, setRel] = useState("");
  const [hidden, setHidden] = useState(false);
  const [listing, setListing] = useState<FsListing | null>(null);
  const [answered, setAnswered] = useState("");
  const [error, setError] = useState("");
  const [newName, setNewName] = useState<string | null>(null);
  const [typing, setTyping] = useState(false);

  useEffect(() => {
    api
      .fsRoots()
      .then((r) => {
        setRoots(r);
        setRootName((r.find((x) => x.is_default && x.exists) ?? r.find((x) => x.exists) ?? r[0])?.name ?? "");
      })
      .catch((e) => setError(errText(e)));
  }, []);

  const root = roots?.find((r) => r.name === rootName);
  // Loading while the last answer (listing or error) is for another folder than the one asked for.
  const want = `${rootName}\n${rel}\n${hidden}`;
  const loading = !!root?.exists && answered !== want;
  useEffect(() => {
    if (!root?.exists) return;
    let stale = false;
    const key = `${root.name}\n${rel}\n${hidden}`;
    api
      .fsBrowse(root.name, rel, hidden)
      .then((l) => {
        if (stale) return;
        setListing(l);
        setError("");
      })
      .catch((e) => !stale && setError(errText(e)))
      .finally(() => !stale && setAnswered(key));
    return () => {
      stale = true;
    };
  }, [root, rel, hidden]);

  const shown = listing && listing.root === rootName ? listing : null;
  const name = newName ?? suggestedName;
  const problem = shown ? folderNameProblem(name, shown.entries.map((e) => e.name)) : "";
  const blockedHere = shown ? (shown.in_project ? `used by project ${shown.in_project}` : shown.is_root && !root?.allow_root_itself ? "the project folder itself: pick or create a sub-folder" : "") : "";
  const emptyHere = !!shown && shown.entries.length === 0 && shown.files === 0 && shown.hidden_dirs === 0;
  const crumbs = shown?.path ? shown.path.split("/") : [];
  const host = hostOf(root, value);

  return (
    <div className="flex min-w-0 flex-col gap-2.5">
      {roots && roots.length > 1 && (
        <div className="flex flex-wrap gap-1.5" role="tablist" aria-label="Project folders">
          {roots.map((r) => (
            <button
              key={r.name}
              type="button"
              role="tab"
              aria-selected={r.name === rootName}
              onClick={() => {
                setRootName(r.name);
                setRel("");
              }}
              className={clsx(
                "h-7 rounded-full border px-3 text-[12px] font-semibold",
                r.name === rootName ? "border-accent bg-accent/15 text-accent-soft" : "border-line-2 text-nav hover:text-text",
                !r.exists && "opacity-60",
              )}
            >
              {r.name}
            </button>
          ))}
        </div>
      )}
      {root && (
        <div className="flex flex-col gap-0.5 font-mono text-[11px] text-muted">
          <span className="break-all">
            <span className="tracking-[0.08em] uppercase">Project folder</span> <span className="text-text-2">{root.path}</span>
          </span>
          {root.host_path && <span className="break-all">on the host {root.host_path}</span>}
        </div>
      )}

      <div className="flex min-w-0 flex-col overflow-hidden rounded-lg border border-line bg-inset">
        <div className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-0.5 border-b border-line px-3 py-2 font-mono text-[11.5px]">
          <button type="button" onClick={() => setRel("")} className="text-link hover:text-link-hover">
            {root?.name ?? "…"}
          </button>
          {crumbs.map((c, i) => (
            <span key={i} className="flex min-w-0 items-center gap-1">
              <span className="text-muted">/</span>
              <button type="button" onClick={() => setRel(crumbs.slice(0, i + 1).join("/"))} className={clsx("break-all", i === crumbs.length - 1 ? "text-text" : "text-link hover:text-link-hover")}>
                {c}
              </button>
            </span>
          ))}
          <span className="ml-auto flex items-center gap-2 text-muted">
            {loading && <span>…</span>}
            <label className="flex cursor-pointer items-center gap-1">
              <input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} className="accent-[var(--accent)]" />
              dot-folders
            </label>
          </span>
        </div>
        <ul className="max-h-[248px] min-h-[96px] overflow-y-auto py-1" aria-label="Sub-folders">
          {root && !root.exists && <li className="px-3 py-2 font-mono text-[11.5px] text-warn">! {root.path} does not exist on this machine yet: create it, or type a path below.</li>}
          {shown?.parent !== null && shown?.parent !== undefined && (
            <li>
              <button type="button" onClick={() => setRel(shown.parent ?? "")} className="flex w-full items-center gap-2 px-3 py-1.5 text-left font-mono text-[12px] text-muted hover:bg-white/[.03]">
                ..
              </button>
            </li>
          )}
          {shown?.entries.map((e) => {
            const used = e.is_project ?? e.contains_project;
            const path = join(shown.abs_path, e.name);
            return (
              <li key={e.name} className={clsx("flex min-w-0 items-center gap-2 pr-2 hover:bg-white/[.03]", value === path && "bg-accent/10")}>
                <button
                  type="button"
                  onClick={() => setRel(shown.path ? `${shown.path}/${e.name}` : e.name)}
                  className="flex min-w-0 flex-1 items-center gap-2 py-1.5 pl-3 text-left"
                  title={`Open ${e.name}`}
                >
                  {e.is_git ? <FolderGit2 className="size-3.5 shrink-0 text-accent-soft" /> : <Folder className="size-3.5 shrink-0 text-muted" />}
                  <span className={clsx("min-w-0 truncate font-mono text-[12.5px]", e.hidden ? "text-muted" : "text-text")}>{e.name}</span>
                  {e.kind === "symlink" && <Link2 className="size-3 shrink-0 text-muted" aria-label="symlink" />}
                  {e.is_git && <Tag tone="accent">git</Tag>}
                  {used && <Tag tone="warn">{e.is_project ? `project ${e.is_project}` : `has ${e.contains_project}`}</Tag>}
                  {e.has_children && <ChevronRight className="ml-auto size-3.5 shrink-0 text-muted" />}
                </button>
                {mode === "existing" && (
                  <Btn size="sm" tone="quiet" disabled={!!used} onClick={() => onChange(path)} title={used ? `Used by project ${used}` : `Select ${path}`}>
                    Select
                  </Btn>
                )}
              </li>
            );
          })}
          {shown && shown.entries.length === 0 && <li className="px-3 py-2 font-mono text-[11.5px] text-muted">no sub-folders{shown.hidden_dirs > 0 ? ` · ${shown.hidden_dirs} dot-folder${shown.hidden_dirs === 1 ? "" : "s"} hidden` : ""}</li>}
          {shown?.truncated && <li className="px-3 py-1.5 font-mono text-[11px] text-warn">only the first {shown.entries.length} folders are shown: open a sub-folder or type a path</li>}
        </ul>
        {shown && (
          <div className="flex flex-wrap items-center justify-between gap-2 border-t border-line px-3 py-2 font-mono text-[11px] text-muted">
            <span>
              {shown.entries.length} folder{shown.entries.length === 1 ? "" : "s"} · {shown.files} file{shown.files === 1 ? "" : "s"}
            </span>
            {(mode === "existing" || emptyHere) && (
              <Btn size="sm" tone={mode === "existing" ? "primary" : "ghost"} disabled={!!blockedHere} title={blockedHere || `Select ${shown.abs_path}`} onClick={() => onChange(shown.abs_path)}>
                {mode === "existing" ? "Select this folder" : "Use this empty folder"}
              </Btn>
            )}
          </div>
        )}
      </div>
      {error && <span className="font-mono text-[11.5px] break-words text-bad">✗ {error}</span>}
      {shown && blockedHere && mode === "existing" && <span className="font-mono text-[11px] text-muted">This folder: {blockedHere}.</span>}

      {mode === "new" && shown && (
        <div className="flex flex-col gap-1.5">
          <span className="flex items-center gap-1.5 font-mono text-[10.5px] tracking-[0.1em] text-muted uppercase">
            <FolderPlus className="size-3" /> New folder here
          </span>
          <div className="flex min-w-0 gap-2">
            <TextInput value={name} onChange={(e) => setNewName(e.target.value)} placeholder="my-app" aria-label="New folder name" />
            <Btn tone="primary" disabled={!!problem || !!shown.in_project} onClick={() => onChange(join(shown.abs_path, name))}>
              Use
            </Btn>
          </div>
          {(shown.in_project || problem || value !== join(shown.abs_path, name)) && (
            <span className={clsx("font-mono text-[11px] break-all", shown.in_project || problem ? "text-warn" : "text-muted")}>
              {shown.in_project ? `inside project ${shown.in_project}: pick another folder` : problem || `${join(shown.abs_path, name)}${shown.host_path ? ` · on the host ${join(shown.host_path, name)}` : ""}`}
            </span>
          )}
        </div>
      )}

      <div className="flex flex-col gap-1.5">
        {value && (
          <span className="font-mono text-[11.5px] break-all text-text-2">
            <span className="text-muted">selected </span>
            {value}
            {host && <span className="text-muted"> · on the host {host}</span>}
          </span>
        )}
        <button type="button" onClick={() => setTyping(!typing)} className="self-start font-mono text-[11px] text-link hover:text-link-hover">
          {typing ? "Hide the path field" : "Type a path instead"}
        </button>
        {typing && (
          <TextInput value={value} onChange={(e) => onChange(e.target.value)} placeholder={root ? join(root.path, "my-app") : "~/project/my-app"} aria-label="Folder path" />
        )}
      </div>
    </div>
  );
}

function Tag({ tone, children }: { tone: "accent" | "warn"; children: string }) {
  return (
    <span className={clsx("shrink-0 rounded border px-1 py-px font-mono text-[10px]", tone === "accent" ? "border-accent/40 text-accent-soft" : "border-warn/40 text-warn")}>{children}</span>
  );
}
