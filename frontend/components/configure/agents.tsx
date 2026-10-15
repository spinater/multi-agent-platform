"use client";

/** Configure › Agents (backend agents.rs, `AgentDef`): the relation map, then the agents as cards (layout A) or a
 * table (B) with the editor beside or below. One definition, one file per target CLI: Claude Code and agy read
 * Markdown, Codex reads TOML. Machine scope is AgentScope `user` (the CLIs' home folders), project scope the
 * project's folders; a project agent with a machine agent's name is an override. Edits are drafts until Save. */

import { useEffect, useState } from "react";

import { api, type ConfigOp } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { AgentDef } from "@/lib/generated/AgentDef";
import type { AgentDetail } from "@/lib/generated/AgentDetail";
import type { AgentProvider } from "@/lib/generated/AgentProvider";
import type { AgentScope } from "@/lib/generated/AgentScope";
import type { AgentsOverview } from "@/lib/generated/AgentsOverview";
import type { Capability } from "@/lib/generated/Capability";
import type { ConfigSource } from "@/lib/generated/ConfigSource";
import type { Effort } from "@/lib/generated/Effort";
import type { SkillsOverview } from "@/lib/generated/SkillsOverview";

import { AGENT_PROVIDERS, CAPABILITIES, CLAUDE_MODELS, EFFORTS, GEMINI_MODELS, PROVIDER_FORMAT, PROVIDER_LABEL, blankDef } from "../agents/shared";
import { useApp } from "../app-context";
import { AskAgent } from "./ask-agent";
import { RelationMap, type RelItem } from "./relation-map";
import { Box, Chip, Lbl, Note, Pick, Seg, SourceBadge, Switch, TabHead, btnCls, dangerCls, inputCls, same, useConfigure } from "./ui";
import { useMcpServers } from "./mcp";

/** An agent in the editor: its definition, where it lives, and what it was when loaded (`null`: new). */
type Item = { key: string; scope: AgentScope; def: AgentDef; base: AgentDef | null; baseName: string | null; deleted?: boolean };

const keyOf = (scope: AgentScope, name: string) => `${scope}/${name}`;
let seq = 0;

export function AgentsTab() {
  const { scope, slug, setPending, layout } = useConfigure();
  const { providers, project } = useApp();
  const target = scope === "project" ? slug : project;
  const [overview, setOverview] = useState<AgentsOverview | null>(null);
  const [details, setDetails] = useState<Record<string, AgentDetail>>({});
  const [skills, setSkills] = useState<SkillsOverview | null>(null);
  const [items, setItems] = useState<Item[] | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [tick, setTick] = useState(0);
  const mcp = useMcpServers(scope === "project" ? slug : null);

  useEffect(() => {
    let live = true;
    (async () => {
      try {
        const o = await api.agents(target);
        const rows = o.rows.filter((r) => scope === "project" || r.scope === "user");
        const ds = await Promise.all(rows.map((r) => api.agent(target, r.scope, r.name).catch(() => null)));
        if (!live) return;
        const map: Record<string, AgentDetail> = {};
        ds.forEach((d) => d && (map[keyOf(d.scope, d.def.name)] = d));
        setOverview(o);
        setDetails(map);
        setItems(Object.values(map).map((d) => ({ key: keyOf(d.scope, d.def.name), scope: d.scope, def: d.def, base: d.def, baseName: d.def.name })));
        setError("");
      } catch (e) {
        if (live) setError((e as Error).message);
      }
    })();
    api.skills(target).then((s) => live && setSkills(s), () => {});
    return () => {
      live = false;
    };
  }, [target, scope, tick]);

  const visible = (items ?? []).filter((i) => !i.deleted && (scope === "project" || i.scope === "user"));
  const names = new Set(visible.filter((i) => i.scope === "user").map((i) => i.def.name));
  const sourceOf = (i: Item): ConfigSource => (i.scope === "user" ? "machine" : names.has(i.def.name) ? "override" : "project");
  const current = visible.find((i) => i.key === sel) ?? visible[0] ?? null;

  const dirty = (items ?? []).filter((i) => i.deleted || !i.base || !same(i.def, i.base));
  useEffect(() => {
    setPending(
      "agents",
      dirty.length
        ? {
            count: dirty.length,
            files: [...new Set(dirty.map((i) => (i.scope === "user" ? "~/.claude/agents · ~/.codex/agents · ~/.gemini/config/agents" : ".claude/agents · .codex/agents · .agents/agents")))],
            save: async () => {
              for (const i of dirty) {
                if (i.deleted) {
                  if (i.baseName) await api.deleteAgent(target, i.scope, i.baseName);
                  continue;
                }
                await api.saveAgent(target, i.scope, i.def);
                if (i.baseName && i.baseName !== i.def.name) await api.deleteAgent(target, i.scope, i.baseName);
              }
              setTick((t) => t + 1);
            },
            discard: () => setTick((t) => t + 1),
          }
        : null,
    );
  }, [JSON.stringify(dirty.map((d) => [d.key, d.deleted, d.def])), target, setPending]); // eslint-disable-line react-hooks/exhaustive-deps

  const update = (key: string, patch: Partial<AgentDef>) => setItems((xs) => (xs ?? []).map((x) => (x.key === key ? { ...x, def: { ...x.def, ...patch } } : x)));
  const add = (def: AgentDef, sc: AgentScope = scope === "machine" ? "user" : "project") => {
    const key = `new:${++seq}`;
    setItems((xs) => [...(xs ?? []), { key, scope: sc, def, base: null, baseName: null }]);
    setSel(key);
  };

  // The relation map's outer ring: enabled skills, context files, MCP servers, capabilities.
  const rel: RelItem[] = [
      ...(skills?.skills ?? []).map((s) => ({ id: `skill:${s.id}`, type: "skill" as const, label: s.name, off: !s.enabled })),
      ...(skills?.contexts ?? []).filter((c) => c.exists).map((c) => ({ id: `rule:${c.provider}:${c.file}`, type: "rule" as const, label: c.file, off: false })),
      ...(mcp.servers ?? []).map((m) => ({ id: `mcp:${m.name}`, type: "mcp" as const, label: m.name, off: !m.ok })),
      ...CAPABILITIES.map((c) => ({ id: `tool:${c.id}`, type: "tool" as const, label: c.id, off: false })),
  ];
  const byId = new Map(visible.map((i) => [i.key, i]));
  const binds = (agent: string, r: RelItem) => {
      const a = byId.get(agent);
      if (!a) return false;
      const t = a.def.targets as string[];
      if (r.type === "tool") return a.def.tools === null || a.def.tools.includes(r.id.slice(5) as Capability);
      if (r.type === "skill") return !!skills?.skills.find((s) => `skill:${s.id}` === r.id)?.loads_in.some((l) => t.includes(l.provider === "agy" ? "gemini" : l.provider));
      if (r.type === "rule") return t.includes(r.id.split(":")[1] === "agy" ? "gemini" : r.id.split(":")[1]);
      return !!mcp.servers?.find((m) => `mcp:${m.name}` === r.id)?.providers.some((p) => t.includes(p));
  };

  const applyOps = (ops: ConfigOp[]) => {
    let n = 0;
    for (const o of ops) {
      const name = o.target.replace(/^agent:/, "");
      const v = (o.value ?? {}) as Partial<AgentDef>;
      const it = visible.find((i) => i.def.name === name);
      if (o.op === "add" && !it) {
        add({ ...blankDef(), ...v, name: v.name ?? name });
        n++;
      } else if (o.op === "change" && it) {
        update(it.key, v);
        n++;
      } else if (o.op === "remove" && it) {
        setItems((xs) => (xs ?? []).map((x) => (x.key === it.key ? { ...x, deleted: true } : x)));
        n++;
      }
    }
    return n;
  };

  const rows = visible.map((i) => {
    const d = details[keyOf(i.scope, i.baseName ?? "")];
    const primary = (i.def.targets[0] ?? "claude") as AgentProvider;
    const model = i.def.models[primary] ?? "inherit";
    return { i, d, primary, model, src: sourceOf(i) };
  });

  return (
    <>
      <TabHead
        title="Agents"
        desc="A provider and model with a role, the skills, rules, tools and MCP servers it gets, and how many can run at once."
        savesTo={scope === "machine" ? "each CLI’s home: ~/.claude/agents, ~/.codex/agents, ~/.gemini/config/agents" : ".claude/agents/<name>.md · .codex/agents/<name>.toml · .agents/agents/<name>.md"}
        add={{ label: "+ New agent", onClick: () => add({ ...blankDef(), name: "new-agent" }) }}
      />
      <AskAgent tab="agents" context={{ agents: visible.map((i) => ({ ...i.def, source: sourceOf(i) })), skills: skills?.skills.map((s) => ({ id: s.id, enabled: s.enabled })), providers: providers.map((p) => p.name) }} onApply={applyOps} />
      {error && <Note tone="bad">Could not read the agents: {error}</Note>}
      {!items && !error && <Note>Reading the agent folders…</Note>}
      {items && (
        <>
          <RelationMap agents={visible.map((i) => ({ id: i.key, name: i.def.name, color: providerColor(i.def.targets[0]) }))} items={rel} binds={binds} selected={current?.key ?? null} onSelect={setSel} />
          {visible.length === 0 && <Note>No agents yet. + New agent writes one file per CLI you pick; each CLI then finds it by name.</Note>}
          <div className="flex flex-wrap items-start gap-3.5">
            {layout === "A" ? (
              <div className="flex max-w-[340px] flex-[1_1_260px] flex-col gap-2">
                {rows.map(({ i, primary, model, src }) => (
                  <button
                    key={i.key}
                    onClick={() => setSel(i.key)}
                    className="flex cursor-pointer flex-col gap-2 rounded-[10px] border p-3 text-left text-text"
                    style={{ borderColor: current?.key === i.key ? "var(--accent)" : "var(--line)", background: current?.key === i.key ? "color-mix(in srgb, var(--accent) 6%, transparent)" : "var(--surface)" }}
                  >
                    <span className="flex w-full items-center gap-2">
                      <span className="size-2 rounded-full" style={{ background: providerColor(primary) }} />
                      <span className="text-[14px] leading-none font-semibold">{i.def.name || "unnamed"}</span>
                      {(!i.base || !same(i.def, i.base)) && <span className="size-1.5 rounded-full bg-accent" title="unsaved" />}
                      <span className="ml-auto">
                        <SourceBadge source={src} />
                      </span>
                    </span>
                    <span className="font-mono text-[11px] leading-[1.2] text-text-2">
                      {i.def.targets.map((t) => PROVIDER_LABEL[t]).join(" · ")} / {model}
                    </span>
                    <span className="font-mono text-[10.5px] leading-[1.3] text-muted">
                      tools {i.def.tools === null ? "all" : i.def.tools.join(", ") || "none"} · effort {i.def.effort ?? "inherit"}
                    </span>
                  </button>
                ))}
              </div>
            ) : (
              <div className="w-full flex-[1_1_100%] overflow-auto rounded-[10px] border border-line">
                <div className="flex min-w-[760px] flex-col">
                  <div className="grid grid-cols-[150px_170px_1fr_120px_80px_90px] gap-2.5 border-b border-line px-3 py-2.5 font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase">
                    <span>agent</span>
                    <span>provider / model</span>
                    <span>description</span>
                    <span>tools</span>
                    <span>effort</span>
                    <span>source</span>
                  </div>
                  {rows.map(({ i, primary, model, src }) => (
                    <button
                      key={i.key}
                      onClick={() => setSel(i.key)}
                      className="grid cursor-pointer grid-cols-[150px_170px_1fr_120px_80px_90px] items-center gap-2.5 border-b border-line px-3 py-[11px] text-left font-mono text-[11.5px] leading-[1.2] text-text last:border-b-0 hover:bg-white/[.03]"
                      style={{ background: current?.key === i.key ? "color-mix(in srgb, var(--accent) 6%, transparent)" : undefined }}
                    >
                      <span className="flex items-center gap-[7px] font-sans text-[13px] font-semibold">
                        <span className="size-[7px] rounded-full" style={{ background: providerColor(primary) }} />
                        {i.def.name || "unnamed"}
                      </span>
                      <span className="text-text-2">
                        {primary} / {model}
                      </span>
                      <span className="truncate text-muted">{i.def.description}</span>
                      <span>{i.def.tools === null ? "all" : i.def.tools.length}</span>
                      <span>{i.def.effort ?? "inherit"}</span>
                      <span>
                        <SourceBadge source={src} />
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            )}
            {current && (
              <Editor
                key={current.key}
                item={current}
                source={sourceOf(current)}
                overview={overview}
                codexModels={providers.find((p) => p.name === "codex")?.models ?? []}
                onChange={(p) => update(current.key, p)}
                onDuplicate={() => add({ ...current.def, name: `${current.def.name}-copy` })}
                onDelete={() => setItems((xs) => (xs ?? []).map((x) => (x.key === current.key ? { ...x, deleted: true } : x)))}
                onUseMachine={() => setItems((xs) => (xs ?? []).map((x) => (x.key === current.key ? { ...x, deleted: true } : x)))}
                skills={skills}
                mcpNames={(mcp.servers ?? []).filter((m) => m.providers.some((p) => (current.def.targets as string[]).includes(p))).map((m) => m.name)}
              />
            )}
          </div>
        </>
      )}
    </>
  );
}

function Editor({
  item,
  source,
  overview,
  codexModels,
  onChange,
  onDuplicate,
  onDelete,
  onUseMachine,
  skills,
  mcpNames,
}: {
  item: Item;
  source: ConfigSource;
  overview: AgentsOverview | null;
  codexModels: string[];
  onChange: (p: Partial<AgentDef>) => void;
  onDuplicate: () => void;
  onDelete: () => void;
  onUseMachine: () => void;
  skills: SkillsOverview | null;
  mcpNames: string[];
}) {
  const d = item.def;
  const models: Record<AgentProvider, string[]> = { claude: CLAUDE_MODELS, codex: ["inherit", ...codexModels], gemini: GEMINI_MODELS };
  const fileOf = (p: AgentProvider) => {
    const dir = overview?.dirs.find((x) => x.provider === p && x.scope === item.scope);
    return dir ? `${dir.path}/${d.name || "<name>"}.${p === "codex" ? "toml" : "md"}` : `${d.name}.${p === "codex" ? "toml" : "md"}`;
  };
  const toggleTarget = (p: AgentProvider) => {
    const has = d.targets.includes(p);
    const next = AGENT_PROVIDERS.filter((x) => (x === p ? !has : d.targets.includes(x)));
    if (!next.length) return;
    onChange({ targets: next });
  };
  const loads = (skills?.skills ?? []).filter((s) => s.enabled && s.loads_in.some((l) => (d.targets as string[]).includes(l.provider === "agy" ? "gemini" : l.provider)));
  const rules = (skills?.contexts ?? []).filter((c) => c.exists && (d.targets as string[]).includes(c.provider === "agy" ? "gemini" : c.provider));
  return (
    <div className="flex min-w-0 flex-[999_1_480px] flex-col gap-4 rounded-xl border border-line bg-surface p-[18px]">
      <div className="flex flex-wrap items-center gap-2.5">
        <input
          value={d.name}
          onChange={(e) => onChange({ name: e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, "-") })}
          className="h-9 min-w-0 flex-[1_1_200px] rounded-[7px] border border-line-2 bg-inset px-2.5 text-[16px] leading-none font-semibold text-text outline-none focus:border-accent"
          aria-label="Agent name"
        />
        <SourceBadge source={source} />
        <button onClick={onDuplicate} className={btnCls}>
          Duplicate
        </button>
        {source === "override" && (
          <button onClick={onUseMachine} className="h-[30px] cursor-pointer rounded-md border px-3 text-[12px] font-medium text-warn" style={{ borderColor: "color-mix(in srgb, var(--warn) 40%, transparent)" }} title="Delete the project file so the machine agent applies">
            Use machine value
          </button>
        )}
        <button onClick={onDelete} className={dangerCls}>
          Delete
        </button>
      </div>
      <span className="font-mono text-[11px] leading-[1.4] text-muted">{item.base ? `in ${item.scope === "user" ? "the CLIs' home folders" : "the project folder"}` : "new · written on Save"}</span>
      <label className="flex flex-col gap-1.5">
        <Lbl>description · when to use this agent</Lbl>
        <input value={d.description} onChange={(e) => onChange({ description: e.target.value })} className={`${inputCls} h-[34px] font-sans text-[12.5px]`} />
      </label>
      <div className="flex flex-col gap-2">
        <Lbl>targets · one definition, a file for each CLI</Lbl>
        {AGENT_PROVIDERS.map((p) => {
          const on = d.targets.includes(p);
          return (
            <div key={p} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-line px-2.5 py-[9px]" style={{ opacity: on ? 1 : 0.5 }}>
              <Switch on={on} onClick={() => toggleTarget(p)} />
              <span className="flex w-[118px] items-center gap-[7px] text-[12.5px] leading-none font-semibold">
                <span className="size-[7px] rounded-full" style={{ background: providerColor(p) }} />
                {PROVIDER_LABEL[p]}
              </span>
              <Pick value={d.models[p] ?? "inherit"} options={models[p]} onChange={(m) => onChange({ models: { ...d.models, [p]: m === "inherit" ? null : m } })} />
              <span className="ml-auto font-mono text-[10.5px] leading-[1.3] [overflow-wrap:anywhere] text-muted">{on ? `${fileOf(p)} · ${PROVIDER_FORMAT[p]}` : `not written for ${PROVIDER_LABEL[p]}`}</span>
            </div>
          );
        })}
      </div>
      <label className="flex flex-col gap-1.5">
        <Lbl>instructions · system prompt</Lbl>
        <textarea
          value={d.instructions}
          onChange={(e) => onChange({ instructions: e.target.value })}
          rows={5}
          className="resize-y rounded-[7px] border border-line-2 bg-inset p-2.5 font-mono text-[12.5px] leading-normal text-text outline-none focus:border-accent"
        />
      </label>
      <div className="flex flex-wrap gap-x-6 gap-y-4">
        <div className="flex flex-col gap-[7px]">
          <Lbl>tools</Lbl>
          <div className="flex flex-wrap gap-1">
            <Chip on={d.tools === null} title="CLI default: every tool the CLI has" onClick={() => onChange({ tools: d.tools === null ? ["read", "search"] : null })}>
              all (inherit)
            </Chip>
            {CAPABILITIES.map((c) => (
              <Chip
                key={c.id}
                on={d.tools !== null && d.tools.includes(c.id)}
                title={c.note}
                onClick={() => onChange({ tools: d.tools === null ? [c.id] : d.tools.includes(c.id) ? d.tools.filter((x) => x !== c.id) : [...d.tools, c.id] })}
              >
                {c.id}
              </Chip>
            ))}
          </div>
        </div>
        <div className="flex flex-col gap-[7px]">
          <Lbl>effort</Lbl>
          <Seg value={(d.effort ?? "inherit") as Effort | "inherit"} options={["inherit", ...EFFORTS]} onChange={(e) => onChange({ effort: e === "inherit" ? null : (e as Effort) })} />
        </div>
      </div>
      <Box dashed className="gap-3.5">
        <span className="flex flex-wrap items-baseline gap-x-2.5 gap-y-1.5">
          <span className="text-[12px] leading-none font-semibold">What it gets</span>
          <span className="font-mono text-[10.5px] leading-[1.3] text-muted">from its target CLIs: enabled skills they load, the rule files they read, the MCP servers they have</span>
        </span>
        <Group label="skills · only enabled ones load" items={loads.map((s) => s.name)} />
        <Group label="rules" items={rules.map((c) => `${c.file} (${c.provider})`)} />
        <Group label="mcp servers" items={mcpNames} />
        <span className="font-mono text-[10.5px] leading-[1.4] text-muted">fallback provider, max parallel sessions and a token budget per agent are proposed in the design and not in AgentDef yet.</span>
      </Box>
    </div>
  );
}

function Group({ label, items }: { label: string; items: string[] }) {
  return (
    <div className="flex flex-col gap-[7px]">
      <Lbl>{label}</Lbl>
      <div className="flex flex-wrap gap-1.5">
        {items.length ? items.map((x) => <Chip key={x} on disabled>{x}</Chip>) : <span className="font-mono text-[11px] text-muted">none</span>}
      </div>
    </div>
  );
}
