"use client";

import { useApp } from "../app-context";
import { ProjectsView } from "../projects";
import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";

/** Placeholder until Projects.dc.html is built: the existing project manager. `route.arg` is a slug or `new`. */
export function ProjectsScreen() {
  const { projects, providers, route, go } = useApp();
  return (
    <>
      <TopBar />
      <Main>
        <PageHeader kicker="Workspaces" title="Projects" />
        <ProjectsView projects={projects} providers={providers} selected={route.arg === "new" ? null : route.arg} onSelect={(slug) => go("projects", slug)} />
      </Main>
    </>
  );
}
