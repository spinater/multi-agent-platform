import clsx from "clsx";

import { providerColor } from "@/lib/colors";
import type { AgentFile } from "@/lib/generated/AgentFile";
import type { AgentProvider } from "@/lib/generated/AgentProvider";
import type { AgentReader } from "@/lib/generated/AgentReader";
import type { AgentRow } from "@/lib/generated/AgentRow";
import { providerName } from "@/lib/use-usage";

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

const GRID = "grid items-center gap-x-4";

/** A CLI without a format of its own here (Cursor, Copilot): it loads the agent when the CLI it reads has a usable file. */
function ReaderCell({ reader, row }: { reader: AgentReader; row: AgentRow }) {
  const f = row.files.find((x) => x.provider === reader.reads && !x.error);
  const name = providerName(reader.provider, true);
  if (!reader.scopes.includes(row.scope))
    return (
      <span className="font-mono text-[11px] text-muted/70" title={`${name} does not read ${PROVIDER_LABEL[reader.reads]}'s ${row.scope} agents`}>
        n/a
      </span>
    );
  if (!f)
    return (
      <span className="font-mono text-[12px] text-muted/70" title={`${name}: loads ${PROVIDER_LABEL[reader.reads]} files only, and there is none`}>
        —
      </span>
    );
  return (
    <span className="flex min-w-0 flex-col gap-0.5" title={`${name} loads ${f.path} itself (it has no agent format MAP writes)`}>
      <span className="flex items-center gap-1.5 font-mono text-[12px] text-text-2">
        <span style={{ color: providerColor(reader.provider) }}>↳</span>
        <span className="truncate">via {PROVIDER_LABEL[reader.reads]}</span>
      </span>
      <span className="font-mono text-[10.5px] text-muted">same file</span>
    </span>
  );
}

/** One row per agent and scope; a column per CLI says whether that CLI has the agent, with which model. CLIs that load
 * another CLI's files (`readers`) get a narrower column of their own. */
export function AgentMatrix({ rows, readers = [], selected, onSelect }: { rows: AgentRow[]; readers?: AgentReader[]; selected: string | null; onSelect: (row: AgentRow) => void }) {
  const grid = { gridTemplateColumns: `minmax(220px,1fr) repeat(${AGENT_PROVIDERS.length}, 150px)${readers.length ? ` repeat(${readers.length}, 120px)` : ""}` };
  return (
    <div className="overflow-x-auto rounded-xl border border-line bg-surface">
      <div className="flex flex-col" style={{ minWidth: 720 + readers.length * 136 }}>
        <div className={clsx(GRID, "border-b border-line px-4 py-2.5")} style={grid}>
          <span className="label">Agent</span>
          {AGENT_PROVIDERS.map((p) => (
            <span key={p} className="label flex items-center gap-1.5">
              <span className="size-1.5 rounded-full" style={{ background: providerColor(p) }} />
              {PROVIDER_LABEL[p]}
            </span>
          ))}
          {readers.map((r) => (
            <span key={r.provider} className="label flex items-center gap-1.5" title={`${providerName(r.provider)} has no agent format of its own here: it loads ${PROVIDER_LABEL[r.reads]}'s files`}>
              <span className="size-1.5 rounded-full" style={{ background: providerColor(r.provider) }} />
              {providerName(r.provider, true)}
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
              style={grid}
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
              {readers.map((x) => (
                <ReaderCell key={x.provider} reader={x} row={r} />
              ))}
            </button>
          );
        })}
      </div>
    </div>
  );
}
