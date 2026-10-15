"use client";

import { useEffect, useState } from "react";

import { api } from "@/lib/api";
import type { ProjectSettings } from "@/lib/generated/ProjectSettings";

import { useApp } from "../app-context";
import { same, useConfigure } from "./ui";

/** The newest settings each project saved from here, so two tabs saved one after the other do not undo each other
 * before the WebSocket brings the project back. */
const latest = new Map<string, ProjectSettings>();

/** A tab's draft of some keys of the project's settings (`settings` in `PATCH /api/projects/{slug}`). Edits are
 * local until the save bar saves them; each changed key counts as one change. `value` is `null` without a project. */
export function useProjectSettings<K extends keyof ProjectSettings>(tab: string, keys: K[], files: string[]) {
  const { slug, setPending } = useConfigure();
  const { projects } = useApp();
  const info = slug ? projects.find((p) => p.project.slug === slug) : undefined;
  const base = info ? (latest.get(info.project.slug) ?? info.project.settings) : null;
  const [draft, setDraft] = useState<Partial<Pick<ProjectSettings, K>>>({});

  // A newer copy from the server replaces ours once it has what we saved.
  const server = info?.project.settings;
  useEffect(() => {
    if (server && slug && latest.has(slug) && keys.every((k) => same(server[k], latest.get(slug)![k]))) latest.delete(slug);
  }, [server, slug]); // eslint-disable-line react-hooks/exhaustive-deps

  const value = base ? ({ ...pick(base, keys), ...draft } as Pick<ProjectSettings, K>) : null;
  const changed = base ? keys.filter((k) => k in draft && !same(draft[k], base[k])) : [];

  useEffect(() => {
    if (!slug || !base || !changed.length) {
      setPending(tab, null);
      return;
    }
    setPending(tab, {
      count: changed.length,
      files,
      save: async () => {
        const now = latest.get(slug) ?? base;
        const r = await api.updateProject(slug, { settings: { ...now, ...draft } });
        latest.set(slug, r.project.project.settings);
        setDraft({});
      },
      discard: () => setDraft({}),
    });
  }, [JSON.stringify(draft), slug, base === null]); // eslint-disable-line react-hooks/exhaustive-deps

  const patch = (p: Partial<Pick<ProjectSettings, K>>) => setDraft((d) => ({ ...d, ...p }));
  return { value, patch, changed, project: info ?? null };
}

function pick<T, K extends keyof T>(o: T, keys: K[]): Pick<T, K> {
  const out = {} as Pick<T, K>;
  for (const k of keys) out[k] = o[k];
  return out;
}
