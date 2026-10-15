"use client";

/** "Ask an agent to do it" (Configure tabs; backend `POST /api/configure/ask`): pick a provider, model and effort,
 * describe the change in plain words, and the agent proposes a checklist of changes (+ add, ~ change, − remove). The
 * checked ones are applied to the tab's unsaved draft; nothing is written until Save. The agent runs once, in a
 * scratch folder, with the tab's current config as context. */

import { useState } from "react";

import { configureApi, type AskReply, type ConfigOp } from "@/lib/api";
import { providerColor } from "@/lib/colors";
import type { JsonValue } from "@/lib/generated/serde_json/JsonValue";

import { useT } from "@/lib/i18n";

import { useApp } from "../app-context";
import { Seg, useConfigure } from "./ui";

export type AskTab = "agents" | "rules" | "skills" | "perms" | "hooks" | "verify" | "workflows";

const COPY: Record<AskTab, { ph: string; ex: string[] }> = {
  agents: { ph: "e.g. add a security auditor on claude that reads everything, never edits, and uses the security-review skill", ex: ["Add a security auditor", "Give the planner more effort", "Cheaper model for the tester"] },
  workflows: { ph: "e.g. write tests before verify and tell me when a task fails", ex: ["Write tests before verify", "Fail faster: lint first", "Skip the planner for small tasks"] },
  rules: { ph: "e.g. agents keep committing .env files and putting Thai text straight into components", ex: ["Never commit secrets", "Thai UI text goes in locale files", "Always write tests with a fix"] },
  skills: { ph: "e.g. turn off the skills nobody used this month", ex: ["Turn off skills nobody uses", "Security review for auth code", "Write changelog entries"] },
  perms: { ph: "e.g. let agents run tests without asking, but never run arbitrary shell or push", ex: ["Tests yes, push no", "Read-only for gemini", "No web access"] },
  hooks: { ph: "e.g. run the type check after verify and tell me when something gets stuck", ex: ["Type check before audit", "Notify on stall", "Pull before every session"] },
  verify: { ph: "e.g. tasks pass verify but break the build; make it stricter", ex: ["Add a build step", "Turn the audit on", "Use a stronger auditor"] },
};

const EFFORT_LABEL: Record<string, [string, string]> = {
  claude: ["thinking", "more thinking = better plans, more tokens"],
  codex: ["reasoning effort", "model_reasoning_effort"],
  gemini: ["thinking budget", "thinkingBudget per request"],
};
const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;
const OP_COLOR: Record<string, string> = { add: "var(--ok)", change: "var(--warn)", remove: "var(--bad)" };
const OP_GLYPH: Record<string, string> = { add: "+", change: "~", remove: "−" };

export function AskAgent({
  tab,
  context,
  onApply,
  workflowId,
  allowRuns,
}: {
  tab: AskTab;
  /** The tab's current config, sent to the agent. */
  context: unknown;
  /** Applies the checked ops to the tab's draft; returns how many it could apply. Without it the panel only shows the
   * proposal. */
  onApply?: (ops: ConfigOp[]) => number;
  workflowId?: string;
  /** Workflows: an empty prompt asks for suggestions from the last 30 runs. */
  allowRuns?: boolean;
}) {
  const { slug, cfgLang, flash } = useConfigure();
  const { providers } = useApp();
  const usable = providers.filter((p) => ["claude", "codex", "gemini"].includes(p.name));
  const [prov, setProv] = useState(() => usable.find((p) => p.available && p.signed_in !== false)?.name ?? "claude");
  const info = providers.find((p) => p.name === prov);
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState<(typeof EFFORTS)[number]>("medium");
  const [prompt, setPrompt] = useState("");
  const [phase, setPhase] = useState<"idle" | "thinking" | "ready">("idle");
  const [reply, setReply] = useState<AskReply | null>(null);
  const [on, setOn] = useState<Record<number, boolean>>({});
  const [error, setError] = useState("");
  const copy = COPY[tab];
  const t = useT();
  const fromRuns = !!allowRuns && !prompt.trim();
  const m = model ?? info?.default_model ?? "";

  const run = async () => {
    if (phase === "thinking") return;
    if (!prompt.trim() && !allowRuns) {
      flash("Type what you want first", "warn");
      return;
    }
    setPhase("thinking");
    setError("");
    try {
      const r = await configureApi.ask({ tab, project: slug, provider: prov, model: m || null, effort, prompt: prompt.trim(), language: cfgLang, context: (context ?? null) as JsonValue, workflow_id: workflowId ?? null, from_runs: fromRuns });
      setReply(r);
      setOn(Object.fromEntries(r.ops.map((_, i) => [i, true])));
      setPhase("ready");
    } catch (e) {
      setError((e as Error).message);
      setPhase("idle");
    }
  };
  const chosen = reply ? reply.ops.filter((_, i) => on[i]) : [];
  const apply = () => {
    if (!onApply || !chosen.length) return;
    const n = onApply(chosen);
    flash(`Applied ${n} change${n === 1 ? "" : "s"} from ${prov} · unsaved`);
    setPhase("idle");
    setReply(null);
    setPrompt("");
  };
  const [effLabel, effNote] = EFFORT_LABEL[prov] ?? ["effort", ""];
  const thaiPrompt = /[฀-๿]/.test(prompt);
  const outLang = cfgLang === "th" ? t("ai.outTh") : thaiPrompt ? t("ai.outEnTh") : t("ai.outEn");

  return (
    <div className="flex flex-col gap-3 rounded-xl border p-3.5" style={{ borderColor: `color-mix(in srgb, var(--accent) ${phase === "ready" ? 40 : 20}%, transparent)`, background: "color-mix(in srgb, var(--accent) 3.5%, transparent)" }}>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <span className="flex items-center gap-2 text-[13px] leading-none font-semibold">
          <span className="text-accent">✦</span>
          {t("ai.title")}
        </span>
        <span className="text-[11.5px] leading-[1.3] text-muted">{t("ai.sub")}</span>
        <div className="ml-auto flex gap-0.5 rounded-[7px] border border-line-2 p-0.5">
          {usable.map((p) => (
            <button
              key={p.name}
              onClick={() => {
                setProv(p.name);
                setModel(null);
              }}
              title={!p.available ? "not installed" : p.signed_in === false ? "signed out" : undefined}
              className="flex h-6 cursor-pointer items-center gap-1.5 rounded-[5px] px-2 font-mono text-[11px] font-medium whitespace-nowrap"
              style={{ background: prov === p.name ? "color-mix(in srgb, var(--text) 8%, transparent)" : "transparent", color: prov === p.name ? "var(--text)" : "var(--muted)", opacity: p.available ? 1 : 0.5 }}
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
              className="h-6 cursor-pointer rounded-[5px] border px-2 font-mono text-[11px] font-medium whitespace-nowrap"
              style={{ borderColor: x === m ? "var(--accent)" : "var(--line-2)", background: x === m ? "var(--accent-tint)" : "transparent", color: x === m ? "var(--accent-soft)" : "var(--text-2)" }}
            >
              {x}
            </button>
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="font-mono text-[9.5px] leading-none font-medium tracking-[0.12em] text-muted uppercase">{effLabel}</span>
          <Seg value={effort} options={[...EFFORTS]} onChange={setEffort} />
        </div>
        <span className="font-mono text-[10.5px] leading-[1.3] text-muted">{effNote}</span>
      </div>
      <div className="flex flex-wrap items-stretch gap-2">
        <textarea
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) run();
          }}
          rows={2}
          placeholder={copy.ph}
          className="min-w-0 flex-[1_1_360px] resize-y rounded-lg border border-line-2 bg-inset px-3 py-2.5 text-[13px] leading-normal text-text outline-none focus:border-accent"
        />
        <button
          onClick={run}
          className="min-h-11 flex-none cursor-pointer rounded-lg border border-accent px-4 text-[12.5px] font-semibold whitespace-nowrap"
          style={{ background: phase === "thinking" ? "var(--accent-tint)" : "var(--accent)", color: phase === "thinking" ? "var(--accent-soft)" : "var(--accent-text)" }}
        >
          {phase === "thinking" ? `${prov} · ${m} · ${effort}${fromRuns ? " · reading the last 30 runs" : ""}…` : fromRuns ? "✦ Suggest from runs" : t("ai.generate")}
        </button>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {copy.ex.map((x) => (
          <button key={x} onClick={() => setPrompt(x)} className="h-[26px] cursor-pointer rounded-[13px] border border-line-2 px-2.5 text-[11.5px] whitespace-nowrap text-text-2 hover:border-accent/50 hover:text-accent-soft">
            {x}
          </button>
        ))}
      </div>
      <span className="font-mono text-[10.5px] leading-[1.4]" style={{ color: cfgLang === "th" ? "var(--warn)" : "var(--muted)" }}>
        {outLang}
      </span>
      {error && <span className="font-mono text-[11px] leading-[1.4] text-bad">{error}</span>}
      {phase === "ready" && reply && (
        <div className="flex flex-col gap-2 border-t border-line pt-3">
          <span className="flex items-center gap-2 text-[12.5px] leading-[1.4] font-medium">
            <span className="size-[7px] rounded-full" style={{ background: providerColor(reply.provider) }} />
            {reply.provider} · {reply.model} · {effort} proposes {reply.ops.length} change{reply.ops.length === 1 ? "" : "s"}
            {prompt.trim() ? ` for “${prompt.trim().slice(0, 80)}”` : " from the last runs"}
          </span>
          <span className="text-[12px] leading-normal text-text-2">{reply.summary}</span>
          {reply.evidence.map((e, i) => (
            <span key={i} className="font-mono text-[11px] leading-[1.4] text-muted">
              · {e}
            </span>
          ))}
          {reply.ops.map((o, i) => {
            const c = !!on[i];
            return (
              <button
                key={i}
                onClick={() => setOn({ ...on, [i]: !c })}
                className="flex cursor-pointer items-center gap-2.5 rounded-lg border px-2.5 py-2 text-left font-mono text-[12px] leading-[1.4]"
                style={{ borderColor: c ? "var(--line-2)" : "var(--line)", background: c ? "color-mix(in srgb, var(--text) 3%, transparent)" : "transparent" }}
              >
                <span className="flex size-4 flex-none items-center justify-center rounded border text-[11px] font-bold text-accent-text" style={{ borderColor: c ? "var(--accent)" : "var(--line-3)", background: c ? "var(--accent)" : "transparent" }}>
                  {c ? "✓" : ""}
                </span>
                <span className="w-3.5 flex-none font-bold" style={{ color: OP_COLOR[o.op] ?? "var(--text-2)" }}>
                  {OP_GLYPH[o.op] ?? "·"}
                </span>
                <span className="min-w-0 flex-1" style={{ color: c ? "var(--text)" : "var(--muted)" }}>
                  {o.text}
                </span>
              </button>
            );
          })}
          <div className="flex flex-wrap gap-2">
            {onApply && (
              <button onClick={apply} disabled={!chosen.length} className="h-8 cursor-pointer rounded-[7px] border border-accent bg-accent px-3.5 text-[12.5px] font-semibold whitespace-nowrap text-accent-text disabled:opacity-50">
                {t("ai.apply", { n: chosen.length })}
              </button>
            )}
            <button onClick={() => setPhase("idle")} className="h-8 cursor-pointer rounded-[7px] border border-line-3 px-3.5 text-[12.5px] font-medium text-nav">
              Discard
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
