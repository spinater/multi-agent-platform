import type { Lane } from "@/lib/generated/Lane";
import type { ProjectInfo } from "@/lib/generated/ProjectInfo";
import type { Question } from "@/lib/generated/Question";
import type { ReflexCase } from "@/lib/generated/ReflexCase";
import type { RepoFile } from "@/lib/generated/RepoFile";

/** Lane colours are fixed everywhere (README, "Reflex"). */
export const LANE_COLOR: Record<Lane, string> = { reflex: "var(--lane-reflex)", llm: "var(--lane-llm)", abstain: "var(--lane-abstain)" };
export const LANES: Lane[] = ["reflex", "llm", "abstain"];

/** `color-mix` tint of a colour, for borders and backgrounds. */
export const mix = (color: string, pct: number) => `color-mix(in srgb, ${color} ${pct}%, transparent)`;

export const INSTALL_COMMAND = "curl -fsSL https://raw.githubusercontent.com/gist-rs/reflex/main/install.sh | sh";
export const MAX_DOMAINS = 8;
export const MIN_CASES = 16;
/** The folder of the corpus repo that Reflex never loads (`repo::ARCHIVE_DIR`). */
export const ARCHIVE_DIR = "_archive";

/** Reflex confidence, always as `2.1e-4`, never a percentage. */
export const fmtConf = (c: number | null | undefined) => (c == null ? "—" : c === 0 ? "0" : c.toExponential(1));
export const fmtLog = (l: number | null | undefined) => (l == null ? "—" : fmtConf(10 ** l));
export const pct = (f: number) => `${Math.round(f * 100)}%`;
/** Times are whole milliseconds per decision (averages are fractional); a sub-millisecond Reflex answer is `<1 ms`. */
export const fmtMs = (ms: number) => (ms === 0 ? "<1 ms" : ms < 1000 ? `${Math.round(ms * 10) / 10} ms` : `${(ms / 1000).toFixed(2)} s`);
export const fmtAnswer = (v: string | boolean | null | undefined) => (v === null || v === undefined ? "no answer" : v === true ? "yes" : v === false ? "no" : v);
export const kindLabel = (q: Question) => (q.kind === "noul" ? "yes-no" : q.kind);
export const optionsOf = (q: Question): (string | boolean)[] => (q.kind === "noul" ? [true, false] : (q.options ?? []));

export const THAI = /[฀-๿]/;

/** A deterministic 0..1 hash, for the scatter's vertical jitter. */
export function hash(s: string): number {
  let x = 2166136261;
  for (let i = 0; i < s.length; i++) {
    x ^= s.charCodeAt(i);
    x = Math.imul(x, 16777619);
  }
  x ^= x >>> 13;
  x = Math.imul(x, 0x5bd1e995);
  x ^= x >>> 15;
  return (x >>> 0) / 4294967296;
}

/** The log axis, in decades: 1e-5 … 1e-3 as designed, widened by whole decades when real confidences fall outside,
 * so a corpus whose scores reach 1e-2 is not piled against the right edge. */
export type Axis = { lo: number; hi: number };
export const DEFAULT_AXIS: Axis = { lo: -5, hi: -3 };

export function axisFor(values: (number | null | undefined)[]): Axis {
  let { lo, hi } = DEFAULT_AXIS;
  for (const v of values) {
    if (v == null || !(v > 0)) continue;
    const l = Math.log10(v);
    lo = Math.min(lo, Math.floor(l));
    hi = Math.max(hi, Math.ceil(l));
  }
  return { lo: Math.max(lo, -9), hi: Math.min(hi, 0) };
}

/** Fraction 0..1 along the axis; with the default axis this is `(log10(c) + 5) / 2`, clamped. */
export const posLog = (l: number, a: Axis) => Math.max(0, Math.min(1, (l - a.lo) / (a.hi - a.lo)));
export const pos = (c: number | null | undefined, a: Axis) => (c == null || !(c > 0) ? 0 : posLog(Math.log10(c), a));
export const pctPos = (f: number) => `${(f * 100).toFixed(2)}%`;

export function ticks(a: Axis): { label: string; at: number }[] {
  const out: { label: string; at: number }[] = [];
  for (let d = a.lo; d <= a.hi; d++) {
    out.push({ label: `1e${d}`, at: posLog(d, a) });
    if (d < a.hi) out.push({ label: `3e${d}`, at: posLog(d + Math.log10(3), a) });
  }
  return out;
}

/** Cases that Reflex has scored, as `{l: log10 confidence, ok, answered}`. */
export type Scored = { c: ReflexCase; l: number; ok: boolean; answered: boolean };

export function scored(cases: ReflexCase[]): Scored[] {
  return cases.flatMap((c) =>
    c.score ? [{ c, l: c.score.confidence > 0 ? Math.log10(c.score.confidence) : -12, ok: c.score.correct, answered: c.score.answer !== null }] : [],
  );
}

/** What a threshold (log10) would do on the scored cases: Reflex answers those at or above it that it did not abstain on. */
export function metrics(t: number, cases: Scored[]) {
  const answered = cases.filter((c) => c.answered && c.l >= t);
  const wrongN = answered.filter((c) => !c.ok).length;
  return { n: cases.length, answered: answered.length, wrongN, saved: cases.length ? answered.length / cases.length : 0, wrong: answered.length ? wrongN / answered.length : 0 };
}

/** The lowest threshold (log10, step .01) at which at most 5% of Reflex's answers are wrong. */
export function suggest(cases: Scored[], a: Axis): number | null {
  for (let t = a.lo; t <= a.hi + 1e-9; t += 0.01) {
    const m = metrics(t, cases);
    if (m.answered > 0 && m.wrong <= 0.05) return Math.round(t * 100) / 100;
  }
  return null;
}

/** Corpus domains from the repo listing, as `repo::export_corpus` reads them: top-level folders and `.md` files,
 * skipping dotfiles, a top-level README and the archive folder. */
export type Corpus = { domains: { name: string; files: number; bytes: number }[]; files: number; archived: string[]; placeholder: boolean };

export function corpusOf(files: RepoFile[]): Corpus {
  const live = new Map<string, { files: number; bytes: number }>();
  const archived = new Set<string>();
  const md: string[] = [];
  for (const f of files) {
    const segs = f.path.split("/");
    if (!f.path.endsWith(".md") || segs.some((s) => s.startsWith("."))) continue;
    if (segs[0] === ARCHIVE_DIR) {
      if (segs[1]) archived.add(segs[1].replace(/\.md$/, ""));
      continue;
    }
    if (segs.length === 1 && segs[0].toLowerCase() === "readme.md") continue;
    md.push(f.path);
    const name = segs[0].replace(/\.md$/, "");
    const d = live.get(name) ?? { files: 0, bytes: 0 };
    live.set(name, { files: d.files + 1, bytes: d.bytes + Number(f.size) });
  }
  const domains = [...live.entries()].map(([name, d]) => ({ name, ...d })).sort((a, b) => a.name.localeCompare(b.name));
  // The seed corpus a new project starts with is a placeholder, not a corpus.
  return { domains, files: md.length, archived: [...archived].sort(), placeholder: md.length === 1 && md[0] === "example/example.md" };
}

/** The smallest domains over the limit, in the order the server archives them. */
export function smallest(c: Corpus): string[] {
  return [...c.domains]
    .sort((a, b) => a.files - b.files || a.bytes - b.bytes || a.name.localeCompare(b.name))
    .slice(0, Math.max(0, c.domains.length - MAX_DOMAINS))
    .map((d) => d.name);
}

export type Mode = "off" | "nocorpus" | "blocked" | "error" | "noq" | "shadow" | "live";

export function modeOf(info: ProjectInfo, installed: boolean, corpus: Corpus | null): Mode {
  const domains = corpus ? corpus.domains.length : info.reflex.domains.length;
  if (!installed) return "off";
  if (info.reflex.state === "no_corpus" || domains === 0 || corpus?.placeholder) return "nocorpus";
  if (domains > MAX_DOMAINS) return "blocked";
  if (info.reflex.state === "error" || info.reflex.state === "stopped") return "error";
  if (!info.project.questions.length) return "noq";
  if (info.project.threshold === null) return "shadow";
  return "live";
}

export const MODE_BADGE: Record<Mode, [string, string]> = {
  live: ["live", "var(--lane-reflex)"],
  shadow: ["shadow · not calibrated", "var(--lane-llm)"],
  off: ["not installed", "var(--lane-abstain)"],
  nocorpus: ["no corpus", "var(--lane-abstain)"],
  blocked: ["paused · too many domains", "var(--lane-abstain)"],
  error: ["stopped", "var(--lane-abstain)"],
  noq: ["no questions", "var(--lane-llm)"],
};

/** Where questions go when Reflex does not answer: "the LLM", or nobody without a fallback. */
export const toLlm = (fallback: string) => (fallback === "none" ? "nobody (this project has no fallback)" : "the LLM");

/** Who answers below the threshold, in a few words. */
export const fallbackName = (f: string) => (f === "api" ? "Anthropic API" : f === "none" ? "no fallback" : f);

/** Scrolls to a section of the screen, below the sticky top bar. */
export function scrollToSection(id: string) {
  window.setTimeout(() => {
    const el = document.getElementById(id);
    if (el) window.scrollTo({ top: el.getBoundingClientRect().top + window.scrollY - 64, behavior: "smooth" });
  }, 60);
}
