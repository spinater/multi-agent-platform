"use client";

import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";
import { Empty } from "../ui";

/** Placeholder: see design_handoff_map_dashboard/Decisions.dc.html. */
export function DecisionsScreen() {
  return (
    <>
      <TopBar />
      <Main>
        <PageHeader kicker="decisions" title="Decisions" />
        <Empty>Not built yet.</Empty>
      </Main>
    </>
  );
}
