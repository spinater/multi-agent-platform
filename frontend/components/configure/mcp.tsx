"use client";

import { Note, TabHead } from "./ui";

export type McpServerRow = { name: string; providers: string[]; ok: boolean };

/** The MCP servers in scope, for the Agents tab's relation map. */
export function useMcpServers(slug: string | null): { servers: McpServerRow[] | null } {
  void slug;
  return { servers: null };
}

export function McpTab() {
  return (
    <>
      <TabHead title="MCP servers" desc="" savesTo="…" />
      <Note>Coming with its backend.</Note>
    </>
  );
}
