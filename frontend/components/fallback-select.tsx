"use client";

import type { ProviderInfo } from "@/lib/generated/ProviderInfo";

import { useApp } from "./app-context";

/** A `panel:` fallback (`panel:gemini/gemini-4-argon-high,claude/opus`): its members answer, and argue when they differ. */
const PANEL = "panel:";

export function FallbackSelect({ value, onChange, providers }: { value: string; onChange: (v: string) => void; providers: ProviderInfo[] }) {
  const { server } = useApp();
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)} className="h-[38px] rounded-[7px] border border-line-2 bg-inset px-2 font-mono text-[13px] tracking-normal text-text normal-case">
      <option value="api">{server?.api_fallback ?? "Anthropic API"}</option>
      {providers.map((p) => (
        <option key={p.name} value={p.name} disabled={!p.available}>
          {p.name} (headless){p.available ? "" : " · not installed"}
        </option>
      ))}
      {value.startsWith(PANEL) && <option value={value}>Panel, they debate: {value.slice(PANEL.length).replaceAll(",", " + ")}</option>}
      <option value="none">None: abstain stays unanswered</option>
    </select>
  );
}
