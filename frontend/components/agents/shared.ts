import type { AgentDef } from "@/lib/generated/AgentDef";
import type { AgentProvider } from "@/lib/generated/AgentProvider";
import type { AgentScope } from "@/lib/generated/AgentScope";
import type { Capability } from "@/lib/generated/Capability";
import type { Effort } from "@/lib/generated/Effort";

/** The matrix columns, in the backend's provider order. `gemini` is the provider that runs `agy`. */
export const AGENT_PROVIDERS: AgentProvider[] = ["claude", "codex", "gemini"];

export const PROVIDER_LABEL: Record<AgentProvider, string> = { claude: "Claude Code", codex: "Codex", gemini: "agy" };

/** The file each CLI reads, for the empty state and the editor's hints. */
export const PROVIDER_FORMAT: Record<AgentProvider, string> = { claude: "Markdown", codex: "TOML", gemini: "Markdown" };

export const CAPABILITIES: { id: Capability; label: string; note: string }[] = [
  { id: "read", label: "Read", note: "read files · Claude Read · agy view_file, list_dir" },
  { id: "search", label: "Search", note: "find in files · Grep, Glob · grep_search, find_by_name" },
  { id: "edit", label: "Edit", note: "write files · Edit, Write · write_to_file, replace_file_content" },
  { id: "shell", label: "Shell", note: "run commands · Bash · run_command · Codex shell_tool" },
  { id: "web", label: "Web", note: "fetch and search · WebFetch, WebSearch · read_url_content, search_web" },
];

export const EFFORTS: Effort[] = ["low", "medium", "high", "xhigh", "max"];
export const CLAUDE_MODELS = ["inherit", "sonnet", "opus", "haiku", "fable"];
export const GEMINI_MODELS = ["inherit", "flash", "pro"];

export const SCOPE_LABEL: Record<AgentScope, string> = { project: "project", user: "user" };

export function blankDef(): AgentDef {
  return { name: "", description: "", instructions: "", tools: null, models: { claude: null, codex: null, gemini: null }, effort: null, targets: [...AGENT_PROVIDERS] };
}

/** `#agents/<scope>/<name>` opens an agent, `#agents/new` the empty editor. */
export function agentArg(scope: AgentScope, name: string): string {
  return `${scope}/${name}`;
}

export function parseAgentArg(arg: string | null): { scope: AgentScope; name: string } | "new" | null {
  if (arg === "new") return "new";
  const m = arg?.match(/^(project|user)\/(.+)$/);
  return m ? { scope: m[1] as AgentScope, name: m[2] } : null;
}

/** Run as the main agent, where the CLI can. */
export function runCommand(provider: AgentProvider, name: string): string | null {
  if (provider === "claude") return `claude --agent ${name}`;
  if (provider === "gemini") return `agy --agent ${name}`;
  return null;
}
