"use client";

/** The knowledge suggestion (backend knowledge.rs): once a project is big or busy enough, MAP suggests setting up
 * knowledge cards; accepting queues one task that follows MAP's knowledge practice. */

import { useState } from "react";

import { api } from "@/lib/api";
import type { KnowledgeOverview } from "@/lib/generated/KnowledgeOverview";
import type { KnowledgeStats } from "@/lib/generated/KnowledgeStats";

import { useApp } from "../app-context";
import { Btn, errText } from "../project-ui";
import { Card, SectionTitle } from "../shell/page";
import { fmtKB } from "./shared";

function statsLine(s: KnowledgeStats): [string, string][] {
  return [
    ["source files", s.source_files.toLocaleString()],
    ["lines", s.source_lines.toLocaleString()],
    ["done tasks", String(s.done_tasks)],
    ["docs", String(s.docs)],
    ["cards", String(s.cards)],
    ["rulebook", fmtKB(s.rulebook_bytes)],
  ];
}

/** The suggestion at the top of the Knowledge screen: a card when recommended, one line otherwise. `onChange` gets the
 * fresh overview after "Not now" or "Show", or `null` when it should be read again (after accepting). */
export function Suggestion({ slug, data, onChange }: { slug: string; data: KnowledgeOverview; onChange: (next: KnowledgeOverview | null) => void }) {
  const { toast, setProject, setScope, go } = useApp();
  const [busy, setBusy] = useState("");
  const [queued, setQueued] = useState<number | null>(null);
  const s = data.suggestion;

  const openTask = (id: number) => {
    setProject(slug);
    setScope("project");
    go("command", `tasks/${id}`);
  };
  const taskLink = (id: number, label = `task #${id}`) => (
    <a
      href={`#command/tasks/${id}`}
      onClick={(e) => {
        e.preventDefault();
        openTask(id);
      }}
      className="text-link hover:text-link-hover"
    >
      {label}
    </a>
  );

  const accept = async () => {
    setBusy("accept");
    try {
      const t = await api.acceptKnowledge(slug);
      setQueued(t.id);
      toast(`Task #${t.id} queued: set up knowledge`);
      onChange(null);
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy("");
    }
  };
  const dismiss = async (dismissed: boolean) => {
    setBusy("dismiss");
    try {
      onChange(await api.dismissKnowledge(slug, dismissed));
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy("");
    }
  };

  if (queued !== null && s.verdict !== "in_progress")
    return <p className="font-mono text-[12px] text-text-2">{taskLink(queued, `Task #${queued}`)} queued: it sets up the knowledge cards</p>;

  switch (s.verdict) {
    case "recommended":
      return (
        <Card className="flex flex-col gap-3.5 border-accent/45 bg-accent/[0.05] p-[18px]">
          <SectionTitle glyph="◆" note="suggested by MAP">
            Knowledge cards
          </SectionTitle>
          <h2 className="text-[18px] leading-tight font-semibold">This project is ready for knowledge cards</h2>
          {s.reasons.length > 0 && (
            <ul className="flex flex-col gap-1 text-[13px] leading-normal text-text-2">
              {s.reasons.map((r) => (
                <li key={r}>· {r}</li>
              ))}
            </ul>
          )}
          <dl className="flex flex-wrap gap-x-5 gap-y-1.5 font-mono text-[11.5px]">
            {statsLine(s.stats).map(([k, v]) => (
              <div key={k} className="flex items-baseline gap-1.5">
                <dt className="text-muted">{k}</dt>
                <dd className="text-text">{v}</dd>
              </div>
            ))}
          </dl>
          {s.plan.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <span className="font-mono text-[10.5px] font-medium tracking-[0.1em] text-muted uppercase">What happens if you accept</span>
              <ol className="flex list-decimal flex-col gap-1 pl-5 text-[13px] leading-normal text-text-2">
                {s.plan.map((step) => (
                  <li key={step}>{step}</li>
                ))}
              </ol>
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            <Btn tone="primary" onClick={() => void accept()} disabled={!!busy}>
              {busy === "accept" ? "Queuing…" : "Set up knowledge"}
            </Btn>
            <Btn onClick={() => void dismiss(true)} disabled={!!busy}>
              Not now
            </Btn>
          </div>
        </Card>
      );
    case "not_yet":
      return s.reasons.length ? <p className="font-mono text-[11.5px] text-muted">Knowledge cards: not needed yet · {s.reasons[0]}</p> : null;
    case "in_progress":
      return <p className="font-mono text-[12px] text-text-2">Setting up: {s.task !== null ? taskLink(s.task) : "a task is queued"}</p>;
    case "in_use":
      return null;
    case "dismissed":
      return (
        <p className="font-mono text-[11.5px] text-muted">
          Knowledge suggested earlier ·{" "}
          <button type="button" onClick={() => void dismiss(false)} disabled={!!busy} className="cursor-pointer text-link hover:text-link-hover">
            Show
          </button>
        </p>
      );
  }
}
