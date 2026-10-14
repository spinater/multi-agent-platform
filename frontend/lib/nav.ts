/** Screens of the dashboard and their hash routes: `#<screen>` or `#<screen>/<arg>`, e.g. `#reflex`,
 * `#command/sessions/map-task-4-1`, `#projects/new`,
 * `#projects/init/<slug>`, `#knowledge/<slug>/doc/<path>`, `#settings/git` (or `agents`, `server`). The static export is one page, so routing lives
 * in the hash. */
export const SCREENS = ["projects", "command", "skills", "agents", "knowledge", "decisions", "reflex", "usage", "settings"] as const;
export type Screen = (typeof SCREENS)[number];

export const SCREEN_LABEL: Record<Screen, string> = {
  projects: "Projects",
  command: "Command centre",
  skills: "Skills",
  agents: "Agents",
  knowledge: "Knowledge",
  decisions: "Decisions",
  reflex: "Reflex",
  usage: "Usage",
  settings: "Settings",
};

export type Route = { screen: Screen; arg: string | null };

/** Hashes from before the screens existed open the command centre's matching view. */
const LEGACY = ["graph", "tasks", "sessions", "events"];

export function parseHash(hash: string): Route {
  const raw = decodeURIComponent(hash.replace(/^#\/?/, ""));
  const slash = raw.indexOf("/");
  const head = slash < 0 ? raw : raw.slice(0, slash);
  const rest = slash < 0 ? null : raw.slice(slash + 1) || null;
  if ((SCREENS as readonly string[]).includes(head)) return { screen: head as Screen, arg: rest };
  if (LEGACY.includes(head)) return { screen: "command", arg: raw };
  return { screen: "command", arg: null };
}

export function formatHash({ screen, arg }: Route): string {
  return `#${screen}${arg ? `/${arg.split("/").map(encodeURIComponent).join("/")}` : ""}`;
}
