"use client";

import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";
import { Empty } from "../ui";

/** Placeholder: see design_handoff_map_dashboard/Skills.dc.html. */
export function SkillsScreen() {
  return (
    <>
      <TopBar />
      <Main>
        <PageHeader kicker="skills" title="Skills" />
        <Empty>Not built yet.</Empty>
      </Main>
    </>
  );
}
