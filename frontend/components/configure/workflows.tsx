"use client";

/** Configure › Workflows (design: ConfigWorkflow.dc.html; backend workflows): which agents work on a task, in what
 * order, and which gates and hooks sit between them. A node graph: ▶ trigger, ○ agent step, ✓ verify, ◎ audit,
 * ⚑ hook, ■ end; edges next (grey), pass (green), fail (red, "fail · retry ≤N" when it goes back). Drag to move
 * (10 px snap), click to edit in the inspector, "add after selected" inserts a step and rewires its outgoing edge,
 * ⇔ Tidy lays it out by longest path. "Ask an agent" proposes ops shown as an overlay diff or side by side; Accept
 * applies the checked ones. Everything is a draft until the save bar's Save. */

import { useEffect, useState } from "react";

import { configureApi, workflowsApi, type AskReply, type ConfigOp, type Workflow, type WorkflowEdge, type WorkflowNode, type WorkflowStats } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { AgentsOverview } from "@/lib/generated/AgentsOverview";
import type { Effort } from "@/lib/generated/Effort";
import type { JsonValue } from "@/lib/generated/serde_json/JsonValue";

import { api } from "@/lib/api";
import { useApp } from "../app-context";
import { Note, Seg, SourceBadge, TabHead, btnCls, dangerCls, inputCls, primaryCls, same, useConfigure } from "./ui";

const NW = 156;
const NH = 60;
const COLW = 200;
const PAD = 28;
type Kind = WorkflowNode["kind"];
const TYPE: Record<Kind, [string, string | null, string]> = {
  trigger: ["▶", "#8b95a5", "trigger"],
  agent: ["○", null, "agent step"],
  verify: ["✓", "#4ade80", "verify gate"],
  audit: ["◎", "#38bdf8", "audit gate"],
  hook: ["⚑", "#F2C14E", "hook"],
  end: ["■", null, "end"],
};
const EDGE: Record<WorkflowEdge["kind"], string> = { next: "#5d6673", pass: "#4ade80", fail: "#F26D6D" };

/** A suggested op on the graph (the design's `add` / `insertBefore` / `remove` / `change`). */
type GraphOp =
  | { op: "add"; after: string; before: string; fail_to?: string | null; node: Partial<WorkflowNode> & { id: string; kind: Kind } }
  | { op: "insert_before"; target: string; node: Partial<WorkflowNode> & { id: string; kind: Kind } }
  | { op: "remove"; id: string; bridge?: [string, string, WorkflowEdge["kind"]] }
  | { op: "change"; id: string; key: string; to: unknown };

type Flag = "add" | "rem" | "chg" | undefined;
type FNode = WorkflowNode & { flag?: Flag };
type FEdge = WorkflowEdge & { flag?: Flag };
type Graph = { nodes: FNode[]; edges: FEdge[] };

const clone = <T,>(o: T): T => JSON.parse(JSON.stringify(o));
let seq = 0;
const newId = (k: string) => `${k}${Date.now().toString(36).slice(-3)}${++seq}`;

function blankNode(id: string, kind: Kind, x = 0, y = 0): WorkflowNode {
  const base: WorkflowNode = { id, kind, label: "", x, y, agent: null, prompt: null, command: null, max_attempts: null, verdict: null, event: null, blocking: null, timeout_secs: null, outcome: null };
  const fill: Record<Kind, Partial<WorkflowNode>> = {
    trigger: { label: "Task ready" },
    agent: { label: "New step" },
    verify: { label: "Check", command: "bun test", max_attempts: 2 },
    audit: { label: "Audit", verdict: "VERDICT: PASS" },
    hook: { label: "Hook", event: "post_done", command: "echo done", blocking: false, timeout_secs: 60 },
    end: { label: "Done", outcome: "done" },
  };
  return { ...base, ...fill[kind] };
}

/** Longest path over the forward edges (back edges found by DFS from the trigger), ends sorted done before failed. */
function tidy(w: Graph): Graph {
  const col: Record<string, number> = {};
  w.nodes.forEach((n) => (col[n.id] = 0));
  const back = new Set<FEdge>();
  const seen = new Set<string>();
  const stack = new Set<string>();
  const dfs = (id: string) => {
    seen.add(id);
    stack.add(id);
    w.edges
      .filter((e) => e.from === id)
      .sort((a, b) => (a.kind === "fail" ? 1 : 0) - (b.kind === "fail" ? 1 : 0))
      .forEach((e) => {
        if (stack.has(e.to)) back.add(e);
        else if (!seen.has(e.to)) dfs(e.to);
      });
    stack.delete(id);
  };
  w.nodes.filter((n) => n.kind === "trigger").forEach((n) => dfs(n.id));
  w.nodes.forEach((n) => !seen.has(n.id) && dfs(n.id));
  const fwd = w.edges.filter((e) => !back.has(e));
  for (let i = 0; i < w.nodes.length; i++)
    fwd.forEach((e) => {
      if (col[e.from] !== undefined && col[e.to] !== undefined && col[e.to] < col[e.from] + 1 && col[e.from] + 1 < w.nodes.length) col[e.to] = col[e.from] + 1;
    });
  // An end nothing leads to (a failed end) sits with the other ends, in the last column.
  const last = Math.max(0, ...Object.values(col));
  w.nodes.forEach((n) => n.kind === "end" && col[n.id] === 0 && (col[n.id] = last));
  const groups: Record<number, FNode[]> = {};
  w.nodes.forEach((n) => (groups[col[n.id]] = groups[col[n.id]] ?? []).push(n));
  Object.entries(groups).forEach(([c, g]) => {
    g.sort((a, b) => (a.kind === "end" && b.kind === "end" ? Number(a.outcome === "failed") - Number(b.outcome === "failed") : a.y - b.y));
    g.forEach((n, i) => {
      n.x = Number(c) * COLW;
      n.y = 170 + (i - (g.length - 1) / 2) * 110;
    });
  });
  return w;
}

/** Applies suggested ops; with `ghost` the changes stay visible as flags (the overlay diff). */
function applyOps(w0: Graph, ops: GraphOp[], ghost: boolean): Graph {
  const w = clone(w0);
  const byId = (id: string) => w.nodes.find((n) => n.id === id);
  const F: Flag = ghost ? "add" : undefined;
  const E = (from: string, to: string, kind: WorkflowEdge["kind"]) => w.edges.push({ from, to, kind, ...(F ? { flag: F } : {}) });
  const drop = (pred: (e: FEdge) => boolean) => {
    if (ghost) w.edges.forEach((e) => pred(e) && (e.flag = "rem"));
    else w.edges = w.edges.filter((e) => !pred(e));
  };
  const make = (o: Partial<WorkflowNode> & { id: string; kind: Kind }, x: number, y: number): FNode => ({ ...blankNode(o.id, o.kind, x, y), ...o, x, y, ...(ghost ? { flag: "add" as Flag } : {}) });
  for (const o of ops) {
    if (o.op === "add") {
      const a = byId(o.after);
      const b = byId(o.before);
      const old = w.edges.find((e) => e.from === o.after && e.to === o.before && !e.flag);
      if (!a || !b || !old || byId(o.node.id)) continue;
      w.nodes.push(make(o.node, (a.x + b.x) / 2, a.y - 100));
      drop((e) => e === old);
      E(o.after, o.node.id, old.kind);
      E(o.node.id, o.before, o.node.kind === "verify" || o.node.kind === "audit" ? "pass" : "next");
      if (o.fail_to) E(o.node.id, o.fail_to, "fail");
    } else if (o.op === "insert_before") {
      const t = byId(o.target);
      if (!t || byId(o.node.id)) continue;
      w.nodes.push(make(o.node, t.x, t.y + 100));
      const ins = w.edges.filter((e) => e.to === o.target && !e.flag);
      drop((e) => ins.includes(e));
      ins.forEach((e) => E(e.from, o.node.id, e.kind));
      E(o.node.id, o.target, "next");
    } else if (o.op === "remove") {
      const n = byId(o.id);
      if (!n) continue;
      if (ghost) n.flag = "rem";
      else w.nodes = w.nodes.filter((x) => x !== n);
      drop((e) => e.from === o.id || e.to === o.id);
      if (o.bridge && byId(o.bridge[0]) && byId(o.bridge[1])) E(o.bridge[0], o.bridge[1], o.bridge[2]);
    } else if (o.op === "change") {
      const n = byId(o.id);
      if (!n) continue;
      (n as Record<string, unknown>)[o.key] = o.to;
      if (ghost) n.flag = "chg";
    }
  }
  return w;
}

/** A backend op's `value` as a graph op, when it is one. */
function graphOp(o: ConfigOp): GraphOp | null {
  const v = o.value as GraphOp | null;
  return v && typeof v === "object" && "op" in v ? v : null;
}

export function WorkflowsTab() {
  const { scope, slug, setPending, flash } = useConfigure();
  const { providers, project } = useApp();
  const [loaded, setLoaded] = useState<Workflow[] | null>(null);
  const [wfs, setWfs] = useState<Workflow[] | null>(null);
  const [removed, setRemoved] = useState<Workflow[]>([]);
  const [cur, setCur] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);
  const [agents, setAgents] = useState<AgentsOverview | null>(null);
  const [stats, setStats] = useState<WorkflowStats | null>(null);
  const [undo, setUndo] = useState<Workflow[] | null>(null);
  const [newKind, setNewKind] = useState<WorkflowEdge["kind"]>("next");
  const [newTo, setNewTo] = useState("");
  // Ask an agent
  const usable = providers.filter((p) => ["claude", "codex", "gemini"].includes(p.name));
  const [prov, setProv] = useState("claude");
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState("medium");
  const [prompt, setPrompt] = useState("");
  const [phase, setPhase] = useState<"idle" | "thinking" | "ready">("idle");
  const [reply, setReply] = useState<AskReply | null>(null);
  const [opsOn, setOpsOn] = useState<Record<number, boolean>>({});
  const [view, setView] = useState<"overlay" | "side">("overlay");
  const { cfgLang } = useConfigure();

  useEffect(() => {
    let live = true;
    workflowsApi.list(slug).then(
      (list) => {
        if (!live) return;
        const mine = list.workflows.filter((w) => scope === "project" || w.source === "machine");
        setLoaded(mine);
        setWfs(clone(mine));
        setRemoved([]);
        setCur((c) => (c && mine.some((w) => w.id === c) ? c : (mine[0]?.id ?? null)));
        setError("");
      },
      (e: Error) => live && setError(e.message),
    );
    api.agents(scope === "project" ? slug : project).then((a) => live && setAgents(a), () => {});
    return () => {
      live = false;
    };
  }, [slug, scope, project, tick]);

  const w = wfs?.find((x) => x.id === cur) ?? null;
  useEffect(() => {
    if (!w || !loaded?.some((x) => x.id === w.id)) {
      setStats(null);
      return;
    }
    workflowsApi.stats(w.id, slug).then(setStats, () => setStats(null));
  }, [w?.id, slug]); // eslint-disable-line react-hooks/exhaustive-deps

  const dirty = [...(wfs ?? []).filter((x) => !same(x, loaded?.find((l) => l.id === x.id))), ...removed];
  useEffect(() => {
    setPending(
      "workflows",
      dirty.length
        ? {
            count: dirty.length,
            files: [scope === "machine" ? "machine settings (workflows)" : "project settings (workflows)"],
            save: async () => {
              for (const r of removed) await workflowsApi.remove(slug, r.id);
              for (const x of (wfs ?? []).filter((x) => !same(x, loaded?.find((l) => l.id === x.id)))) await workflowsApi.save(slug, x);
              setTick((t) => t + 1);
            },
            discard: () => setTick((t) => t + 1),
          }
        : null,
    );
  }, [JSON.stringify(dirty), slug, scope]); // eslint-disable-line react-hooks/exhaustive-deps

  const agentNames = (agents?.rows ?? []).filter((r) => scope === "project" || r.scope === "user").map((r) => r.name);
  const agentOf = (name: string | null) => (agents?.rows ?? []).find((r) => r.name === name);

  /** Edits the current workflow; a machine workflow edited in project scope becomes this project's override. */
  const edit = (fn: (g: Workflow) => void, msg?: string, keepUndo = false) => {
    if (!w || !wfs) return;
    const prev = clone(wfs);
    const next = clone(wfs);
    const g = next.find((x) => x.id === w.id)!;
    fn(g);
    if (g.source === "machine" && scope === "project") g.source = "override";
    setWfs(next);
    if (msg) flash(msg);
    if (keepUndo) setUndo(prev);
  };

  const color = (n: WorkflowNode) => {
    if (n.kind === "agent") return providerColor(agentOf(n.agent)?.files[0]?.provider ?? n.agent ?? "");
    if (n.kind === "end") return n.outcome === "failed" ? "#F26D6D" : "#4ade80";
    return TYPE[n.kind][1] ?? "#8b95a5";
  };
  const meta = (n: WorkflowNode, g: Workflow) => {
    const a = agentOf(n.agent);
    if (n.kind === "trigger") return `deps done · ${triggerText(g)}`;
    if (n.kind === "agent") return a ? `${a.name} · ${a.files.map((f) => f.provider).join("/")}` : n.agent ? `${n.agent} (provider)` : "no agent";
    if (n.kind === "verify") return `${n.command ?? ""} · ≤${n.max_attempts ?? 1}`;
    if (n.kind === "audit") return `${n.agent ?? "auditor"} · ${n.verdict ?? "VERDICT: PASS"}`;
    if (n.kind === "hook") return `${n.event ?? ""} · ${n.command ?? ""}`;
    return n.outcome === "failed" ? "a person decides" : "task closed";
  };

  const ready = phase === "ready" && reply;
  const gops = ready ? reply.ops.map(graphOp) : [];
  const chosen = gops.filter((o, i): o is GraphOp => !!o && !!opsOn[i]);

  if (error) return <Wrap>{<Note tone="bad">Could not read the workflows: {error}</Note>}</Wrap>;
  if (!wfs) return <Wrap>{<Note>Reading the workflows…</Note>}</Wrap>;

  const selN = w?.nodes.find((n) => n.id === sel) ?? null;
  const canAdd = !!selN && selN.kind !== "end";
  const nameOf = (id: string) => w?.nodes.find((n) => n.id === id)?.label ?? id;

  const addStep = (kind: Kind) => {
    if (!w || !selN || selN.kind === "end") return;
    const id = newId(kind);
    edit((g) => {
      const s = g.nodes.find((n) => n.id === selN.id)!;
      const out = g.edges.find((e) => e.from === s.id && e.kind !== "fail");
      g.nodes.push({ ...blankNode(id, kind, s.x + COLW / 2, s.y - 100), agent: kind === "agent" || kind === "audit" ? (agentNames[0] ?? "claude") : null });
      if (out && kind !== "end") {
        g.edges = g.edges.filter((e) => e !== out);
        g.edges.push({ from: s.id, to: id, kind: out.kind }, { from: id, to: out.to, kind: kind === "verify" || kind === "audit" ? "pass" : "next" });
      } else g.edges.push({ from: s.id, to: id, kind: out ? "next" : s.kind === "verify" || s.kind === "audit" ? "pass" : "next" });
      if (kind === "verify" || kind === "audit") g.edges.push({ from: id, to: s.kind === "agent" ? s.id : (g.nodes.find((n) => n.kind === "agent") ?? s).id, kind: "fail" });
      const t = tidy(g);
      g.nodes = t.nodes as WorkflowNode[];
    });
    setSel(id);
  };

  const newWorkflow = () => {
    const id = `wf-${(wfs.length + 1).toString()}-${++seq}`;
    const g = tidy({
      nodes: [blankNode("t", "trigger"), { ...blankNode("a1", "agent"), label: "Implement", agent: agentNames[0] ?? "claude" }, { ...blankNode("v1", "verify"), label: "Verify" }, blankNode("done", "end"), { ...blankNode("failed", "end"), label: "Failed", outcome: "failed" }],
      edges: [
        { from: "t", to: "a1", kind: "next" },
        { from: "a1", to: "v1", kind: "next" },
        { from: "v1", to: "done", kind: "pass" },
        { from: "v1", to: "a1", kind: "fail" },
      ],
    });
    const nw: Workflow = { id, name: `workflow-${wfs.length + 1}`, description: "", trigger: { default: false, keywords: [id], paths: [] }, nodes: g.nodes as WorkflowNode[], edges: g.edges, source: scope === "machine" ? "machine" : "project" };
    setWfs([...wfs, nw]);
    setCur(id);
    setSel("a1");
    flash(`Created ${nw.name}`);
  };

  const suggest = async () => {
    if (!w || phase === "thinking") return;
    setPhase("thinking");
    try {
      const r = await configureApi.ask({
        tab: "workflows",
        project: slug,
        provider: prov,
        model: model ?? null,
        effort: effort as Effort,
        prompt: prompt.trim(),
        language: cfgLang,
        context: { workflow: w, agents: agentNames } as unknown as JsonValue,
        workflow_id: w.id,
        from_runs: !prompt.trim(),
      });
      setReply(r);
      setOpsOn(Object.fromEntries(r.ops.map((_, i) => [i, !!graphOp(r.ops[i])])));
      setSel(null);
      setPhase("ready");
    } catch (e) {
      flash((e as Error).message, "bad");
      setPhase("idle");
    }
  };

  const mainGraph: Graph | null = w ? (ready && view === "overlay" ? applyOps(w, chosen, true) : w) : null;
  const info = providers.find((p) => p.name === prov);
  const m = model ?? info?.default_model ?? "";

  return (
    <Wrap>
      <div className="flex flex-col gap-3.5">
        <div className="flex flex-wrap items-center gap-2">
          {wfs.map((x) => (
            <button
              key={x.id}
              onClick={() => {
                setCur(x.id);
                setSel(null);
                setPhase("idle");
              }}
              className="flex h-[30px] cursor-pointer items-center gap-2 rounded-[7px] border px-3 font-mono text-[12px] font-medium whitespace-nowrap"
              style={{ borderColor: x.id === cur ? "var(--accent)" : "var(--line-2)", background: x.id === cur ? "var(--accent-tint)" : "transparent", color: x.id === cur ? "var(--accent-soft)" : "var(--nav)" }}
            >
              {x.name}
              <SourceBadge source={x.source} />
            </button>
          ))}
          <button onClick={newWorkflow} className="h-[30px] cursor-pointer rounded-[7px] border border-dashed border-line-3 px-3 text-[12px] font-medium whitespace-nowrap text-nav hover:border-accent hover:text-accent-soft">
            + New workflow
          </button>
          {w && <span className="ml-auto font-mono text-[11px] leading-[1.3] text-muted">runs for {triggerText(w)}</span>}
        </div>

        {!w && <Note>No workflows yet: tasks run the fixed pipeline (agent → verify → audit → land). + New workflow makes one; a task uses it when its trigger matches.</Note>}

        {w && (w.source === "override" || (w.source === "machine" && scope === "project")) && (
          <div className="rounded-lg border px-3 py-[9px] text-[12px] leading-[1.45] text-text" style={{ borderColor: "color-mix(in srgb, var(--warn) 30%, transparent)", background: "color-mix(in srgb, var(--warn) 6%, transparent)" }}>
            {w.source === "override"
              ? `This project now has its own copy of ${w.name}. It overrides the machine workflow here; other projects keep the machine one.`
              : `${w.name} comes from the machine settings. Editing it here saves a project copy that overrides it.`}
          </div>
        )}

        {w && (
          <div className="flex flex-col gap-3 rounded-xl border p-3.5" style={{ borderColor: "color-mix(in srgb, var(--accent) 25%, transparent)", background: "color-mix(in srgb, var(--accent) 3.5%, transparent)" }}>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <span className="flex items-center gap-2 text-[13px] leading-none font-semibold">
                <span className="text-accent">✦</span>Ask an agent to do it
              </span>
              <span className="text-[11.5px] leading-[1.3] text-muted">Describe the change, or leave it empty and the agent suggests changes from recent runs. You review a diff before anything changes.</span>
              <div className="ml-auto flex gap-0.5 rounded-[7px] border border-line-2 p-0.5">
                {usable.map((p) => (
                  <button
                    key={p.name}
                    onClick={() => {
                      setProv(p.name);
                      setModel(null);
                    }}
                    className="flex h-6 cursor-pointer items-center gap-1.5 rounded-[5px] px-2 font-mono text-[11px] font-medium"
                    style={{ background: prov === p.name ? "color-mix(in srgb, var(--text) 8%, transparent)" : "transparent", color: prov === p.name ? "var(--text)" : "var(--muted)" }}
                  >
                    <span className="size-1.5 rounded-full" style={{ background: providerColor(p.name) }} />
                    {p.name}
                  </button>
                ))}
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-x-3.5 gap-y-2">
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase">model</span>
                {(info?.models.length ? info.models : [info?.default_model ?? ""]).filter(Boolean).map((x) => (
                  <button
                    key={x}
                    onClick={() => setModel(x)}
                    className="h-6 cursor-pointer rounded-[5px] border px-2 font-mono text-[11px] font-medium"
                    style={{ borderColor: x === m ? "var(--accent)" : "var(--line-2)", background: x === m ? "var(--accent-tint)" : "transparent", color: x === m ? "var(--accent-soft)" : "var(--text-2)" }}
                  >
                    {x}
                  </button>
                ))}
              </div>
              <div className="flex items-center gap-1.5">
                <span className="font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase">{prov === "codex" ? "reasoning effort" : prov === "gemini" ? "thinking budget" : "thinking"}</span>
                <Seg value={effort} options={["low", "medium", "high", "xhigh", "max"]} onChange={setEffort} />
              </div>
            </div>
            <div className="flex flex-wrap items-stretch gap-2">
              <textarea
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) suggest();
                }}
                rows={2}
                placeholder="e.g. tasks fail verify too often on lint errors; catch them earlier and tell me when a task fails"
                className="min-w-0 flex-[1_1_360px] resize-y rounded-lg border border-line-2 bg-inset px-3 py-2.5 text-[13px] leading-normal text-text outline-none focus:border-accent"
              />
              <button
                onClick={suggest}
                className="min-h-11 flex-none cursor-pointer rounded-lg border border-accent px-4 text-[12.5px] font-semibold whitespace-nowrap"
                style={{ background: phase === "thinking" ? "var(--accent-tint)" : "var(--accent)", color: phase === "thinking" ? "var(--accent-soft)" : "var(--accent-text)" }}
              >
                {phase === "thinking" ? `${prov} · ${m} · ${effort} · ${prompt.trim() ? "drafting changes" : "reading the last 30 runs"}…` : prompt.trim() ? "✦ Generate" : "✦ Suggest from runs"}
              </button>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {["Catch lint errors before the tests", "Write tests before verify and notify on fail", "Skip the planner and keep docs in step"].map((x) => (
                <button key={x} onClick={() => setPrompt(x)} className="h-[26px] cursor-pointer rounded-[13px] border border-line-2 px-2.5 text-[11.5px] whitespace-nowrap text-text-2 hover:border-accent/50 hover:text-accent-soft">
                  {x}
                </button>
              ))}
            </div>
          </div>
        )}

        {w && (
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="mr-1 font-mono text-[9.5px] leading-none font-medium tracking-[0.14em] text-muted uppercase">add after selected</span>
            {(["agent", "verify", "audit", "hook", "end"] as Kind[]).map((k) => (
              <button
                key={k}
                onClick={() => addStep(k)}
                disabled={!canAdd}
                className="flex h-7 items-center gap-1.5 rounded-md border border-line-2 px-2.5 text-[11.5px] font-medium whitespace-nowrap enabled:cursor-pointer enabled:hover:bg-white/5"
                style={{ color: canAdd ? "var(--text)" : "var(--muted)" }}
              >
                <span style={{ color: TYPE[k][1] ?? (k === "agent" ? "#5EA8F2" : "#4ade80") }}>{TYPE[k][0]}</span>
                {k[0].toUpperCase() + k.slice(1)}
              </button>
            ))}
            <button onClick={() => edit((g) => void Object.assign(g, tidy(g)))} className="h-7 cursor-pointer rounded-md border border-line-2 px-2.5 text-[11.5px] font-medium text-nav hover:bg-white/5">
              ⇔ Tidy
            </button>
            {undo && (
              <button
                onClick={() => {
                  setWfs(undo);
                  setUndo(null);
                }}
                className="h-7 cursor-pointer px-2 text-[12px] font-semibold text-accent-soft"
              >
                Undo
              </button>
            )}
          </div>
        )}

        {w && mainGraph && (
          <div className="flex flex-wrap items-start gap-3.5">
            <div className="flex min-w-0 flex-[999_1_560px] flex-col gap-3.5">
              {!(ready && view === "side") && (
                <Canvas
                  g={mainGraph}
                  sel={sel}
                  drag={!(ready && view === "overlay")}
                  stats={stats}
                  color={color}
                  meta={(n) => meta(n, w)}
                  onSelect={(id) => {
                    setSel(id);
                    setNewTo("");
                  }}
                  onMove={(id, x, y) =>
                    edit((g) => {
                      const n = g.nodes.find((q) => q.id === id);
                      if (n) {
                        n.x = x;
                        n.y = y;
                      }
                    })
                  }
                />
              )}
              {ready && view === "side" && (
                <div className="grid gap-3 [grid-template-columns:repeat(auto-fit,minmax(min(380px,100%),1fr))]">
                  {[
                    { g: w as Graph, k: "current", c: "var(--muted)", b: "var(--line)" },
                    { g: tidy(applyOps(w, chosen, false)), k: `${prov} suggests · ${chosen.length} of ${gops.filter(Boolean).length} changes`, c: "var(--accent-soft)", b: "color-mix(in srgb, var(--accent) 35%, transparent)" },
                  ].map((s) => (
                    <div key={s.k} className="flex min-w-0 flex-col gap-2">
                      <span className="font-mono text-[10px] leading-none font-medium tracking-[0.14em] uppercase" style={{ color: s.c }}>
                        {s.k}
                      </span>
                      <Canvas g={s.g} sel={null} drag={false} stats={null} color={color} meta={(n) => meta(n, w)} scale={0.58} border={s.b} />
                    </div>
                  ))}
                </div>
              )}
              <div className="flex flex-wrap gap-x-4 gap-y-1.5 font-mono text-[10.5px] leading-[1.4] text-muted">
                {(["next", "pass", "fail"] as const).map((k) => (
                  <span key={k} className="flex items-center gap-1.5">
                    <span className="h-0.5 w-4" style={{ background: EDGE[k] }} />
                    {k === "fail" ? "fail / retry" : k}
                  </span>
                ))}
                <span>drag to move · click to edit · stats from the last 30 runs</span>
              </div>
              {ready && (
                <div className="flex flex-col gap-3 rounded-xl border p-4" style={{ borderColor: "color-mix(in srgb, var(--accent) 35%, transparent)", background: "color-mix(in srgb, var(--accent) 4%, transparent)" }}>
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                    <span className="size-2 rounded-full" style={{ background: providerColor(reply.provider) }} />
                    <span className="text-[14px] leading-[1.2] font-semibold">
                      {reply.provider} · {reply.model} · {effort} suggests {reply.ops.length} change{reply.ops.length === 1 ? "" : "s"} to {w.name}
                      {prompt.trim() ? ` for “${prompt.trim().slice(0, 70)}”` : " from its last 30 runs"}
                    </span>
                    <span className="ml-auto">
                      <Seg
                        value={view}
                        options={[
                          ["overlay", "A · Overlay"],
                          ["side", "B · Side by side"],
                        ]}
                        onChange={setView}
                      />
                    </span>
                  </div>
                  <p className="text-[13px] leading-normal text-nav">{reply.summary}</p>
                  <div className="flex flex-col gap-1">
                    {reply.evidence.map((e, i) => (
                      <span key={i} className="font-mono text-[11px] leading-[1.45] text-text-2">
                        · {e}
                      </span>
                    ))}
                  </div>
                  <div className="flex flex-col gap-1.5">
                    {reply.ops.map((o, i) => {
                      const on = !!opsOn[i];
                      const g = gops[i];
                      const glyph = !g ? "·" : g.op === "remove" ? "−" : g.op === "change" ? "~" : "+";
                      const c = !g ? "var(--muted)" : g.op === "remove" ? "#F26D6D" : g.op === "change" ? "#F2C14E" : "#7ee2a8";
                      return (
                        <button
                          key={i}
                          disabled={!g}
                          onClick={() => setOpsOn({ ...opsOn, [i]: !on })}
                          className="flex cursor-pointer items-center gap-2.5 rounded-lg border px-2.5 py-2 text-left font-mono text-[12px] leading-[1.35] disabled:cursor-not-allowed"
                          style={{ borderColor: on ? "var(--line-2)" : "var(--line)", background: on ? "color-mix(in srgb, var(--text) 3%, transparent)" : "transparent" }}
                          title={g ? undefined : "not a graph change; skipped"}
                        >
                          <span className="flex size-4 flex-none items-center justify-center rounded border text-[11px] font-bold text-accent-text" style={{ borderColor: on ? "var(--accent)" : "var(--line-3)", background: on ? "var(--accent)" : "transparent" }}>
                            {on ? "✓" : ""}
                          </span>
                          <span className="w-3.5 flex-none font-bold" style={{ color: c }}>
                            {glyph}
                          </span>
                          <span className="min-w-0 flex-1" style={{ color: on ? "var(--text)" : "var(--muted)" }}>
                            {o.text || (g ? opText(g, nameOf) : o.target)}
                          </span>
                        </button>
                      );
                    })}
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <button
                      onClick={() => {
                        if (!chosen.length) return;
                        edit(
                          (g) => {
                            const r = tidy(applyOps(g, chosen, false));
                            g.nodes = r.nodes as WorkflowNode[];
                            g.edges = r.edges;
                          },
                          `Applied ${chosen.length} changes from ${reply.provider} · unsaved`,
                          true,
                        );
                        setPhase("idle");
                      }}
                      className={primaryCls}
                    >
                      Accept {chosen.length} change{chosen.length === 1 ? "" : "s"}
                    </button>
                    <button onClick={() => setPhase("idle")} className={btnCls}>
                      Reject
                    </button>
                    <button onClick={suggest} className={btnCls}>
                      Ask again
                    </button>
                  </div>
                </div>
              )}
            </div>

            <Inspector
              w={w}
              selN={selN}
              agentNames={agentNames}
              nameOf={nameOf}
              color={color}
              newKind={newKind}
              newTo={newTo}
              setNewKind={setNewKind}
              setNewTo={setNewTo}
              canDeleteWf={!(w.source === "machine" && scope === "project")}
              onDeleteWf={() => {
                const was = loaded?.find((l) => l.id === w.id);
                if (was) setRemoved([...removed, was]);
                setWfs(wfs.filter((x) => x.id !== w.id));
                setCur(wfs.find((x) => x.id !== w.id)?.id ?? null);
                setSel(null);
              }}
              edit={edit}
              setSel={setSel}
              onTrigger={(t) => edit((g) => void (g.trigger = t))}
              onName={(n) => edit((g) => void (g.name = n))}
            />
          </div>
        )}
      </div>
    </Wrap>
  );

  function Wrap({ children }: { children: React.ReactNode }) {
    return (
      <>
        <TabHead
          title="Workflows"
          desc="Which agents work on a task, in what order, and which gates and hooks sit between them. Ask an agent to suggest changes from recent runs."
          savesTo={scope === "machine" ? "machine settings in the platform database (workflows)" : "project settings in the platform database (workflows)"}
        />
        {children}
      </>
    );
  }
}

function triggerText(w: Workflow) {
  const t = w.trigger;
  const parts = [t.default ? "every task without another match" : "", t.keywords.length ? `tasks mentioning ${t.keywords.join(", ")}` : "", t.paths.length ? `paths ${t.paths.join(", ")}` : ""].filter(Boolean);
  return parts.join(" · ") || "tasks that name it";
}

function opText(o: GraphOp, nameOf: (id: string) => string) {
  if (o.op === "add") return `Add ${TYPE[o.node.kind][2]} "${o.node.label ?? o.node.id}" between ${nameOf(o.after)} and ${nameOf(o.before)}`;
  if (o.op === "insert_before") return `Add ${o.node.event ?? ""} hook "${o.node.label ?? o.node.id}" before ${nameOf(o.target)} · ${o.node.command ?? ""}`;
  if (o.op === "remove") return `Remove "${nameOf(o.id)}"${o.bridge ? `; ${nameOf(o.bridge[0])} goes straight to ${nameOf(o.bridge[1])}` : ""}`;
  return `${nameOf(o.id)}: ${o.key} → ${String(o.to)}`;
}

function Canvas({
  g,
  sel,
  drag,
  stats,
  color,
  meta,
  onSelect,
  onMove,
  scale = 1,
  border = "var(--line)",
}: {
  g: Graph;
  sel: string | null;
  drag: boolean;
  stats: WorkflowStats | null;
  color: (n: WorkflowNode) => string;
  meta: (n: WorkflowNode) => string;
  onSelect?: (id: string | null) => void;
  onMove?: (id: string, x: number, y: number) => void;
  scale?: number;
  border?: string;
}) {
  const [moving, setMoving] = useState<{ id: string; x: number; y: number } | null>(null);
  const nodes = g.nodes.map((n) => (moving && n.id === moving.id ? { ...n, x: moving.x, y: moving.y } : n));
  if (!nodes.length) return null;
  const minX = Math.min(...nodes.map((n) => n.x));
  const minY = Math.min(...nodes.map((n) => n.y)) - 20;
  const pos: Record<string, { x: number; y: number }> = {};
  nodes.forEach((n) => (pos[n.id] = { x: n.x - minX + PAD, y: n.y - minY + PAD + 10 }));
  const labels: { x: number; y: number; text: string; color: string; border: string }[] = [];
  const shown = g.edges.filter((e) => pos[e.from] && pos[e.to]);
  // Backward edges loop below the nodes, each one lower than the last.
  const lows: Record<number, number> = {};
  let back = 0;
  shown.forEach((e, i) => {
    const s = pos[e.from];
    const t = pos[e.to];
    if (t.x > s.x + 20 || Math.abs(t.x - s.x) <= 20) return;
    lows[i] = Math.max(s.y + NH, t.y + NH + 2) + 56 + back * 20;
    back++;
  });
  const maxLow = Math.max(0, ...Object.values(lows));
  const edges = shown.map((e, i) => {
      const s = pos[e.from];
      const t = pos[e.to];
      const src = nodes.find((n) => n.id === e.from);
      const tgt = nodes.find((n) => n.id === e.to);
      let d: string;
      let lx: number;
      let ly: number;
      if (t.x > s.x + 20) {
        const sx = s.x + NW;
        const sy = s.y + NH / 2;
        const tx = t.x - 2;
        const ty = t.y + NH / 2;
        const dx = Math.max(36, (tx - sx) * 0.5);
        d = `M${sx} ${sy} C${sx + dx} ${sy},${tx - dx} ${ty},${tx} ${ty}`;
        lx = (sx + tx) / 2;
        ly = (sy + ty) / 2;
      } else if (Math.abs(t.x - s.x) <= 20) {
        const x = s.x + NW / 2;
        d = t.y > s.y ? `M${x} ${s.y + NH} L${x} ${t.y - 2}` : `M${x} ${s.y} L${x} ${t.y + NH + 2}`;
        lx = x + 22;
        ly = (s.y + t.y + NH) / 2;
      } else {
        const sx = s.x + NW / 2;
        const sy = s.y + NH;
        const tx = t.x + NW / 2;
        const ty = t.y + NH + 2;
        const low = lows[i];
        d = `M${sx} ${sy} C${sx} ${low},${tx} ${low},${tx} ${ty}`;
        lx = (sx + tx) / 2;
        ly = 0.75 * low + (0.25 * (sy + ty)) / 2;
      }
      const add = e.flag === "add";
      const rem = e.flag === "rem";
      const text = e.kind === "pass" ? "pass" : e.kind === "fail" ? (tgt && tgt.kind !== "end" ? `fail · retry ≤${src?.max_attempts ?? 1}` : "fail") : "";
      if (text || add || rem)
        labels.push({
          x: lx,
          y: ly,
          text: (add ? "+ " : rem ? "− " : "") + (text || e.kind),
          color: add ? "#7ee2a8" : rem ? "#F26D6D" : EDGE[e.kind],
          border: add ? "rgba(74,222,128,.4)" : rem ? "rgba(242,109,109,.4)" : "var(--line-2)",
        });
      return (
        <path
          key={i}
          d={d}
          fill="none"
          stroke={add ? "#7ee2a8" : rem ? "#F26D6D" : EDGE[e.kind]}
          strokeWidth={add ? 2 : 1.5}
          strokeDasharray={add || rem ? "5 4" : undefined}
          opacity={rem ? 0.55 : 1}
          markerEnd={`url(#wf-${add ? "add" : e.kind})`}
        />
      );
    });
  const W = Math.max(...Object.values(pos).map((p) => p.x)) + NW + PAD + 30;
  const H = Math.max(Math.max(...Object.values(pos).map((p) => p.y)) + NH + 40, maxLow + 30) + PAD;

  const down = (ev: React.PointerEvent, n: FNode) => {
    if (!drag || ev.button !== 0) return;
    ev.stopPropagation();
    const start = { px: ev.clientX, py: ev.clientY, x: n.x, y: n.y };
    let moved = false;
    let last = { x: n.x, y: n.y };
    const mv = (e: PointerEvent) => {
      const dx = (e.clientX - start.px) / scale;
      const dy = (e.clientY - start.py) / scale;
      if (!moved && Math.hypot(dx, dy) < 4) return;
      moved = true;
      last = { x: Math.round((start.x + dx) / 10) * 10, y: Math.round((start.y + dy) / 10) * 10 };
      setMoving({ id: n.id, ...last });
    };
    const up = () => {
      window.removeEventListener("pointermove", mv);
      window.removeEventListener("pointerup", up);
      setMoving(null);
      if (moved) onMove?.(n.id, last.x, last.y);
      else onSelect?.(n.id);
    };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", up);
  };

  return (
    <div className="relative max-h-[560px] overflow-auto rounded-xl border bg-inset" style={{ borderColor: border }}>
      <div style={{ width: W * scale, height: H * scale, position: "relative" }}>
        <div
          onClick={(e) => e.target === e.currentTarget && onSelect?.(null)}
          className="absolute top-0 left-0 origin-top-left"
          style={{
            width: W,
            height: H,
            transform: scale !== 1 ? `scale(${scale})` : undefined,
            backgroundImage: "radial-gradient(color-mix(in srgb, var(--text) 7%, transparent) 1px, transparent 1px)",
            backgroundSize: "22px 22px",
          }}
        >
          <svg width={W} height={H} className="pointer-events-none absolute top-0 left-0 overflow-visible">
            <defs>
              {(
                [
                  ["next", EDGE.next],
                  ["pass", EDGE.pass],
                  ["fail", EDGE.fail],
                  ["add", "#7ee2a8"],
                ] as const
              ).map(([id, c]) => (
                <marker key={id} id={`wf-${id}`} viewBox="0 0 8 8" refX={7} refY={4} markerWidth={7} markerHeight={7} orient="auto">
                  <path d="M0 0L8 4L0 8z" fill={c} />
                </marker>
              ))}
            </defs>
            {edges}
          </svg>
          {labels.map((l, i) => (
            <span
              key={i}
              className="pointer-events-none absolute -translate-x-1/2 -translate-y-1/2 rounded bg-inset px-1.5 py-0.5 font-mono text-[9.5px] leading-[1.2] font-medium whitespace-nowrap"
              style={{ left: l.x, top: l.y, border: `1px solid ${l.border}`, color: l.color }}
            >
              {l.text}
            </span>
          ))}
          {nodes.map((n) => {
            const c = color(n);
            const f = n.flag;
            const isSel = sel === n.id;
            const tag = f === "add" ? "+ added" : f === "rem" ? "− removed" : f === "chg" ? "~ changed" : "";
            const tagColor = f === "add" ? "#7ee2a8" : f === "rem" ? "#F26D6D" : "#F2C14E";
            const st = stats?.nodes.find((s) => s.node === n.id);
            return (
              <div
                key={n.id}
                onPointerDown={(e) => down(e, n)}
                onClick={() => !drag && onSelect?.(n.id)}
                className="absolute flex items-center gap-[9px] rounded-[10px] px-2.5 select-none"
                style={{
                  left: pos[n.id].x,
                  top: pos[n.id].y,
                  width: NW,
                  height: NH,
                  border: `1px ${f === "add" || f === "rem" ? "dashed" : "solid"} ${f ? tagColor : isSel ? "#FF7A1A" : "var(--line-3)"}`,
                  background: f === "add" ? "rgba(74,222,128,.06)" : "var(--menu)",
                  boxShadow: isSel ? "0 0 0 3px rgba(255,122,26,.2)" : undefined,
                  opacity: f === "rem" ? 0.55 : 1,
                  cursor: drag ? "grab" : "pointer",
                  touchAction: "none",
                }}
              >
                <span className="flex size-[26px] flex-none items-center justify-center rounded-[7px] font-mono text-[13px] font-semibold" style={{ background: `color-mix(in srgb, ${c} 14%, transparent)`, color: c }}>
                  {TYPE[n.kind][0]}
                </span>
                <span className="flex min-w-0 flex-col gap-[5px]">
                  <span className="truncate text-[12.5px] leading-[1.1] font-semibold" style={{ textDecoration: f === "rem" ? "line-through" : undefined }}>
                    {n.label}
                  </span>
                  <span className="truncate font-mono text-[10px] leading-[1.1] text-muted">{meta(n)}</span>
                </span>
                {tag && (
                  <span className="absolute -top-[9px] left-2 rounded bg-inset px-1.5 py-0.5 font-mono text-[9px] leading-none font-semibold tracking-[0.08em] uppercase" style={{ border: `1px solid ${tagColor}`, color: tagColor }}>
                    {tag}
                  </span>
                )}
                {st && !f && (
                  <span className="absolute top-[66px] left-0.5 font-mono text-[9.5px] leading-none whitespace-nowrap" style={{ color: (st.fail_pct ?? 0) >= 30 ? "#F2C14E" : "#5d6673" }}>
                    {st.runs} runs · {fmtMs(st.avg_ms)}
                    {n.kind === "verify" || n.kind === "audit" ? ` · ${Math.round(st.fail_pct ?? 0)}% fail` : ""}
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function fmtMs(ms: number | null) {
  if (ms == null) return "—";
  const s = Math.round(ms / 1000);
  return s >= 60 ? `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s` : `${s}s`;
}

function Inspector({
  w,
  selN,
  agentNames,
  nameOf,
  color,
  newKind,
  newTo,
  setNewKind,
  setNewTo,
  canDeleteWf,
  onDeleteWf,
  edit,
  setSel,
  onTrigger,
  onName,
}: {
  w: Workflow;
  selN: WorkflowNode | null;
  agentNames: string[];
  nameOf: (id: string) => string;
  color: (n: WorkflowNode) => string;
  newKind: WorkflowEdge["kind"];
  newTo: string;
  setNewKind: (k: WorkflowEdge["kind"]) => void;
  setNewTo: (s: string) => void;
  canDeleteWf: boolean;
  onDeleteWf: () => void;
  edit: (fn: (g: Workflow) => void, msg?: string, keepUndo?: boolean) => void;
  setSel: (id: string | null) => void;
  onTrigger: (t: Workflow["trigger"]) => void;
  onName: (n: string) => void;
}) {
  const lbl = "font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase";
  const set = <K extends keyof WorkflowNode>(k: K, v: WorkflowNode[K]) =>
    edit((g) => {
      const n = g.nodes.find((x) => x.id === selN?.id);
      if (n) n[k] = v;
    });
  const agentSelect = (k: "agent") => (
    <select value={selN?.[k] ?? ""} onChange={(e) => set(k, e.target.value || null)} className={inputCls}>
      <option value="">choose an agent…</option>
      {agentNames.map((a) => (
        <option key={a} value={a}>
          {a}
        </option>
      ))}
      {["claude", "codex", "gemini"].map((p) => (
        <option key={p} value={p}>
          {p} (provider, no agent file)
        </option>
      ))}
    </select>
  );
  return (
    <div className="flex min-w-0 flex-[1_1_280px] flex-col gap-3 rounded-xl border border-line bg-surface p-4">
      {!selN ? (
        <>
          <span className="font-mono text-[10px] leading-none font-medium tracking-[0.14em] text-muted uppercase">workflow</span>
          <input value={w.name} onChange={(e) => onName(e.target.value)} className="h-[34px] rounded-[7px] border border-line-2 bg-inset px-2.5 text-[18px] leading-[1.2] font-semibold text-text outline-none focus:border-accent" />
          <span className="text-[12.5px] leading-normal text-text-2">
            {w.nodes.filter((n) => n.kind === "agent").length} agent steps, {w.nodes.filter((n) => n.kind === "verify" || n.kind === "audit").length} gates, {w.nodes.filter((n) => n.kind === "hook").length} hooks. Runs for {triggerText(w)}.
          </span>
          <label className="flex flex-col gap-1.5">
            <span className={lbl}>runs for tasks mentioning (comma separated)</span>
            <input
              value={w.trigger.keywords.join(", ")}
              onChange={(e) => onTrigger({ ...w.trigger, keywords: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })}
              className={inputCls}
              placeholder="bug, fix"
            />
          </label>
          <label className="flex items-center gap-2 text-[12px] text-text-2">
            <input type="checkbox" checked={w.trigger.default} onChange={() => onTrigger({ ...w.trigger, default: !w.trigger.default })} />
            default for tasks no other workflow matches
          </label>
          <span className="font-mono text-[11px] leading-normal text-muted">Select a step to edit it. Steps added from the bar go after the selected step.</span>
          {canDeleteWf && (
            <button onClick={onDeleteWf} className={`${dangerCls} self-start`}>
              Delete workflow
            </button>
          )}
        </>
      ) : (
        <>
          <div className="flex items-center gap-2">
            <span className="font-mono text-[13px] leading-none font-semibold" style={{ color: color(selN) }}>
              {TYPE[selN.kind][0]}
            </span>
            <span className="font-mono text-[10px] leading-none font-medium tracking-[0.14em] text-muted uppercase">{TYPE[selN.kind][2]}</span>
          </div>
          <input value={selN.label} onChange={(e) => set("label", e.target.value)} className="h-[34px] rounded-[7px] border border-line-2 bg-inset px-2.5 text-[14px] font-semibold text-text outline-none focus:border-accent" />
          {selN.kind === "agent" && (
            <>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>agent</span>
                {agentSelect("agent")}
                <span className="font-mono text-[10.5px] leading-[1.4] text-muted">role, model and tools come from the agent</span>
              </label>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>extra prompt for this step</span>
                <textarea value={selN.prompt ?? ""} onChange={(e) => set("prompt", e.target.value || null)} rows={4} className="resize-y rounded-[7px] border border-line-2 bg-inset px-2.5 py-2 font-mono text-[12px] leading-[1.45] text-text outline-none" />
              </label>
            </>
          )}
          {selN.kind === "verify" && (
            <>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>command</span>
                <input value={selN.command ?? ""} onChange={(e) => set("command", e.target.value)} className={inputCls} />
                <span className="font-mono text-[10.5px] text-muted">exit 0 = pass</span>
              </label>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>max attempts</span>
                <input type="number" min={1} max={10} value={selN.max_attempts ?? 1} onChange={(e) => set("max_attempts", Math.max(1, Math.min(10, Number(e.target.value) || 1)))} className={inputCls} />
                <span className="font-mono text-[10.5px] leading-[1.4] text-muted">a fail goes back to the linked step until attempts run out</span>
              </label>
            </>
          )}
          {selN.kind === "audit" && (
            <>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>auditor</span>
                {agentSelect("agent")}
              </label>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>pass when output contains</span>
                <input value={selN.verdict ?? "VERDICT: PASS"} onChange={(e) => set("verdict", e.target.value)} className={inputCls} />
              </label>
            </>
          )}
          {selN.kind === "hook" && (
            <>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>when</span>
                <select value={selN.event ?? "post_done"} onChange={(e) => set("event", e.target.value)} className={inputCls}>
                  {["pre_launch", "post_verify", "post_done", "on_fail"].map((v) => (
                    <option key={v}>{v}</option>
                  ))}
                </select>
              </label>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>command</span>
                <input value={selN.command ?? ""} onChange={(e) => set("command", e.target.value)} className={inputCls} />
              </label>
              <label className="flex flex-col gap-1.5">
                <span className={lbl}>blocking</span>
                <select value={selN.blocking ? "yes" : "no"} onChange={(e) => set("blocking", e.target.value === "yes")} className={inputCls}>
                  <option value="yes">yes · a failure fails the task</option>
                  <option value="no">no · log and continue</option>
                </select>
              </label>
            </>
          )}
          {selN.kind === "end" && (
            <label className="flex flex-col gap-1.5">
              <span className={lbl}>outcome</span>
              <select value={selN.outcome ?? "done"} onChange={(e) => set("outcome", e.target.value as WorkflowNode["outcome"])} className={inputCls}>
                <option value="done">done</option>
                <option value="failed">failed · a person decides</option>
              </select>
            </label>
          )}
          <div className="h-px bg-line" />
          <span className={lbl}>goes to</span>
          {w.edges
            .filter((e) => e.from === selN.id)
            .map((e, i) => (
              <div key={i} className="flex items-center gap-2 font-mono text-[11.5px] leading-[1.2]">
                <span className="w-10" style={{ color: e.kind === "next" ? "var(--text-2)" : EDGE[e.kind] }}>
                  {e.kind}
                </span>
                <span className="min-w-0 flex-1 truncate">→ {nameOf(e.to)}</span>
                <button
                  onClick={() => edit((g) => void (g.edges = g.edges.filter((x) => !(x.from === e.from && x.to === e.to && x.kind === e.kind))))}
                  title="Remove connection"
                  className="size-[22px] cursor-pointer rounded-[5px] text-muted hover:bg-bad/10 hover:text-bad"
                >
                  ×
                </button>
              </div>
            ))}
          <div className="flex gap-1.5">
            <select value={newKind} onChange={(e) => setNewKind(e.target.value as WorkflowEdge["kind"])} className={`${inputCls} h-[30px] w-[76px] text-[11px]`}>
              <option value="next">next</option>
              <option value="pass">pass</option>
              <option value="fail">fail</option>
            </select>
            <select value={newTo} onChange={(e) => setNewTo(e.target.value)} className={`${inputCls} h-[30px] flex-1 text-[11px]`}>
              <option value="">choose step…</option>
              {w.nodes
                .filter((n) => n.id !== selN.id && n.kind !== "trigger")
                .map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.label}
                  </option>
                ))}
            </select>
            <button
              onClick={() => {
                if (!newTo) return;
                edit((g) => {
                  if (!g.edges.some((e) => e.from === selN.id && e.to === newTo && e.kind === newKind)) g.edges.push({ from: selN.id, to: newTo, kind: newKind });
                });
                setNewTo("");
              }}
              className={`${btnCls} h-[30px]`}
            >
              + Link
            </button>
          </div>
          {selN.kind !== "trigger" && (
            <button
              onClick={() => {
                const id = selN.id;
                edit(
                  (g) => {
                    const ins = g.edges.filter((e) => e.to === id);
                    const out = g.edges.find((e) => e.from === id && e.kind !== "fail");
                    g.nodes = g.nodes.filter((n) => n.id !== id);
                    g.edges = g.edges.filter((e) => e.from !== id && e.to !== id);
                    if (out) ins.forEach((e) => e.from !== out.to && g.edges.push({ from: e.from, to: out.to, kind: e.kind }));
                  },
                  `Deleted ${selN.label}`,
                  true,
                );
                setSel(null);
              }}
              className={`${dangerCls} self-start`}
            >
              Delete step
            </button>
          )}
        </>
      )}
    </div>
  );
}
