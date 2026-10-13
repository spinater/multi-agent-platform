/** Provider and project colours from the design tokens (`--p-*`, `--area-*` in globals.css). */

const PROVIDER_VAR: Record<string, string> = {
  claude: "var(--p-claude)",
  codex: "var(--p-codex)",
  gemini: "var(--p-gemini)",
  agy: "var(--p-gemini)",
};

export function providerColor(name: string | null | undefined): string {
  return (name && PROVIDER_VAR[name]) || "var(--p-other)";
}

/** A stable colour per project for the all-projects views; `null` is the default workspace. */
export function projectColor(slug: string | null, order: (string | null)[]): string {
  const i = order.indexOf(slug);
  return `var(--area-${i < 0 ? 0 : (i % 8) + 1})`;
}
