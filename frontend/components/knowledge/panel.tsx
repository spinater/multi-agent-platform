"use client";

/** Knowledge › graph's side panel: a node (what it cites, who cites it), a domain (its members), or the domain list. */

import type { KnowledgeGraph } from "@/lib/generated/KnowledgeGraph";
import type { KnowledgeGraphNode } from "@/lib/generated/KnowledgeGraphNode";

import { KIND_COLOR, type Sel } from "./graph";
import { KIND_LABEL } from "./shared";

export function Panel({
  graph,
  sel,
  setSel,
  domColor,
  onOpenDoc,
}: {
  graph: KnowledgeGraph | null;
  sel: Sel;
  setSel: (s: Sel) => void;
  domColor: Map<string, string>;
  onOpenDoc: (path: string) => void;
}) {
  const lbl = "font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase";
  if (!graph) return <div className="flex max-w-[380px] min-w-0 flex-[1_1_280px] flex-col rounded-[10px] border border-line p-3.5" />;
  const node = sel?.kind === "node" ? graph.nodes.find((n) => n.id === sel.id) : undefined;
  const dom = sel?.kind === "domain" ? graph.domains.find((d) => d.id === sel.id) : node ? graph.domains.find((d) => d.id === node.domain) : undefined;
  const nodeById = new Map(graph.nodes.map((n) => [n.id, n]));
  const item = (n: KnowledgeGraphNode, meta: string, metaColor = "var(--muted)") => (
    <button
      key={n.id}
      onClick={() => setSel({ kind: "node", id: n.id })}
      className="flex cursor-pointer items-center gap-2 rounded-[7px] border border-line px-2 py-1.5 text-left text-[12px] leading-[1.3] text-text hover:border-accent/40"
    >
      <span className={n.kind === "doc" ? "size-[7px] flex-none rounded-[2px]" : "size-[7px] flex-none rotate-45"} style={{ background: n.kind === "doc" ? KIND_COLOR[n.doc_kind ?? "doc"] : n.state === "missing" ? "#F26D6D" : n.state === "stale" ? "#F2C14E" : "#8b95a5" }} />
      <span className="min-w-0 flex-1 truncate">{n.kind === "doc" ? n.label : n.path}</span>
      <span className="flex-none font-mono text-[10.5px]" style={{ color: metaColor }}>
        {meta}
      </span>
    </button>
  );
  let body: React.ReactNode;
  if (node) {
    const out = graph.edges.filter((e) => e.from === node.id).map((e) => ({ e, n: nodeById.get(e.to)! })).filter((x) => x.n);
    const inc = graph.edges.filter((e) => e.to === node.id).map((e) => ({ e, n: nodeById.get(e.from)! })).filter((x) => x.n);
    body = (
      <>
        <div className="flex items-center gap-2">
          <span className={node.kind === "doc" ? "size-[9px] rounded-[2px]" : "size-[9px] rotate-45"} style={{ background: node.kind === "doc" ? KIND_COLOR[node.doc_kind ?? "doc"] : "#8b95a5" }} />
          <span className={lbl}>{node.kind === "doc" ? KIND_LABEL[node.doc_kind ?? "doc"] : "code file"}</span>
          <button onClick={() => setSel(null)} className="ml-auto cursor-pointer font-mono text-[11px] text-muted">
            clear ×
          </button>
        </div>
        <span className="text-[17px] leading-[1.25] font-semibold [overflow-wrap:anywhere]">{node.kind === "doc" ? node.label : node.label}</span>
        <span className="font-mono text-[11px] leading-[1.4] [overflow-wrap:anywhere] text-text-2">{node.path}</span>
        {node.state !== "ok" && (
          <span className="rounded-[7px] border px-2.5 py-2 font-mono text-[11.5px] leading-[1.45]" style={{ borderColor: node.state === "missing" ? "rgba(242,109,109,.4)" : "rgba(242,193,78,.4)", color: node.state === "missing" ? "#F26D6D" : "#F2C14E" }}>
            {node.kind === "code"
              ? node.state === "missing"
                ? "cited by a document but not in the repo (moved, renamed or deleted)"
                : "a document citing it is stale: the code changed after the document"
              : node.state === "missing"
                ? "cites a file that does not exist"
                : "a file it cites changed after it was last updated"}
          </span>
        )}
        {node.kind === "doc" && (
          <button onClick={() => onOpenDoc(node.path)} className="cursor-pointer self-start text-[12.5px] font-medium text-link hover:text-link-hover">
            Open the document →
          </button>
        )}
        {out.length > 0 && (
          <div className="flex flex-col gap-1.5">
            <span className={lbl}>{node.kind === "doc" ? "cites and links" : "points at"}</span>
            {out.slice(0, 40).map(({ e, n }) => item(n, e.kind, e.state === "missing" ? "#F26D6D" : e.state === "stale" ? "#F2C14E" : undefined))}
          </div>
        )}
        {inc.length > 0 && (
          <div className="flex flex-col gap-1.5">
            <span className={lbl}>{node.kind === "doc" ? "linked from" : "cited by"}</span>
            {inc.slice(0, 40).map(({ e, n }) => item(n, e.kind))}
          </div>
        )}
      </>
    );
  } else if (dom) {
    const members = graph.nodes.filter((n) => n.domain === dom.id).sort((a, b) => b.degree - a.degree);
    body = (
      <>
        <div className="flex items-center gap-2">
          <span className="size-[9px] rounded-full" style={{ background: domColor.get(dom.id) }} />
          <span className={lbl}>domain</span>
          <button onClick={() => setSel(null)} className="ml-auto cursor-pointer font-mono text-[11px] text-muted">
            clear ×
          </button>
        </div>
        <span className="text-[17px] leading-[1.25] font-semibold">{dom.name}</span>
        <span className="text-[12.5px] leading-[1.55] text-text-2">{dom.summary}.</span>
        <div className="flex flex-col gap-1.5">
          <span className={lbl}>documents</span>
          {members.filter((n) => n.kind === "doc").slice(0, 30).map((n) => item(n, `${n.degree}`))}
        </div>
        <div className="flex flex-col gap-1.5">
          <span className={lbl}>code</span>
          {members.filter((n) => n.kind === "code").slice(0, 30).map((n) => item(n, n.state === "ok" ? `${n.degree}` : n.state, n.state === "missing" ? "#F26D6D" : n.state === "stale" ? "#F2C14E" : undefined))}
        </div>
      </>
    );
  } else {
    body = (
      <>
        <span className={lbl}>domains</span>
        <span className="text-[12.5px] leading-[1.55] text-text-2">Click a node to see what it cites and who cites it, or pick a domain:</span>
        {graph.domains.slice(0, 40).map((d) => (
          <button key={d.id} onClick={() => setSel({ kind: "domain", id: d.id })} className="flex cursor-pointer items-center gap-2 rounded-[7px] border border-line px-2 py-1.5 text-left text-[12px] text-text hover:border-accent/40">
            <span className="size-[7px] flex-none rounded-full" style={{ background: domColor.get(d.id) }} />
            <span className="min-w-0 flex-1 truncate">{d.name}</span>
            <span className="flex-none font-mono text-[10.5px]" style={{ color: d.flagged ? "#F2C14E" : "var(--muted)" }}>
              {d.docs}·{d.code}
              {d.flagged ? ` · ${d.flagged}!` : ""}
            </span>
          </button>
        ))}
      </>
    );
  }
  return (
    <div className="flex max-h-[580px] max-w-[380px] min-w-0 flex-[1_1_280px] flex-col gap-3 overflow-auto rounded-[10px] border border-line bg-surface p-3.5">
      {body}
      <span className="mt-auto font-mono text-[10.5px] leading-normal text-muted/80">built from the index on each load · no LLM · domains are the code folders documents cite</span>
    </div>
  );
}
