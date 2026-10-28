# Design brief: Knowledge screen

## What it is, in one sentence
A project's memory: the docs its agents read before they work, a check that those docs are still true, and a log
of what the agents looked up.

## Who uses it and why
The owner (one person running many AI coding agents). They open it to answer three questions, in this order:
1. **Is my project's knowledge healthy?** Are there docs that lie (stale), are too long to be useful, or are
   missing from the index? If so, fix them with one click (MAP creates a task for an agent).
2. **What do we know about X?** Search, then read a doc.
3. **Are agents finding what they need?** Which searches agents ran, and which ones found nothing (a gap to fill).

Design for question 1 first. Most visits are "glance at health, click fix, leave".

## Words the screen uses (show these definitions in the UI, as hints or an empty state)
- **Card**: a short Markdown note in `.docs/knowledge/` that summarises part of the code for agents. It lists its
  `sources:` (the code files it describes).
- **Index**: `.docs/knowledge/index.md`, the table of contents of all cards. Agents read it first.
- **Stale**: a card's source file changed (git commit) after the card was last updated, so the card may be wrong.
- **Cap**: the maximum length of a card (200 lines; warning at 170). Long cards waste the agent's context.
- **Rulebook**: `CLAUDE.md` / `AGENTS.md`, sent with every agent request. It has a size budget (60 KB; warning at 52 KB).
- **Scratch**: temporary folders like `.scratch`. They are flagged when they are large (over 2 GB) or contain copies
  of the repo (old copies make search return stale docs).
- **MAP memory**: finished/failed tasks and approved plans that MAP saves itself (`map://tasks/12`). Read-only.
- **Fix task**: a ready-written task that MAP queues for an agent to fix one problem.

## Screen structure (top to bottom, by priority)
0. **Suggestion** (only when the project has no knowledge yet). MAP decides whether the project needs knowledge now:
   - **Recommended**: a prominent card above everything: "This project is ready for knowledge cards", the reasons
     ("142 source files, 18 000 lines: more than an agent can read before each task"; "5 tasks done: their lessons are
     only in MAP's memory"), the key numbers, then "What happens if you accept" as a numbered list (one agent task
     surveys the code, writes cards and the index by our conventions, points CLAUDE.md at it, one commit). Two
     buttons: **Set up knowledge** (primary) and **Not now**.
   - **Not yet**: one muted line ("Still small: 12 files, 600 lines. MAP suggests it from 30 files or 3 tasks done").
   - **In progress**: one line with a link to the setup task.
   - **In use / dismissed**: nothing, or a tiny "suggested earlier · show" link.
1. **Header**: project picker, "indexed 1 min ago", Reindex button. Small.
2. **Health summary** (most prominent): one status line, such as "3 problems · 1 error, 2 warnings", or a clear
   "All healthy" state.
   - Problems are grouped by type. Each one shows the file, a plain-language reason ("source `api/auth.rs`
     changed 4 days after this card") and a **Create fix task** button.
   - Groups with many items get one bulk button ("Fix all 12 stale cards").
3. **Budgets**: one horizontal bar per rulebook (size vs warning vs cap) and one per scratch folder. These are
   visual gauges, not tables.
4. **Search**: one input and a kind filter (card, requirement, task, rulebook, doc, memory). Results show the title,
   the path, a highlighted snippet and the kind. Click a result to open the doc viewer.
5. **Browse**: all documents grouped by kind. Cards show lines vs cap, whether they are in the index, and how many
   sources they have. Collapsed by default.
6. **Agent searches**: recent queries with who ran them (agent or owner) and the hit count. Highlight zero-hit
   queries as gaps.

**Doc viewer**: a reading view of one doc, with rendered Markdown. A card's sources and its links open in place.
There is a back button to the screen.

## States to design
- No project yet: "Knowledge lives in a project folder. Create a project first."
- Suggestion recommended (no index, big project), not yet (small project), in progress (setup task running).
- Project with no index: explain the convention in 3 lines and offer "Create index task".
- All healthy.
- Many problems (65 near-cap cards, a 57 KB rulebook, a 6 GB scratch folder). This is the real case for the bistro
  project; it must stay scannable.
- Searching / no results ("Nothing found. If agents search this too, it is a gap: write a card").

## Avoid
- Showing every section with equal weight. Health comes first; Browse is secondary.
- Unexplained jargon (stale, cap, rulebook, scratch): every term above needs a hint the first time it appears.
- Tables for the budgets. Use gauges.
- Editing docs in the UI. Fixes always go through an agent task.

## Data available (API `GET /api/projects/{slug}/knowledge`)
`docs[]` (path, kind, title, lines, bytes, modified, in_index, sources), `counts` by kind, `issues[]` (check,
level, path, detail, fix_title, fix_prompt), `rulebooks[]` (path, bytes, warn, cap),
`scratch[]` (path, bytes, copies[]), `recent_queries[]` (query, by, hits, at), `indexed_at`, `suggestion` (verdict
recommended|not_yet|in_progress|in_use|dismissed, reasons[], stats {source_files, source_lines, done_tasks, docs,
cards, rulebook_bytes}, plan[], task). Accept: `POST …/knowledge/accept`; not now: `POST …/knowledge/dismiss`.
Search: `GET …/knowledge/search?q=&kind=` returns hits with snippet and score.
