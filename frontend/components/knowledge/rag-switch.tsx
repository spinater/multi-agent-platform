"use client";

/** The project's RAG switch (`settings.knowledge.rag`, backend `rag.rs`): on, every task's prompt tells the agent to
 * search this knowledge first and read only the sections it needs; off, agents' searches and reads are refused. */

import { useState } from "react";

import { api } from "@/lib/api";
import type { KnowledgeOverview } from "@/lib/generated/KnowledgeOverview";

import { useApp } from "../app-context";
import { Toggle, errText } from "../project-ui";
import { Box } from "./sections";

export function RagSwitch({ slug, data, onChange }: { slug: string; data: KnowledgeOverview; onChange: () => void }) {
  const { projects, toast } = useApp();
  const [busy, setBusy] = useState(false);
  const on = data.settings.rag;
  const flip = async () => {
    const info = projects.find((p) => p.project.slug === slug);
    if (!info) return;
    setBusy(true);
    try {
      const s = info.project.settings;
      await api.updateProject(slug, { settings: { ...s, knowledge: { ...s.knowledge, rag: !on } } });
      toast(`RAG ${on ? "off" : "on"} for ${info.project.name}`);
      onChange();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy(false);
    }
  };
  return (
    <Box
      title="RAG for agents"
      glyph="⌕"
      note={on ? "on" : "off"}
      actions={<Toggle on={on} onClick={flip} disabled={busy} label="RAG for agents" />}
    >
      <p className="text-[12.5px] leading-snug text-text-2">
        {on
          ? "Every task's prompt tells the agent to search this knowledge before it starts and to read only the sections it needs: a long document comes back as its outline, one section at a time. What they take, and its tokens, shows under Agent searches and reads."
          : "Off: task prompts say nothing of the knowledge, and agents' searches and reads are refused, so they work from the repository alone. You can still search here."}
      </p>
    </Box>
  );
}
