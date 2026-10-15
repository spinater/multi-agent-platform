"use client";

/** Configure › Git: Git & SSH (machine: `git_access.rs`), Repo & branches (the project folder's git state and
 * `settings.branches`), Parallel (`settings.parallel`, plus the machine-wide tasks-at-once cap). The editors are the
 * ones the project settings use; their drafts join the save bar. */

import { useEffect, useState } from "react";

import { gitApi } from "@/lib/api";
import type { SettingsInfo } from "@/lib/generated/SettingsInfo";

import { useApp } from "../app-context";
import { BranchesEditor } from "../branches";
import { ProjectGit } from "../git-access";
import { ParallelEditor, ParallelStepper } from "../parallel";
import { useDefaults } from "../project-ui";
import { GitSshSettings } from "../settings/git-ssh";
import { useProjectSettings } from "./project-draft";
import { Box, Lbl, Note, TabHead, useConfigure } from "./ui";

export function GitTab() {
  const [settings, setSettings] = useState<SettingsInfo | null>(null);
  useEffect(() => {
    gitApi.settings().then(setSettings, () => {});
  }, []);
  return (
    <>
      <TabHead
        title="Git & SSH"
        desc="One SSH setup for every project, so agents can clone, pull and push. MAP’s own key is the default; you can use this machine’s ~/.ssh instead."
        savesTo="<data_dir>/git/ (settings.json, id_ed25519, known_hosts, gitconfig) · saved as you change it"
      />
      <GitSshSettings settings={settings} showCli />
    </>
  );
}

function ProjectOnly() {
  const { setScope } = useConfigure();
  const { project } = useApp();
  return (
    <Note>
      This is a per-project setting, stored with the project.{" "}
      {project !== null ? (
        <>
          Switch to{" "}
          <button onClick={() => setScope("project")} className="cursor-pointer font-semibold text-accent-soft">
            Project
          </button>{" "}
          to change it.
        </>
      ) : (
        "Pick a project in the switcher to change it."
      )}
    </Note>
  );
}

export function BranchesTab() {
  const { scope, slug } = useConfigure();
  const defaults = useDefaults();
  const { value, patch, project } = useProjectSettings("branches", ["branches"], ["project settings (branches)"]);
  return (
    <>
      <TabHead
        title="Repo & branches"
        desc="The project folder’s git state, and whether tasks share that folder or each get their own branch and worktree."
        savesTo="project settings in the platform database (branches)"
      />
      {scope === "machine" || !slug || !value || !project ? (
        <ProjectOnly />
      ) : (
        <>
          <ProjectGit info={project} />
          <Box>
            <Lbl>where tasks run</Lbl>
            <BranchesEditor value={value.branches} onChange={(branches) => patch({ branches })} slug={slug} defaults={defaults} />
          </Box>
        </>
      )}
    </>
  );
}

export function ParallelTab() {
  const { scope, slug } = useConfigure();
  const { orchestrator, providers } = useApp();
  const { value, patch, changed } = useProjectSettings("parallel", ["parallel"], ["project settings (parallel)"]);
  const { value: other } = useProjectSettings("parallel-ctx", ["branches", "audit", "providers"], []);
  return (
    <>
      <TabHead
        title="Parallel"
        desc="How many of this project’s tasks run at once, and who decides which tasks may share the folder."
        savesTo="project settings (parallel) · machine cap: [orchestrator] max_parallel (live)"
      />
      <Box>
        <Lbl>machine · tasks at once across every project</Lbl>
        <ParallelStepper orchestrator={orchestrator} />
      </Box>
      {scope === "machine" || !slug || !value || !other ? (
        <ProjectOnly />
      ) : (
        <Box>
          <Lbl>this project</Lbl>
          <ParallelEditor
            value={value.parallel}
            onChange={(parallel) => patch({ parallel })}
            mode={other.branches.mode}
            slug={slug}
            providers={providers}
            defaultProvider={(other.audit?.enabled && other.audit.provider) || other.providers.find((x) => providers.find((i) => i.name === x.name)?.available)?.name || null}
            dirty={changed.length > 0}
          />
        </Box>
      )}
    </>
  );
}
