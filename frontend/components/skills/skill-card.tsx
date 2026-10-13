import clsx from "clsx";

import { providerColor } from "@/lib/colors";
import type { Skill } from "@/lib/generated/Skill";

import { DOT_PROVIDERS, slash, SOURCE_COLOR, sourceLabel, tint, usageText } from "./shared";

export function SourceBadge({ skill }: { skill: Skill }) {
  const c = SOURCE_COLOR[skill.source];
  return (
    <span
      className="inline-flex h-[18px] max-w-[45%] flex-none items-center truncate rounded px-1.5 font-mono text-[9px] font-medium tracking-[0.1em] whitespace-nowrap uppercase"
      style={{ color: c, background: tint(c, 10), border: `1px solid ${tint(c, 35)}` }}
    >
      {sourceLabel(skill)}
    </span>
  );
}

/** Filled dot: the provider loads it (natively or through its context file); ring only: it does not. */
export function ProviderDots({ skill }: { skill: Skill }) {
  return (
    <span className="flex gap-1">
      {DOT_PROVIDERS.map((p) => {
        const load = skill.loads_in.find((l) => l.provider === p);
        const color = providerColor(p);
        return (
          <span
            key={p}
            title={load ? `${p} (${load.how})` : `${p}: not loaded`}
            className="size-2 rounded-full"
            style={{ background: load ? (load.how === "native" ? color : tint(color, 45)) : "transparent", border: `1px solid ${load ? color : "var(--line-3)"}` }}
          />
        );
      })}
    </span>
  );
}

export function SkillCard({ skill, selected, onSelect }: { skill: Skill; selected: boolean; onSelect: () => void }) {
  return (
    <button
      onClick={onSelect}
      aria-pressed={selected}
      className={clsx(
        "flex min-w-0 flex-col gap-2.5 rounded-xl border px-[15px] py-3.5 text-left text-text transition-colors",
        selected ? "border-accent/60 bg-accent/[.06]" : "border-line bg-surface hover:border-line-3",
        !skill.enabled && "opacity-45",
      )}
    >
      <span className="flex w-full items-center justify-between gap-2">
        <span className="truncate font-mono text-[13.5px] leading-[1.2] font-semibold text-accent-soft">/{slash(skill)}</span>
        <SourceBadge skill={skill} />
      </span>
      <span className="line-clamp-3 text-[12.5px] leading-[1.45] text-pretty text-nav">{skill.description || "No description."}</span>
      <span className="mt-auto flex w-full items-center justify-between gap-2">
        <ProviderDots skill={skill} />
        <span className="font-mono text-[10.5px] whitespace-nowrap text-muted">{usageText(skill)}</span>
      </span>
    </button>
  );
}
