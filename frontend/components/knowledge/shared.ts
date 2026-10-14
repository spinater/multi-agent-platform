/** Labels, tones and path helpers shared by the Knowledge screen's sections and doc viewer. */

import type { KnowledgeCheck } from "@/lib/generated/KnowledgeCheck";
import type { KnowledgeKind } from "@/lib/generated/KnowledgeKind";
import type { KnowledgeSettings } from "@/lib/generated/KnowledgeSettings";
import type { Level } from "@/lib/generated/Level";
import { formatHash } from "@/lib/nav";

import type { Tone } from "../ui";

export const KINDS: KnowledgeKind[] = ["card", "requirement", "task", "rulebook", "doc", "memory"];

export const KIND_LABEL: Record<KnowledgeKind, string> = {
  card: "cards",
  requirement: "requirements",
  task: "task sheets",
  rulebook: "rulebooks",
  doc: "docs",
  memory: "MAP memory",
};

export const KIND_TONE: Record<KnowledgeKind, Tone> = { card: "accent", requirement: "info", task: "ok", rulebook: "warn", doc: "idle", memory: "info" };

export const LEVEL_RANK: Record<Level, number> = { error: 0, warning: 1, info: 2, success: 3 };

/** What each health check means, in one line, for the owner reading the list. */
export function checkText(check: KnowledgeCheck, s: KnowledgeSettings): { label: string; why: string } {
  switch (check) {
    case "no_index":
      return { label: "No index", why: `Agents find cards through ${s.index}; without it they grep the whole tree and read whatever matches first.` };
    case "stale":
      return { label: "Stale card", why: "A file the card summarises changed in a commit after the card was last updated, so it may describe old code." };
    case "missing_source":
      return { label: "Missing source", why: "The card's sources: list names a file that no longer exists (moved, renamed or deleted)." };
    case "over_cap":
      return { label: "Over the line cap", why: `Cards over ${s.card_line_cap} lines get skimmed or cut off; split them into focused cards.` };
    case "near_cap":
      return { label: "Near the line cap", why: `Over ${s.card_line_warn} lines; the next addition may push it past ${s.card_line_cap}.` };
    case "not_in_index":
      return { label: "Not in the index", why: "The index does not mention this card, so an agent that starts from the index never finds it." };
    case "broken_link":
      return { label: "Broken link", why: "A relative Markdown link points at a file that does not exist." };
    case "rulebook_size":
      return { label: "Rulebook size", why: "Rulebooks load into every agent request, so every byte is paid on every turn; move detail into cards." };
    case "scratch_size":
      return { label: "Scratch size", why: "Agent scratch folders only grow; they slow searches, backups and worktree copies." };
    case "nested_copy":
      return { label: "Nested repo copy", why: "A copy of the repo inside a scratch folder: grep finds its stale files next to the real ones." };
  }
}

/** Decimal KB, the unit rulebook budgets are written in (60 KB = 60 000 bytes). */
export const fmtKB = (n: number) => `${n >= 10_000 ? Math.round(n / 1000) : (n / 1000).toFixed(1)} KB`;

/** `href` (relative to the document at `from`, or to the project folder when it starts with `/`) as a project path. */
export function resolvePath(from: string, href: string): string {
  const parts = href.startsWith("/") || !from.includes("/") ? [] : from.slice(0, from.lastIndexOf("/")).split("/");
  for (const seg of href.replace(/^\/+/, "").split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") parts.pop();
    else parts.push(seg);
  }
  return parts.join("/");
}

/** `#knowledge/<slug>` or, with a path, `#knowledge/<slug>/doc/<path>`. */
export const knowledgeHash = (slug: string, path?: string | null) => formatHash({ screen: "knowledge", arg: path ? `${slug}/doc/${path}` : slug });

/** Markdown unless it is plainly a text file; MAP's own memory is written as Markdown. */
export const isMarkdown = (path: string) => !/\.(txt|text|log)$/i.test(path);

export const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
