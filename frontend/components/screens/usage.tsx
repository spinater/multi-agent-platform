"use client";

import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";
import { Empty } from "../ui";

/** Placeholder: see design_handoff_map_dashboard/Usage.dc.html. */
export function UsageScreen() {
  return (
    <>
      <TopBar />
      <Main>
        <PageHeader kicker="usage" title="Usage" />
        <Empty>Not built yet.</Empty>
      </Main>
    </>
  );
}
