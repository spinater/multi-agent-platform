# Working on this project

Read this file before every task. `AGENTS.md` and `GEMINI.md` only point here: this is the one copy of the rules,
so change them here and nowhere else.

## What this is

<!-- MAP:FILL what-this-is: two to four sentences. What the product does, for whom, and the one or two things it must never do. Plain words, no marketing. -->

## Read before touching

| If you touch | Read first |
| --- | --- |
<!-- MAP:FILL read-before-touching: table rows `| <path or area> | <doc, card or file to read first> |`, one per risky area of this codebase (money, auth, schema, deploy, public API). Point at files that exist, or at .docs/knowledge/... files you create. -->

## Invariants

Rules that hold after every change. Breaking one is a bug even when every test passes.

<!-- MAP:FILL invariants: a bullet list of the project's own invariants (data that must never be lost or duplicated, numbers that must add up, who may see what). Each bullet says where it is enforced or tested. -->

## Stack and commands

<!-- MAP:FILL stack-and-commands: the language, framework and package manager; then a short list of the real commands: install, run locally, the gate (bash scripts/verify.sh), and slow suites (db, e2e) with when they are required. -->

## Deploy

<!-- MAP:FILL deploy: how a change reaches users (or "not deployed yet"), who may deploy, and the command or pipeline. Never put secrets here. -->

## Tasks

Work is tracked as task cards, files in `tasks/`. The full rules and the card template are in `tasks/README.md`.

- `tasks/todo/`: the queue, worked in number order.
- `tasks/todo-human/`: blocked on a person. Only when you can quote who must answer what; move the card back to
  `todo/` once it is answered. It is not a place for work that is merely hard.
- `tasks/done/`: shipped, with the commit and the verification that was actually run.
- Name cards `NNN-short-slug.md`. NNN is the highest number anywhere under `tasks/` plus one; a card split off
  another gets a letter suffix (`017b`).

## Closing a card (every step, every time)

1. A test covers the logic the card adds. No runnable test means the card cannot close.
2. Run the gate: `bash scripts/verify.sh`. If it is RED, report the actual output and leave the card where it is.
3. Write the commands you actually ran, and their result, into the card's `## Verification`. Green means the real
   command passed, not that it should pass.
4. `git mv` the card to `tasks/done/` (`bash scripts/task-move.sh NNN done`).
5. One commit for the card, its message ending `(task NNN)`.
6. Push immediately. Finished work is never left uncommitted or unpushed.

## No idle

- Card done and gate green: start the next card in queue order.
- Blocked on a person: move the card to `todo-human/` with the exact question, then work on another card.

## The gate: scripts/verify.sh

- Kept small and fast enough to run before every commit. Each check says what it catches.
- A check that cannot run is skipped loudly, never silently.
- It ends with ALL GREEN or RED and exits non-zero on RED.
- Slow suites (database, end-to-end) live outside the gate, but they are required when you touch their area.

## Reviewers

Review is chosen by the cost of being wrong, not run on every change.

- A diff that touches a path in the list below must go through `code-reviewer` before the card closes.
- Auth, credentials and deploy changes also go through `security-reviewer`.
- Schema, contract and module-boundary changes: draft them in the card first, have `architect` review the draft,
  then apply.
- "Is this what was asked?" is `sa-requirements`; writing and running tests is `qa-tester`.
- Output from sub-agents goes through the same gate as your own.

### Review path list

<!-- MAP:FILL review-paths: a bullet list of paths or globs whose changes need code-reviewer (and which also need security-reviewer or architect), from the project's money, auth, schema and deploy code. -->

## Questions to the owner

Every option gets one short, concrete scenario of what happens if it is picked. A question without scenarios is
not ready to ask. Questions go into the card's `## Questions`.

## Docs

- `.docs/knowledge/index.md`: **start here**. One row per knowledge card; read your area's card before grepping.
  Cards list their `sources:` in frontmatter, stay under 200 lines, and are updated in the same commit as their
  sources (see the conventions at the top of the index).
- Search the project's knowledge and past tasks' outcomes with `$MAP_KNOWLEDGE_URL/search?q=…` (header
  `Authorization: Bearer $MAP_PROJECT_TOKEN`) before re-deriving something.
- `.docs/knowledge/domain/`: business rules. Changing a rule means updating its doc in the same commit.
- `.docs/knowledge/ops/`: how things run (environments, jobs, accounts, limits).
- `.docs/design/`: designs and decisions that span several cards.
- `.docs/roadmap/YYYY-MM-DD-<topic>.md`: plans, dated.
- `.docs/runbook/`: step-by-step fixes for things that go wrong.

## Scratch

Scratch files go in `.scratch/` (git-ignored), never in `/tmp`. Never copy the repo into it: a stale copy's docs
turn up in every grep. Delete what you no longer need.

## Status file

At the end of every turn, write `.scratch/agent-status.json`:

```json
{"card": "NNN", "state": "working", "need_clear": false, "note": "one line on where you are", "at": "<ISO 8601 time>"}
```

`state` is `working`, `done` or `blocked`; `need_clear` is true when the context should be cleared before the next card.

## Agents

Defined for every CLI: `.claude/agents/`, `.codex/agents/` and `.agents/agents/`.

| Agent | Use it for | Writes files |
| --- | --- | --- |
| code-reviewer | Diffs that touch the review path list (required there) | no |
| security-reviewer | Auth, credentials, secrets, deploy | no |
| architect | Schema, contracts, module boundaries; drafts before they are applied | no |
| qa-tester | Writing and running tests for a card | yes |
| sa-requirements | Checking a change against what was asked, quoted from the card's Source | no |
