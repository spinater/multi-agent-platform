"use client";

/** A project's Knowledge settings (`settings.knowledge`): which folders are indexed, where the cards' index is, the
 * rulebooks and scratch folders, and the limits the health checks use. Saved with the other orchestrator settings. */

import { useState } from "react";

import type { KnowledgeSettings } from "@/lib/generated/KnowledgeSettings";

import { ListEditor, fmtBytes } from "../branches";
import { DEFAULT_KNOWLEDGE, Field, TextInput, Toggle } from "../project-ui";
import { fmtKB } from "./shared";

const MB = 1024 * 1024;

/** A whole, non-negative number field, in units of `scale`; an empty or invalid entry keeps the old value. */
function NumberField({ label, hint, value, onChange, scale = 1 }: { label: string; hint: string; value: number; onChange: (v: number) => void; scale?: number }) {
  // The text being typed, so the field can be cleared and retyped; back to the value on blur.
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <Field label={label} hint={hint}>
      <TextInput
        type="number"
        min={0}
        value={draft ?? String(Math.round(value / scale))}
        onChange={(e) => {
          setDraft(e.target.value);
          const v = Number(e.target.value);
          if (e.target.value.trim() !== "" && Number.isFinite(v) && v >= 0) onChange(Math.round(v * scale));
        }}
        onBlur={() => setDraft(null)}
      />
    </Field>
  );
}

export function KnowledgeEditor({ value, onChange }: { value: KnowledgeSettings; onChange: (k: KnowledgeSettings) => void }) {
  const set = (patch: Partial<KnowledgeSettings>) => onChange({ ...value, ...patch });
  const d = DEFAULT_KNOWLEDGE;
  return (
    <div className="flex flex-col gap-4">
      <Field label="RAG for agents" hint="On: task prompts tell agents to search this knowledge and read only the sections they need. Off: their searches and reads are refused.">
        <Toggle on={value.rag} onClick={() => set({ rag: !value.rag })} label="RAG for agents" />
      </Field>
      <ListEditor title="indexed folders" hint="Markdown and text files under these folders are searchable; rulebooks are added on their own." placeholder=".docs" items={value.dirs} onChange={(dirs) => set({ dirs })} />
      <Field label="Index" hint="The cards' index; the folder it is in holds the cards, each listing what it summarises under sources: in its frontmatter.">
        <TextInput value={value.index} onChange={(e) => set({ index: e.target.value })} placeholder={d.index} />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <NumberField label="Card line cap" hint={`Over it is an error (default ${d.card_line_cap}).`} value={value.card_line_cap} onChange={(card_line_cap) => set({ card_line_cap })} />
        <NumberField label="Card line warning" hint={`Over it is a warning (default ${d.card_line_warn}).`} value={value.card_line_warn} onChange={(card_line_warn) => set({ card_line_warn })} />
      </div>
      <ListEditor title="rulebooks" hint="Loaded into every agent request, so each has a byte budget." placeholder="CLAUDE.md" items={value.rulebooks} onChange={(rulebooks) => set({ rulebooks })} />
      <div className="grid gap-3 sm:grid-cols-2">
        <NumberField label="Rulebook cap (bytes)" hint={`${fmtKB(value.rulebook_byte_cap)} · default ${fmtKB(d.rulebook_byte_cap)}`} value={value.rulebook_byte_cap} onChange={(rulebook_byte_cap) => set({ rulebook_byte_cap })} />
        <NumberField label="Rulebook warning (bytes)" hint={`${fmtKB(value.rulebook_byte_warn)} · default ${fmtKB(d.rulebook_byte_warn)}`} value={value.rulebook_byte_warn} onChange={(rulebook_byte_warn) => set({ rulebook_byte_warn })} />
      </div>
      <ListEditor title="scratch folders" hint="Agent scratch space, checked for size and for copies of the repo inside it." placeholder=".scratch" items={value.scratch} onChange={(scratch) => set({ scratch })} />
      <NumberField label="Scratch warning (MB)" hint={`${fmtBytes(value.scratch_byte_warn)} · default ${fmtBytes(d.scratch_byte_warn)}`} value={value.scratch_byte_warn} scale={MB} onChange={(scratch_byte_warn) => set({ scratch_byte_warn })} />
      {(value.card_line_warn > value.card_line_cap || value.rulebook_byte_warn > value.rulebook_byte_cap) && (
        <p className="font-mono text-[11px] text-warn">A warning limit above its cap never fires before the error does.</p>
      )}
    </div>
  );
}
