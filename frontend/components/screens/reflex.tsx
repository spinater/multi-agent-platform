"use client";

import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";

import { api, cloneUrl } from "@/lib/api";
import type { Decision } from "@/lib/generated/Decision";
import type { DecisionSummary } from "@/lib/generated/DecisionSummary";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ReflexCase } from "@/lib/generated/ReflexCase";

import { useApp } from "../app-context";
import { Calibrate } from "../reflex/calibrate";
import { Flow } from "../reflex/flow";
import {
  INSTALL_COMMAND,
  MAX_DOMAINS,
  MIN_CASES,
  MODE_BADGE,
  type Corpus,
  type Mode,
  axisFor,
  corpusOf,
  fmtConf,
  modeOf,
  pct,
  scrollToSection,
  smallest,
  toLlm,
} from "../reflex/lib";
import { Log } from "../reflex/log";
import { Playground } from "../reflex/playground";
import { QuestionsModal, starterQuestion } from "../reflex/questions";
import { Setup, type Step } from "../reflex/setup";
import { Banner, ModeBadge, RxButton, RxSection, errText } from "../reflex/ui";
import { Main, PageHeader } from "../shell/page";
import { TopBar } from "../shell/top-bar";

const LEAD =
  "Reflex answers choice, score and yes/no questions from this project's own docs in a few milliseconds, on this machine, without a model. When it is not confident enough, the question goes to an LLM. Every question Reflex answers is an LLM call you don't make.";

/** The Reflex screen, layout A · Guided (design_handoff_map_dashboard/Reflex.dc.html): how it works, set up,
 * playground and calibrate side by side, then the decision log. Everything comes from the current project. */
export function ReflexScreen() {
  const { current, project, projects, connection } = useApp();
  return (
    <>
      <TopBar />
      <div className="reflex-theme flex flex-1 flex-col bg-r-bg text-r-text">
        {current ? (
          <ProjectReflex key={current.project.slug} info={current} />
        ) : project !== null && (connection === "connecting" || !projects.length) ? (
          <Main>
            <p className="text-[11px] tracking-[0.16em] text-r-muted uppercase">loading…</p>
          </Main>
        ) : (
          <NoProject />
        )}
      </div>
    </>
  );
}

function NoProject() {
  const { projects, setProject, go } = useApp();
  return (
    <Main className="gap-[18px]">
      <PageHeader kicker="default workspace · decision engine" title="Reflex" lead={LEAD} actions={<ModeBadge label="no project" color="var(--r-faint)" />} />
      <Banner
        color="var(--lane-llm)"
        title="Reflex runs per project"
        text="Each project has its own corpus repo and its own Reflex process. The default workspace has neither, so pick a project or create one."
        actions={
          <>
            {projects.slice(0, 4).map((p) => (
              <RxButton key={p.project.slug} onClick={() => setProject(p.project.slug)}>
                Open {p.project.name}
              </RxButton>
            ))}
            <RxButton tone="primary" onClick={() => go("projects", "new")}>
              Create a project
            </RxButton>
          </>
        }
      />
    </Main>
  );
}

type Data = { cases: ReflexCase[]; summary: DecisionSummary | null; decisions: Decision[]; corpus: Corpus | null };

/** Loads the screen's data, again whenever the project's live info changes (a decision, a Reflex restart…). */
function useReflexData(info: ProjectInfo) {
  const slug = info.project.slug;
  const [data, setData] = useState<Data>({ cases: [], summary: null, decisions: [], corpus: null });
  const [tick, setTick] = useState(0);
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    Promise.all([api.cases(slug), api.decisionSummary(slug, 24), api.decisions(slug, 500), api.repo(slug, "corpus")]).then(
      ([cases, summary, decisions, repo]) => {
        if (!live) return;
        setData({ cases, summary, decisions, corpus: corpusOf(repo.files) });
        setError("");
      },
      (e) => live && setError(errText(e)),
    );
    return () => {
      live = false;
    };
  }, [slug, info, tick]);
  const refresh = useCallback(() => setTick((t) => t + 1), []);
  return { ...data, error, refresh };
}

const BANNER_TONE: Record<Mode, string> = {
  off: "var(--lane-abstain)",
  nocorpus: "var(--lane-abstain)",
  blocked: "var(--lane-abstain)",
  error: "var(--lane-abstain)",
  noq: "var(--lane-llm)",
  shadow: "var(--lane-llm)",
  live: "var(--lane-reflex)",
};

function ProjectReflex({ info }: { info: ProjectInfo }) {
  const { server, go, toast, now } = useApp();
  const p = info.project;
  const { cases, summary, decisions, corpus, error, refresh } = useReflexData(info);
  const [qid, setQid] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState("");

  const version = server?.reflex_version ?? null;
  const installed = server === null || version !== null;
  const mode = modeOf(info, installed, corpus);
  const question = p.questions.find((q) => q.id === qid) ?? p.questions[0] ?? null;
  const known = new Set(p.questions.map((q) => q.id));
  const usable = cases.filter((c) => known.has(c.question_id));
  const scoredN = usable.filter((c) => c.score).length;
  const axis = useMemo(() => axisFor([...cases.map((c) => c.score?.confidence), p.threshold, ...decisions.slice(0, 100).map((d) => d.confidence)]), [cases, decisions, p.threshold]);
  const can = installed && mode !== "nocorpus" && mode !== "blocked" && p.questions.length > 0;
  const domains = corpus?.domains ?? [];
  const over = corpus ? smallest(corpus) : [];
  const examples = useMemo(() => {
    const own = cases.filter((c) => c.question_id === question?.id).map((c) => c.text);
    const asked = decisions.filter((d) => d.question.id === question?.id).map((d) => d.state);
    return [...new Set([...own.slice(-3), ...asked])].slice(0, 4);
  }, [cases, decisions, question?.id]);

  const act = async (label: string, f: () => Promise<string | void>) => {
    setBusy(label);
    try {
      const msg = await f();
      if (msg) toast(msg);
      refresh();
    } catch (e) {
      toast(errText(e), "bad");
    } finally {
      setBusy("");
    }
  };
  const copyInstall = () =>
    navigator.clipboard.writeText(INSTALL_COMMAND).then(
      () => toast("Install command copied: run it in a terminal on this machine"),
      () => toast(INSTALL_COMMAND, "warn"),
    );
  const checkAgain = () =>
    act("check", async () => {
      const o = await api.overview();
      if (!o.server.reflex_version) return "Reflex is still not found (reflex.binary in map.toml)";
      await api.restartReflex(p.slug);
      return `Found ${o.server.reflex_version}; Reflex restarted`;
    });
  const restart = () => act("restart", async () => `Reflex: ${(await api.restartReflex(p.slug)).state.replace("_", " ")}`);
  const archive = () =>
    act("archive", async () => {
      const r = await api.archiveDomains(p.slug, {});
      return `Archived ${r.moved.join(", ")} (${r.files} files, in _archive/) · Reflex ${r.reflex.state.replace("_", " ")}`;
    });
  const openCorpus = () => go("projects", p.slug);

  const banners: Partial<Record<Mode, { title: string; text: ReactNode; actions: ReactNode }>> = {
    off: {
      title: "Reflex is not installed",
      text: (
        <>
          Every question goes to {toLlm(p.fallback)}. Install Reflex on this machine to start answering from your own docs: <code className="text-r-text">{INSTALL_COMMAND}</code>
        </>
      ),
      actions: (
        <>
          <RxButton onClick={checkAgain} disabled={!!busy}>
            {busy === "check" ? "Checking…" : "Check again"}
          </RxButton>
          <RxButton tone="primary" onClick={copyInstall}>
            Copy install command
          </RxButton>
        </>
      ),
    },
    nocorpus: {
      title: "No corpus yet",
      text: (
        <>
          Add English runbook notes to this project&apos;s corpus repo: one folder per domain, 1–8 domains, <code>.md</code> files
          {corpus?.placeholder ? ", in place of the example folder" : ""}. Edit it in the dashboard or <code className="text-r-text">git clone {cloneUrl(p.slug, "corpus")}</code>.
        </>
      ),
      actions: (
        <RxButton tone="primary" onClick={openCorpus}>
          Open corpus
        </RxButton>
      ),
    },
    blocked: {
      title: `Too many domains (${domains.length})`,
      text: `Reflex supports 1–${MAX_DOMAINS} domains and stays paused until you merge or archive ${over.length}. Questions go to ${toLlm(p.fallback)} meanwhile. Archiving moves the smallest (${over.join(", ")}) to _archive/ in the corpus repo, where Reflex does not read them.`,
      actions: (
        <RxButton tone="primary" onClick={archive} disabled={!!busy}>
          {busy === "archive" ? "Archiving…" : `Archive ${over.length} smallest`}
        </RxButton>
      ),
    },
    error: {
      title: info.reflex.state === "stopped" ? "Reflex is stopped" : "Reflex stopped with an error",
      text: `Questions go to ${toLlm(p.fallback)} until it runs again. ${info.reflex.message.split("\n")[0]}`,
      actions: (
        <RxButton tone="primary" onClick={restart} disabled={!!busy}>
          {busy === "restart" ? "Restarting…" : "Restart Reflex"}
        </RxButton>
      ),
    },
    noq: {
      title: "No questions yet",
      text: "Reflex only answers questions with fixed answers. Add one to start routing.",
      actions: (
        <RxButton tone="primary" onClick={() => setEditing(true)}>
          Add questions
        </RxButton>
      ),
    },
    shadow: {
      title: `Not calibrated · every question goes to ${p.fallback === "none" ? "nobody" : "the LLM"}`,
      text: `Reflex is scoring in shadow, but without a threshold it never answers. ${usable.length < MIN_CASES ? `Add ${MIN_CASES - usable.length} more cases, then calibrate.` : "You have enough cases, run calibration."}`,
      actions: (
        <RxButton tone="primary" onClick={() => scrollToSection("rx-cal")}>
          Open calibrate
        </RxButton>
      ),
    },
  };
  const banner = banners[mode];

  const files = corpus?.files ?? 0;
  const domainsOk = domains.length >= 1 && domains.length <= MAX_DOMAINS && !corpus?.placeholder;
  const calibrated = p.threshold !== null;
  const asked = info.stats.total;
  const steps: Step[] = [
    {
      title: "Install Reflex",
      desc: "Runs on this machine next to the agents. No model, no network calls.",
      done: installed,
      status: installed ? `installed · ${version ?? "reflex"}` : `not installed · $ ${INSTALL_COMMAND}`,
      statusColor: installed ? "var(--lane-reflex)" : "var(--r-muted)",
      cta: installed ? undefined : { label: "Copy install command", onClick: copyInstall },
    },
    {
      title: "Write the corpus",
      desc: "The project's corpus repo · one folder per domain, 1–8 domains, English runbook-style notes in .md files.",
      done: domainsOk,
      status: !installed && !domainsOk
        ? "waiting for step 1"
        : !corpus
          ? "reading the corpus repo…"
          : !domains.length || corpus.placeholder
            ? "no domains yet · only the example folder"
            : domains.length > MAX_DOMAINS
              ? `${domains.length} domains · limit is ${MAX_DOMAINS} · merge or archive ${over.length}`
              : `${domains.length} domains · ${files} files · ${domains.map((d) => d.name).join(", ")}${corpus.archived.length ? ` · archived: ${corpus.archived.join(", ")}` : ""}`,
      statusColor: domains.length > MAX_DOMAINS ? "var(--lane-abstain)" : domainsOk ? "var(--lane-reflex)" : "var(--r-muted)",
      cta: !installed
        ? undefined
        : domains.length > MAX_DOMAINS
          ? { label: `Archive ${over.length} smallest`, onClick: archive, busy: busy === "archive" }
          : !domainsOk
            ? { label: "Open corpus", onClick: openCorpus }
            : undefined,
    },
    {
      title: "Define questions",
      desc: "Each question has fixed answers: choice, score or yes-no. Reflex never writes free text.",
      done: p.questions.length > 0,
      status: p.questions.length ? `${p.questions.length} question${p.questions.length === 1 ? "" : "s"} · ${p.questions.map((q) => q.id).join(", ")}` : "none yet",
      statusColor: p.questions.length ? "var(--lane-reflex)" : "var(--r-muted)",
      cta: domainsOk && !p.questions.length ? { label: "Add questions", onClick: () => setEditing(true) } : undefined,
    },
    {
      title: "Calibrate",
      desc: `At least ${MIN_CASES} cases of "message | correct answer". The threshold comes from them.`,
      done: calibrated,
      status: calibrated ? `threshold ${fmtConf(p.threshold)} · ${usable.length} cases` : `${usable.length} / ${MIN_CASES} cases`,
      statusColor: calibrated ? "var(--lane-reflex)" : usable.length >= MIN_CASES ? "var(--lane-llm)" : "var(--r-muted)",
      cta: p.questions.length && !calibrated ? { label: "Open calibrate", onClick: () => scrollToSection("rx-cal") } : undefined,
    },
    {
      title: "Try the playground",
      desc: "Ask a real question and check which lane it takes and why.",
      done: calibrated && asked > 0,
      status: asked ? `${asked} question${asked === 1 ? "" : "s"} asked` : "not tried yet",
      statusColor: asked ? "var(--lane-reflex)" : "var(--r-muted)",
      cta: calibrated && !asked ? { label: "Open playground", onClick: () => scrollToSection("rx-play") } : undefined,
    },
  ];

  const total = summary?.total ?? 0;
  const saved = total ? (summary?.reflex.count ?? 0) / total : 0;
  const [badge, badgeColor] = info.reflex.state === "starting" && mode !== "off" ? ["starting", "var(--lane-llm)"] : MODE_BADGE[mode];

  return (
    <Main className="gap-[18px]">
      <PageHeader kicker={`project · ${p.slug} · decision engine`} title="Reflex" lead={LEAD} actions={<ModeBadge label={badge} color={badgeColor} />} />
      {banner && <Banner color={BANNER_TONE[mode]} title={banner.title} text={banner.text} actions={banner.actions} />}
      {error && <Banner color="var(--lane-abstain)" title="Could not load the Reflex data" text={error} actions={<RxButton onClick={refresh}>Retry</RxButton>} />}

      <RxSection id="rx-how" n="01" title="How it works" meta={`last 24 h · ${total} questions`}>
        <Flow question={question} corpus={corpus} threshold={p.threshold} mode={mode} cases={scoredN} fallback={p.fallback} summary={summary} axis={axis} />
      </RxSection>
      <RxSection id="rx-setup" n="02" title="Set up" meta={`${steps.filter((s) => s.done).length} / 5 done`}>
        <Setup steps={steps} />
      </RxSection>
      <div className="flex flex-wrap items-start gap-[18px]">
        <RxSection id="rx-play" n="03" title="Playground" meta={p.threshold === null ? "no threshold yet" : `threshold ${fmtConf(p.threshold)}`} className="flex-[1_1_440px]">
          <Playground
            info={info}
            mode={mode}
            axis={axis}
            question={question}
            onPick={setQid}
            examples={examples}
            onEditQuestions={() => setEditing(true)}
            onChanged={refresh}
          />
        </RxSection>
        <RxSection id="rx-cal" n="04" title="Calibrate" meta={`${usable.length} cases · threshold ${fmtConf(p.threshold)}`} className="flex-[1_1_440px]">
          <Calibrate info={info} can={can} cases={cases} axis={axis} question={question} onChanged={refresh} toast={toast} />
        </RxSection>
      </div>
      <RxSection id="rx-log" n="05" title="Results" meta={`${pct(saved)} of LLM calls saved`}>
        <Log
          summary={summary}
          decisions={decisions}
          now={now}
          emptyText={p.questions.length ? "No decisions yet. Ask one in the playground." : "No questions defined yet, so nothing has been routed."}
        />
      </RxSection>
      {editing && (
        <QuestionsModal
          slug={p.slug}
          initial={p.questions.length ? p.questions : [starterQuestion(domains.map((d) => d.name))]}
          domains={domains.map((d) => d.name)}
          onClose={() => setEditing(false)}
          onSaved={() => toast("Questions saved")}
        />
      )}
    </Main>
  );
}
