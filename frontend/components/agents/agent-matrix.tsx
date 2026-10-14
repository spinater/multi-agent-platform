import clsx from "clsx";

import { providerColor } from "@/lib/colors";
import type { AgentFile } from "@/lib/generated/AgentFile";
import type { AgentProvider } from "@/lib/generated/AgentProvider";
import type { AgentRow } from "@/lib/generated/AgentRow";

import { tint } from "../skills/shared";
import { AGENT_PROVIDERS, PROVIDER_LABEL } from "./shared";

export function ScopeBadge({ scope }: { scope: AgentRow["scope"] }) {
  const c = scope === "project" ? "var(--accent)" : "var(--info)";
  return (
    <span
      className="inline-flex h-[18px] flex-none items-center rounded px-1.5 font-mono text-[9px] font-medium tracking-[0.1em] whitespace-nowrap uppercase"
      style={{ color: c, background: tint(c, 10), border: `1px solid ${tint(c, 35)}` }}
    >
      {scope}
    </span>
  );
}

function toolsText(f: AgentFile): string {
  if (f.provider === "codex") return f.all_tools ? "all tools" : f.tools.length ? "shell off" : "read only";
  return f.all_tools ? "all tools" : `${f.tools.length} tool${f.tools.length === 1 ? "" : "s"}`;
}

/** ✓ with the model: the CLI has a file for it; ! the file has a problem the CLI may trip on; — no file. */
function Cell({ provider, files }: { provider: AgentProvider; files: AgentFile[] }) {
  const mine = files.filter((f) => f.provider === provider);
  const f = mine[0];
  if (!f)
    return (
      <span className="font-mono text-[12px] text-muted/70" title={`${PROVIDER_LABEL[provider]}: no file`}>
        —
      </span>
    );
  const title = [f.path, f.error ?? "", f.all_tools ? "all tools" : f.tools.join(", "), mine.length > 1 ? `also in ${mine.slice(1).map((m) => m.path).join(", ")}` : ""].filter(Boolean).join("\n");
  if (f.error)
    return (
      <span className="flex min-w-0 flex-col gap-0.5" title={title}>
        <span className="font-mono text-[12px] font-semibold text-bad">! error</span>
        <span className="truncate font-mono text-[10.5px] text-bad/80">{f.error}</span>
      </span>
    );
  return (
    <span className="flex min-w-0 flex-col gap-0.5" title={title}>
      <span className="flex items-center gap-1.5 font-mono text-[12px] text-text">
        <span className="font-semibold" style={{ color: providerColor(provider) }}>
          ✓
        </span>
        <span className="truncate">{f.model ?? "inherit"}</span>
        {mine.length > 1 && <span className="text-warn">+{mine.length - 1}</span>}
      </span>
      <span className="font-mono text-[10.5px] text-muted">
        {toolsText(f)}
        {f.effort ? ` · ${f.effort}` : ""}
      </span>
    </span>
  );
}

const GRID = "grid grid-cols-[minmax(220px,1fr)_repeat(3,150px)] items-center gap-x-4";

/** One row per agent and scope; a column per CLI says whether that CLI has the agent, with which model. */
export function AgentMatrix({ rows, selected, onSelect }: { rows: AgentRow[]; selected: string | null; onSelect: (row: AgentRow) => void }) {
  return (
    <div className="overflow-x-auto rounded-xl border border-line bg-surface">
      <div className="flex min-w-[720px] flex-col">
        <div className={clsx(GRID, "border-b border-line px-4 py-2.5")}>
          <span className="label">Agent</span>
          {AGENT_PROVIDERS.map((p) => (
            <span key={p} className="label flex items-center gap-1.5">
              <span className="size-1.5 rounded-full" style={{ background: providerColor(p) }} />
              {PROVIDER_LABEL[p]}
            </span>
          ))}
        </div>
        {rows.map((r) => {
          const key = `${r.scope}/${r.name}`;
          const on = key === selected;
          return (
            <button
              key={key}
              onClick={() => onSelect(r)}
              aria-pressed={on}
              className={clsx(GRID, "border-b border-line px-4 py-3 text-left text-text last:border-b-0", on ? "bg-accent/[.06]" : "hover:bg-white/[.03]")}
            >
              <span className="flex min-w-0 flex-col gap-1">
                <span className="flex min-w-0 items-center gap-2">
                  <span className={clsx("truncate font-mono text-[13px] font-semibold", on ? "text-accent-soft" : "text-text")}>{r.name}</span>
                  <ScopeBadge scope={r.scope} />
                  {r.notes.length > 0 && (
                    <span className="font-mono text-[10px] text-warn" title={r.notes.map((n) => `${PROVIDER_LABEL[n.provider]}: ${n.text}`).join("\n")}>
                      {r.notes.length} note{r.notes.length === 1 ? "" : "s"}
                    </span>
                  )}
                </span>
                <span className="line-clamp-2 text-[12px] leading-[1.45] text-nav">{r.description || "No description."}</span>
              </span>
              {AGENT_PROVIDERS.map((p) => (
                <Cell key={p} provider={p} files={r.files} />
              ))}
            </button>
          );
        })}
      </div>
    </div>
  );
}
