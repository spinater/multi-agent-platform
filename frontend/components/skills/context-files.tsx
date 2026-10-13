import clsx from "clsx";

import { providerColor } from "@/lib/colors";
import type { ContextFile } from "@/lib/generated/ContextFile";

function note(c: ContextFile, all: ContextFile[]): string {
  if (!c.exists) return `Not in this workspace: ${c.provider} starts without project instructions.`;
  if (c.summary) return c.summary;
  if (c.imports.length) return `Imports ${c.imports.join(", ")}, so ${c.provider} reads ${c.imports.length === 1 ? "that file" : "those files"} too.`;
  const by = all.filter((o) => c.imported_by.includes(o.file)).map((o) => o.provider);
  return by.length ? `Only headings; imported by ${by.join(", ")}.` : "Only headings, no prose.";
}

function meta(c: ContextFile, all: ContextFile[]): string {
  if (!c.exists) return "missing";
  const parts = [`${c.lines} line${c.lines === 1 ? "" : "s"}`];
  if (c.imports.length) parts.push(`imports ${c.imports.join(", ")}`);
  const by = all.filter((o) => c.imported_by.includes(o.file)).map((o) => o.provider);
  parts.push(by.length ? `read by ${c.provider}, imported by ${by.join(", ")}` : `read by ${c.provider}`);
  return parts.join(" · ");
}

/** One card per provider's context file (CLAUDE.md, AGENTS.md, GEMINI.md…). */
export function ContextFiles({ contexts }: { contexts: ContextFile[] }) {
  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(240px,1fr))] gap-2.5">
      {contexts.map((c) => (
        <div key={c.file} title={c.path} className={clsx("flex min-w-0 flex-col gap-2 rounded-xl border border-line bg-surface px-4 py-3.5", !c.exists && "opacity-60")}>
          <div className="flex items-center justify-between gap-2">
            <span className="truncate font-mono text-[13px] font-semibold">{c.file}</span>
            <span className="flex items-center gap-1.5 font-mono text-[10.5px] font-medium whitespace-nowrap text-nav">
              <span className="size-1.5 rounded-full" style={{ background: providerColor(c.provider) }} />
              {c.provider}
            </span>
          </div>
          <span className="text-xs leading-[1.45] text-pretty text-text-2">{note(c, contexts)}</span>
          <span className="font-mono text-[10.5px] text-muted">{meta(c, contexts)}</span>
        </div>
      ))}
    </div>
  );
}
