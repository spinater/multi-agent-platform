"use client";

import clsx from "clsx";
import { Plus, RotateCw, X } from "lucide-react";
import { useEffect, useState } from "react";

import { api, cloneUrl, fmtConf, type RepoName, type Route } from "@/lib/api";
import type { DecideResponse } from "@/lib/generated/DecideResponse";
import type { Decision } from "@/lib/generated/Decision";
import type { FitResult } from "@/lib/generated/FitResult";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { ProviderInfo } from "@/lib/generated/ProviderInfo";
import type { Question } from "@/lib/generated/Question";
import type { RepoListing } from "@/lib/generated/RepoListing";

import { Badge, Button, Empty, Input, Label, Meter, Panel, Select, Textarea, type Tone } from "./ui";

const err = (e: unknown) => (e as Error).message;
const show = (v: unknown) => (v === null || v === undefined ? "—" : String(v));
const laneTone: Record<string, Tone> = { reflex: "ok", llm: "warn", abstain: "bad" };
const reflexTone: Record<string, Tone> = { running: "ok", starting: "info", no_corpus: "warn", error: "bad", stopped: "idle" };

function FallbackSelect({ value, onChange, providers }: { value: string; onChange: (v: string) => void; providers: ProviderInfo[] }) {
  return (
    <Select value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="api">Anthropic API</option>
      {providers.map((p) => (
        <option key={p.name} value={p.name} disabled={!p.available}>
          {p.name} (headless){p.available ? "" : " · not installed"}
        </option>
      ))}
      <option value="none">None: abstain stays unanswered</option>
    </Select>
  );
}

export function NewProjectForm({ providers, onClose, onDone }: { providers: ProviderInfo[]; onClose: () => void; onDone: (slug: string) => void }) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [description, setDescription] = useState("");
  const [fallback, setFallback] = useState("api");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const submit = async () => {
    setBusy(true);
    setError("");
    try {
      const p = await api.createProject({ name, slug: slug || undefined, description, fallback });
      onDone(p.project.slug);
    } catch (e) {
      setError(err(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel
      title="New project"
      actions={
        <Button size="sm" onClick={onClose} aria-label="Close">
          <X className="size-3.5" />
        </Button>
      }
    >
      <div className="grid gap-3 p-4 sm:grid-cols-2">
        <Label text="Name">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="Customer support" />
        </Label>
        <Label text="Slug (optional)">
          <Input value={slug} onChange={(e) => setSlug(e.target.value)} placeholder="customer-support" />
        </Label>
        <div className="sm:col-span-2">
          <Label text="Description">
            <Input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What this project decides" />
          </Label>
        </div>
        <Label text="When Reflex is unsure, ask">
          <FallbackSelect value={fallback} onChange={setFallback} providers={providers} />
        </Label>
        <p className="self-end text-[11px] text-muted">Creates a code repo, a corpus repo, an agent workspace and a Reflex engine.</p>
        {error && <p className="text-sm text-bad sm:col-span-2">{error}</p>}
        <div className="flex justify-end gap-2 sm:col-span-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={submit} disabled={busy || !name.trim()}>
            {busy ? "Creating…" : "Create project"}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

const TABS = ["overview", "corpus", "code", "playground", "calibrate", "decisions", "settings"] as const;
type Tab = (typeof TABS)[number];

export function ProjectsView({
  projects,
  providers,
  selected,
  onSelect,
}: {
  projects: ProjectInfo[];
  providers: ProviderInfo[];
  selected: string | null;
  onSelect: (slug: string | null) => void;
}) {
  const [creating, setCreating] = useState(false);
  const [tab, setTab] = useState<Tab>("overview");
  const info = projects.find((p) => p.project.slug === selected) ?? null;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        {projects.map(({ project: p, reflex }) => (
          <button
            key={p.slug}
            onClick={() => onSelect(p.slug)}
            className={clsx("flex items-center gap-2 rounded border px-2.5 py-1 text-[12px]", selected === p.slug ? "border-accent text-text" : "border-line text-muted hover:text-text")}
          >
            <span className={clsx("size-1.5 rounded-full", reflex.state === "running" ? "bg-ok" : reflex.state === "error" ? "bg-bad" : "bg-warn")} />
            {p.slug}
          </button>
        ))}
        <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
          <Plus className="size-3.5" /> New project
        </Button>
      </div>
      {creating && (
        <NewProjectForm
          providers={providers}
          onClose={() => setCreating(false)}
          onDone={(slug) => {
            setCreating(false);
            onSelect(slug);
          }}
        />
      )}
      {!info ? (
        !creating && (
          <Panel title="Projects">
            <Empty>A project is a code repo, a corpus repo and a Reflex engine. Pick one above or create one.</Empty>
          </Panel>
        )
      ) : (
        <>
          <div className="flex flex-wrap items-baseline gap-3">
            <h2 className="text-lg font-semibold">{info.project.name}</h2>
            <Badge tone={reflexTone[info.reflex.state]}>reflex {info.reflex.state.replace("_", " ")}</Badge>
            <span className="text-[12px] text-muted">{info.project.description}</span>
          </div>
          <nav className="flex gap-1 overflow-x-auto border-b border-line">
            {TABS.map((t) => (
              <button key={t} onClick={() => setTab(t)} className={clsx("label px-3 py-2 whitespace-nowrap hover:text-text", tab === t && "border-b border-accent text-text")}>
                {t}
              </button>
            ))}
          </nav>
          {tab === "overview" && <Overview info={info} />}
          {tab === "corpus" && <RepoEditor key={`${info.project.slug}-corpus`} slug={info.project.slug} repo="corpus" />}
          {tab === "code" && <RepoEditor key={`${info.project.slug}-code`} slug={info.project.slug} repo="code" />}
          {tab === "playground" && <Playground key={info.project.slug} info={info} />}
          {tab === "calibrate" && <Calibrate key={info.project.slug} info={info} />}
          {tab === "decisions" && <Decisions key={info.project.slug} slug={info.project.slug} total={info.stats.total} />}
          {tab === "settings" && <Settings key={info.project.slug} info={info} providers={providers} onDeleted={() => onSelect(null)} />}
        </>
      )}
    </div>
  );
}

function Overview({ info }: { info: ProjectInfo }) {
  const { project: p, reflex, stats } = info;
  const [log, setLog] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const decideUrl = typeof location === "undefined" ? "" : `${location.origin}/api/projects/${p.slug}/decide`;
  const curl = `curl -s ${decideUrl} \\
  -H 'content-type: application/json' \\
  -d '{"state": "the facts of the situation",
       "questions": [{"id": "route", "kind": "choice",
         "prompt": "Route this ticket", "options": ["a", "b"]}]}'`;

  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <Panel
        title="Reflex engine"
        actions={
          <>
            <Button size="sm" onClick={async () => setLog((await api.reflexLog(p.slug)).log || "(empty)")}>
              log
            </Button>
            <Button
              size="sm"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                await api.restartReflex(p.slug).finally(() => setBusy(false));
              }}
            >
              <RotateCw className={clsx("size-3.5", busy && "animate-spin")} /> restart
            </Button>
          </>
        }
      >
        <div className="grid gap-2 p-4 text-[12px]">
          <p>
            <Badge tone={reflexTone[reflex.state]}>{reflex.state.replace("_", " ")}</Badge> <span className="text-muted">port {p.port}</span>
          </p>
          {reflex.message && <pre className="rounded bg-panel-2 p-2 whitespace-pre-wrap">{reflex.message}</pre>}
          <p className="flex flex-wrap gap-1.5">
            <span className="label">domains</span>
            {reflex.domains.length ? reflex.domains.map((d) => <Badge key={d}>{d}</Badge>) : <span className="text-muted">none</span>}
          </p>
          {log !== null && <pre className="max-h-60 overflow-auto rounded bg-term-bg p-2 text-[11px] text-term-text">{log}</pre>}
        </div>
      </Panel>

      <Panel title="Decisions">
        <div className="grid gap-3 p-4">
          <div className="grid grid-cols-4 gap-3">
            {(
              [
                ["total", stats.total],
                ["reflex", stats.reflex],
                ["llm", stats.llm],
                ["abstain", stats.abstain],
              ] as const
            ).map(([k, v]) => (
              <div key={k}>
                <p className="text-2xl font-semibold tabular-nums">{v}</p>
                <p className="label">{k}</p>
              </div>
            ))}
          </div>
          <Meter value={stats.reflex} max={stats.total} tone="ok" />
          <p className="text-[11px] text-muted">
            LLM calls avoided {stats.total ? Math.round((100 * stats.reflex) / stats.total) : 0}% · threshold {fmtConf(p.threshold)}
            {p.threshold === null && " (not fitted: every question goes to the fallback; use calibrate)"} · fallback <code>{p.fallback}</code>
            {p.model && <code> {p.model}</code>}
          </p>
        </div>
      </Panel>

      <Panel title="Repositories">
        <div className="grid gap-2 p-4 text-[12px]">
          <p className="label">code: agents work in a checkout of it</p>
          <pre className="overflow-x-auto rounded bg-panel-2 p-2">git clone {typeof location === "undefined" ? "" : cloneUrl(p.slug, "code")}</pre>
          <p className="label mt-2">corpus: one folder per domain; a push to main reloads Reflex</p>
          <pre className="overflow-x-auto rounded bg-panel-2 p-2">git clone {typeof location === "undefined" ? "" : cloneUrl(p.slug, "corpus")}</pre>
          <p className="text-[11px] text-muted">With server.api_token set, git asks for a password: use the token.</p>
        </div>
      </Panel>

      <Panel title="API">
        <div className="grid gap-2 p-4 text-[12px]">
          <pre className="overflow-x-auto rounded bg-panel-2 p-2 text-[11px]">{curl}</pre>
          <p className="text-[11px] text-muted">
            Agents started in this project get the URL as <code>$MAP_DECIDE_URL</code>. Each answer carries <code>lane</code>: reflex, llm or abstain.
          </p>
        </div>
      </Panel>
    </div>
  );
}

function RepoEditor({ slug, repo }: { slug: string; repo: RepoName }) {
  const [listing, setListing] = useState<RepoListing | null>(null);
  const [path, setPath] = useState("");
  const [content, setContent] = useState("");
  const [original, setOriginal] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  // Bumped after each commit to reload the listing.
  const [version, setVersion] = useState(0);
  useEffect(() => {
    let stale = false;
    api.repo(slug, repo).then(
      (l) => !stale && setListing(l),
      (e) => !stale && setError(err(e)),
    );
    return () => {
      stale = true;
    };
  }, [slug, repo, version]);

  const open = async (p: string) => {
    setError("");
    setStatus("");
    try {
      const r = await api.readFile(slug, repo, p);
      setPath(p);
      setContent(r.content);
      setOriginal(r.content);
    } catch (e) {
      setError(err(e));
    }
  };

  const save = async (remove = false) => {
    setBusy(true);
    setError("");
    try {
      const r = await api.writeFile(slug, repo, { path, content: remove ? null : content, message });
      setStatus(`committed ${r.commit.slice(0, 8)}${repo === "corpus" ? " · Reflex restarted" : ""}`);
      setMessage("");
      if (remove) {
        setPath("");
        setContent("");
        setOriginal(null);
      } else setOriginal(content);
      setVersion((v) => v + 1);
    } catch (e) {
      setError(err(e));
    } finally {
      setBusy(false);
    }
  };

  const dirty = original === null ? path !== "" : content !== original;

  return (
    <div className="grid gap-4">
      {repo === "corpus" && (
        <p className="text-[12px] text-muted">
          Each top-level folder is one Reflex domain (1 to 8). Write each <code>.md</code> as prose describing that domain&apos;s situations and how they are handled, like a
          runbook. English works best: Reflex did not tokenise Thai well in testing.
        </p>
      )}
      <div className="grid gap-4 lg:grid-cols-[240px_minmax(0,1fr)]">
        <Panel
          title="Files"
          actions={
            <Button
              size="sm"
              onClick={() => {
                setPath(repo === "corpus" ? "domain/notes.md" : "");
                setContent("");
                setOriginal(null);
                setStatus("");
              }}
            >
              <Plus className="size-3.5" /> new
            </Button>
          }
        >
          <ul className="max-h-[420px] overflow-auto p-2">
            {listing?.files.map((f) => (
              <li key={f.path}>
                <button
                  onClick={() => open(f.path)}
                  className={clsx("w-full truncate rounded px-2 py-1 text-left text-[12px] hover:bg-panel-2", f.path === path && "bg-panel-2 text-accent")}
                >
                  {f.path}
                </button>
              </li>
            ))}
            {listing && listing.files.length === 0 && <li className="px-2 py-1 text-[12px] text-muted">empty</li>}
          </ul>
        </Panel>
        <Panel title={original === null && path ? "new file" : path || "editor"}>
          <div className="grid gap-2 p-3">
            <Input value={path} onChange={(e) => setPath(e.target.value)} readOnly={original !== null} placeholder="path/in/repo.md" />
            <Textarea rows={16} value={content} onChange={(e) => setContent(e.target.value)} />
            <div className="flex flex-wrap gap-2">
              <Input className="min-w-48 flex-1" value={message} onChange={(e) => setMessage(e.target.value)} placeholder="commit message (optional)" />
              <Button variant="primary" disabled={busy || !path || !dirty} onClick={() => save()}>
                {busy ? "Committing…" : "Commit"}
              </Button>
              {original !== null && (
                <Button variant="danger" disabled={busy} onClick={() => confirm(`Delete ${path}?`) && save(true)}>
                  Delete
                </Button>
              )}
            </div>
            {status && <p className="text-[11px] text-muted">{status}</p>}
            {error && <p className="text-[12px] text-bad">{error}</p>}
          </div>
        </Panel>
      </div>
      <Panel title="History">
        <ul className="divide-y divide-line">
          {listing?.commits.map((c) => (
            <li key={c.sha} className="flex gap-3 px-4 py-1.5 text-[12px]">
              <span className="text-muted">{c.sha.slice(0, 8)}</span>
              <span className="truncate">{c.subject}</span>
              <span className="ml-auto shrink-0 text-muted">
                {c.author} · {new Date(c.at).toLocaleString()}
              </span>
            </li>
          ))}
        </ul>
      </Panel>
    </div>
  );
}

const blank = (n: number): Question => ({ id: `q${n}`, kind: "choice", prompt: "", options: ["", ""] });

/** Empty option boxes are dropped, so a half-filled form sends what the user meant. */
const clean = (qs: Question[]): Question[] =>
  qs.map((q) => (q.kind === "noul" ? { ...q, options: undefined } : { ...q, options: (q.options ?? []).map((o) => o.trim()).filter(Boolean) }));

function QuestionsEditor({ value, onChange }: { value: Question[]; onChange: (q: Question[]) => void }) {
  const set = (i: number, patch: Partial<Question>) => onChange(value.map((q, j) => (j === i ? { ...q, ...patch } : q)));
  return (
    <div className="grid gap-2">
      {value.map((q, i) => (
        <div key={i} className="grid gap-2 rounded border border-line bg-panel-2/50 p-3">
          <div className="grid grid-cols-[6rem_9rem_minmax(0,1fr)_auto] items-end gap-2">
            <Label text="id">
              <Input value={q.id} onChange={(e) => set(i, { id: e.target.value })} />
            </Label>
            <Label text="kind">
              <Select value={q.kind} onChange={(e) => set(i, { kind: e.target.value as Question["kind"], options: e.target.value === "noul" ? undefined : (q.options ?? ["", ""]) })}>
                <option value="choice">choice</option>
                <option value="score">score</option>
                <option value="noul">yes / no</option>
              </Select>
            </Label>
            <Label text="prompt">
              <Input value={q.prompt} onChange={(e) => set(i, { prompt: e.target.value })} />
            </Label>
            <Button variant="danger" onClick={() => onChange(value.filter((_, j) => j !== i))} aria-label="Remove question">
              <X className="size-3.5" />
            </Button>
          </div>
          {q.kind !== "noul" && (
            <Label text={`options, comma separated${q.kind === "score" ? " (lowest first)" : ""}`}>
              <Input value={(q.options ?? []).join(", ")} onChange={(e) => set(i, { options: e.target.value.split(",").map((s) => s.trimStart()) })} />
            </Label>
          )}
          <Label text="criteria (optional): what decides between options">
            <Input value={q.criteria ?? ""} onChange={(e) => set(i, { criteria: e.target.value || undefined })} />
          </Label>
        </div>
      ))}
      <div>
        <Button size="sm" onClick={() => onChange([...value, blank(value.length + 1)])}>
          <Plus className="size-3.5" /> question
        </Button>
      </div>
    </div>
  );
}

function Playground({ info }: { info: ProjectInfo }) {
  const p = info.project;
  const [state, setState] = useState("");
  const [questions, setQuestions] = useState<Question[]>(p.questions.length ? p.questions : [blank(1)]);
  const [route, setRoute] = useState<Route>("auto");
  const [result, setResult] = useState<DecideResponse | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState<Record<string, string>>({});

  const decide = async () => {
    setBusy(true);
    setError("");
    setSent({});
    try {
      setResult(await api.decide(p.slug, { state, questions: clean(questions), route }));
    } catch (e) {
      setError(err(e));
    } finally {
      setBusy(false);
    }
  };

  const feedback = async (id: string, confidence: number, outcome: boolean) => {
    try {
      const r = await api.feedback(p.slug, confidence, outcome);
      setSent((s) => ({ ...s, [id]: r.refit ? "sent · recalibrated" : "sent" }));
    } catch (e) {
      setSent((s) => ({ ...s, [id]: err(e) }));
    }
  };

  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <Panel title="Ask">
        <div className="grid gap-3 p-4">
          <Label text="State: the facts of the situation">
            <Textarea rows={5} value={state} onChange={(e) => setState(e.target.value)} />
          </Label>
          <QuestionsEditor value={questions} onChange={setQuestions} />
          <div className="flex flex-wrap items-end gap-2">
            <Label text="Route">
              <Select value={route} onChange={(e) => setRoute(e.target.value as Route)}>
                <option value="auto">auto: Reflex, then fallback</option>
                <option value="reflex">Reflex only</option>
                <option value="llm">fallback only</option>
              </Select>
            </Label>
            <span className="flex-1" />
            <Button
              onClick={() =>
                api
                  .updateProject(p.slug, { questions: clean(questions) })
                  .then(() => setError(""))
                  .catch((e) => setError(err(e)))
              }
            >
              Save questions
            </Button>
            <Button variant="primary" onClick={decide} disabled={busy || questions.length === 0}>
              {busy ? "Deciding…" : "Decide"}
            </Button>
          </div>
          {error && <p className="text-[12px] text-bad">{error}</p>}
        </div>
      </Panel>
      <div className="grid content-start gap-3">
        {!result ? (
          <Panel title="Answers">
            <Empty>
              Each answer shows which lane produced it. Threshold {fmtConf(p.threshold)}
              {p.threshold === null && ": not fitted, so in auto mode every question goes to the fallback"}.
            </Empty>
          </Panel>
        ) : (
          <>
            {result.answers.map((a) => (
              <Panel key={a.question_id} title={a.question_id} actions={<Badge tone={laneTone[a.lane]}>{a.lane}</Badge>}>
                <div className="grid gap-2 p-4 text-[12px]">
                  <p className="text-2xl font-semibold">{show(a.answer)}</p>
                  <dl className="grid grid-cols-[8rem_1fr] gap-x-2 gap-y-1 text-muted">
                    <dt>Reflex said</dt>
                    <dd className="text-text">{show(a.reflex_answer)}</dd>
                    <dt>confidence</dt>
                    <dd className="text-text">
                      {fmtConf(a.confidence)} (threshold {fmtConf(result.threshold)})
                    </dd>
                    <dt>probabilities</dt>
                    <dd className="text-text">{a.probabilities?.map((x) => x.toFixed(3)).join(", ") ?? "—"}</dd>
                    {a.llm_raw !== null && (
                      <>
                        <dt>fallback said</dt>
                        <dd className="text-text">{a.llm_raw}</dd>
                      </>
                    )}
                    <dt>time</dt>
                    <dd className="text-text">{a.ms} ms</dd>
                  </dl>
                  {a.error && <p className="text-bad">{a.error}</p>}
                  {a.lane === "reflex" && a.confidence !== null && (
                    <div className="flex items-center gap-2">
                      <span className="text-muted">Was Reflex right?</span>
                      <Button size="sm" onClick={() => feedback(a.question_id, a.confidence!, true)}>
                        yes
                      </Button>
                      <Button size="sm" onClick={() => feedback(a.question_id, a.confidence!, false)}>
                        no
                      </Button>
                      <span className="text-muted">{sent[a.question_id]}</span>
                    </div>
                  )}
                </div>
              </Panel>
            ))}
            <p className="text-[11px] text-muted">
              total {result.ms} ms{result.reflex_reason && ` · ${result.reflex_reason}`}
            </p>
            {result.reflex_error && <p className="text-[12px] text-bad">{result.reflex_error}</p>}
          </>
        )}
      </div>
    </div>
  );
}

function Calibrate({ info }: { info: ProjectInfo }) {
  const p = info.project;
  const [qid, setQid] = useState(p.questions[0]?.id ?? "");
  const [text, setText] = useState("");
  const [rho, setRho] = useState(0.3);
  const [result, setResult] = useState<FitResult | null>(null);
  const [rows, setRows] = useState<{ expected: string | boolean; state: string }[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const question = p.questions.find((q) => q.id === qid);

  if (!question) return <Empty>Save at least one question in the playground first.</Empty>;

  const run = async (apply: boolean) => {
    setBusy(true);
    setError("");
    try {
      const parsed = text
        .split("\n")
        .map((l) => l.trim())
        .filter(Boolean)
        .map((l) => {
          const i = l.indexOf("|");
          if (i < 0) throw new Error(`missing "|" in: ${l}`);
          const exp = l.slice(0, i).trim();
          return { expected: question.kind === "noul" ? /^(yes|true|y|1|ใช่)$/i.test(exp) : exp, state: l.slice(i + 1).trim() };
        });
      setRows(parsed);
      setResult(await api.fit(p.slug, { rho, apply, cases: parsed.map((c) => ({ ...c, question })) }));
    } catch (e) {
      setError(err(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <Panel title="Labeled cases">
        <div className="grid gap-3 p-4">
          <p className="text-[12px] text-muted">
            Use 16 or more cases the corpus has <b>not</b> seen. The threshold is the ρ-quantile of Reflex&apos;s confidence on the cases it got right, so about ρ of them still
            go to the fallback. Current: <code>{fmtConf(p.threshold)}</code>.
          </p>
          <div className="grid grid-cols-[minmax(0,1fr)_6rem] gap-2">
            <Label text="Question">
              <Select value={qid} onChange={(e) => setQid(e.target.value)}>
                {p.questions.map((q) => (
                  <option key={q.id} value={q.id}>
                    {q.id}: {q.prompt}
                  </option>
                ))}
              </Select>
            </Label>
            <Label text="ρ">
              <Input type="number" min={0} max={1} step={0.05} value={rho} onChange={(e) => setRho(Number(e.target.value))} />
            </Label>
          </div>
          <Label text={`expected | state, one per line (${question.kind === "noul" ? "yes / no" : (question.options ?? []).join(" / ")})`}>
            <Textarea
              rows={12}
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder={"refund | My order came cracked and I want my money back.\nshipping | The parcel has not arrived after ten days."}
            />
          </Label>
          <div className="flex items-center gap-2">
            <span className="text-[11px] text-muted">{text.split("\n").filter((l) => l.trim()).length} cases</span>
            <span className="flex-1" />
            <Button disabled={busy || !text.trim()} onClick={() => run(false)}>
              Evaluate
            </Button>
            <Button variant="primary" disabled={busy || !text.trim()} onClick={() => run(true)}>
              Fit & apply
            </Button>
          </div>
          {error && <p className="text-[12px] text-bad">{error}</p>}
        </div>
      </Panel>
      {result && (
        <div className="grid content-start gap-3">
          <Panel title={result.applied ? "Fitted · applied" : "Fitted"}>
            <div className="grid grid-cols-3 gap-3 p-4">
              <div>
                <p className="text-2xl font-semibold">{fmtConf(result.threshold)}</p>
                <p className="label">threshold</p>
              </div>
              <div>
                <p className="text-2xl font-semibold">{Math.round(result.coverage * 100)}%</p>
                <p className="label">answered by Reflex</p>
              </div>
              <div>
                <p className="text-2xl font-semibold">{result.pass_precision === null ? "—" : `${Math.round(result.pass_precision * 100)}%`}</p>
                <p className="label">correct when it answers</p>
              </div>
              <p className="col-span-3 text-[11px] text-muted">
                n={result.n} · answered {result.n_answered} · correct {result.n_correct} · at or above threshold {result.n_pass}
                {result.n < 16 && " · under 16 cases: a rough look only"}
              </p>
            </div>
          </Panel>
          <Panel title="Cases">
            <div className="overflow-x-auto">
              <table className="w-full text-[12px]">
                <thead>
                  <tr className="label text-left">
                    <th className="px-3 py-1.5 font-medium">expected</th>
                    <th className="px-3 py-1.5 font-medium">reflex</th>
                    <th className="px-3 py-1.5 font-medium">conf</th>
                    <th className="px-3 py-1.5 font-medium" />
                    <th className="px-3 py-1.5 font-medium">state</th>
                  </tr>
                </thead>
                <tbody>
                  {result.cases.map((c, i) => (
                    <tr key={i} className="border-t border-line">
                      <td className="px-3 py-1.5">{String(rows[i]?.expected)}</td>
                      <td className="px-3 py-1.5">{c.answer === null ? "abstain" : String(c.answer)}</td>
                      <td className="px-3 py-1.5">{fmtConf(c.confidence)}</td>
                      <td className={clsx("px-3 py-1.5", c.correct ? "text-ok" : "text-bad")}>{c.correct ? "✓" : "✗"}</td>
                      <td className="max-w-80 truncate px-3 py-1.5 text-muted">{rows[i]?.state}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Panel>
        </div>
      )}
    </div>
  );
}

function Decisions({ slug, total }: { slug: string; total: number }) {
  const [rows, setRows] = useState<Decision[] | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    // Refetches whenever a new decision is recorded (the total comes from the live project info).
    api.decisions(slug).then(setRows, (e) => setError(err(e)));
  }, [slug, total]);
  if (error) return <p className="text-bad">{error}</p>;
  if (!rows) return <Empty>Loading…</Empty>;
  if (!rows.length) return <Empty>No decisions yet.</Empty>;
  return (
    <Panel title={`Decisions · newest first`}>
      <div className="overflow-x-auto">
        <table className="w-full text-[12px]">
          <thead>
            <tr className="label text-left">
              {["time", "lane", "question", "answer", "reflex", "conf", "ms", "state"].map((h) => (
                <th key={h} className="px-3 py-1.5 font-medium">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((d) => (
              <tr key={d.id} className="border-t border-line align-top">
                <td className="px-3 py-1.5 whitespace-nowrap text-muted">{new Date(d.at).toLocaleString()}</td>
                <td className="px-3 py-1.5">
                  <Badge tone={laneTone[d.lane]}>{d.lane}</Badge>
                </td>
                <td className="px-3 py-1.5">{d.question.id}</td>
                <td className="px-3 py-1.5">
                  {show(d.answer)}
                  {d.error && <p className="text-bad">{d.error}</p>}
                </td>
                <td className="px-3 py-1.5 text-muted">{d.reflex_answer === null ? "abstain" : String(d.reflex_answer)}</td>
                <td className="px-3 py-1.5">{fmtConf(d.confidence)}</td>
                <td className="px-3 py-1.5 text-muted">{d.ms}</td>
                <td className="max-w-96 truncate px-3 py-1.5 text-muted">{d.state}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}

function Settings({ info, providers, onDeleted }: { info: ProjectInfo; providers: ProviderInfo[]; onDeleted: () => void }) {
  const p = info.project;
  const [name, setName] = useState(p.name);
  const [description, setDescription] = useState(p.description);
  const [fallback, setFallback] = useState(p.fallback);
  const [model, setModel] = useState(p.model ?? "");
  const [threshold, setThreshold] = useState(p.threshold === null ? "" : String(p.threshold));
  const [confirmSlug, setConfirmSlug] = useState("");
  const [msg, setMsg] = useState("");
  const [error, setError] = useState("");
  const provider = providers.find((x) => x.name === fallback);

  const save = async () => {
    setError("");
    setMsg("");
    try {
      const t = threshold.trim() === "" ? null : Number(threshold);
      if (t !== null && !Number.isFinite(t)) throw new Error("threshold must be a number, or empty");
      await api.updateProject(p.slug, { name, description, fallback, model: model.trim() || null, threshold: t });
      setMsg("saved");
    } catch (e) {
      setError(err(e));
    }
  };

  return (
    <div className="grid gap-4 xl:grid-cols-2">
      <Panel title="Project">
        <div className="grid gap-3 p-4">
          <Label text="Name">
            <Input value={name} onChange={(e) => setName(e.target.value)} />
          </Label>
          <Label text="Description">
            <Input value={description} onChange={(e) => setDescription(e.target.value)} />
          </Label>
          <Label text="When Reflex is unsure, ask">
            <FallbackSelect value={fallback} onChange={setFallback} providers={providers} />
          </Label>
          <Label text={provider ? `Model (${provider.models.join(", ") || "any"})` : "Model (empty: the server's default)"}>
            {provider && provider.models.length > 0 ? (
              <Select value={model} onChange={(e) => setModel(e.target.value)}>
                <option value="">default ({provider.default_model})</option>
                {provider.models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
            ) : (
              <Input value={model} onChange={(e) => setModel(e.target.value)} placeholder="claude-haiku-5-5" />
            )}
          </Label>
          <Label text="Reflex confidence threshold (empty: always fall back; prefer calibrate)">
            <Input value={threshold} onChange={(e) => setThreshold(e.target.value)} placeholder="e.g. 1.2e-5" />
          </Label>
          <div className="flex items-center gap-2">
            <Button variant="primary" onClick={save}>
              Save
            </Button>
            <span className="text-[11px] text-muted">{msg}</span>
          </div>
          {error && <p className="text-[12px] text-bad">{error}</p>}
        </div>
      </Panel>
      <Panel title="Delete project">
        <div className="grid gap-3 p-4 text-[12px]">
          <p className="text-muted">
            Stops its Reflex and permanently deletes both repos, the agent workspace and the decision log. Type <code>{p.slug}</code> to confirm.
          </p>
          <Input value={confirmSlug} onChange={(e) => setConfirmSlug(e.target.value)} />
          <div>
            <Button
              variant="danger"
              disabled={confirmSlug !== p.slug}
              onClick={() =>
                api
                  .deleteProject(p.slug)
                  .then(onDeleted)
                  .catch((e) => setError(err(e)))
              }
            >
              Delete project
            </Button>
          </div>
        </div>
      </Panel>
    </div>
  );
}
