"use client";

import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";
import { Empty } from "../ui";

/** Placeholder: see design_handoff_map_dashboard/Reflex.dc.html. */
export function ReflexScreen() {
  return (
    <>
      <TopBar />
      <Main>
        <PageHeader kicker="reflex" title="Reflex" />
        <Empty>Not built yet.</Empty>
      </Main>
    </>
  );
}
