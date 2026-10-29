# What MAP takes from Hermes Agent

Hermes Agent (Nous Research) is a single self-improving agent: it turns finished work into skills, nudges itself to
save what it learned, searches its past sessions, and compresses its context. MAP runs many agents through provider
CLIs, so it takes the ideas, not the code. Each step below is small, ships alone, and is measured on live before the
next starts.

## 0. The corpus learns (done, 2026-10-29)

`corpus_learn.rs`: a choice answer the panel agreed on, or the owner confirmed, becomes an example line in
`<answer>/learned.md`; Reflex restarts a minute later; 16 held-out lessons fit a first threshold at 90% precision.
Measured on groove: a branch-filter bug Reflex abstained on (confidence 0.004) was answered `bug` at 0.72 after one
lesson on a similar task.

## 1. Lessons on the Reflex screen (done, 2026-10-29)

What the corpus learned, newest first: the decision, the domain, who taught it (panel or owner), what Reflex said
before. One click removes a lesson (deletes its line, restarts Reflex). Without this a wrong panel answer teaches
Reflex silently.

- Backend: `GET /projects/{slug}/corpus/lessons`, `DELETE …/lessons/{id}` over `corpus_lessons`.
- Done when: a lesson made on live shows up, and removing it removes its line from `learned.md`.

## 2. Nudge agents to save what they learned (done, 2026-10-29)

Hermes reminds its agent to persist knowledge. MAP has knowledge cards but waits for agents to think of it.

- With RAG on, a task's prompt ends with: if you found a fact about this project that the next task would need (a
  trap, a rule, where something lives), save it as a card under the index's folder, linked from the index, committed
  with the change (`rag::card_nudge`). Cards are files in the repo, so no API was needed.
- The manager gets the same line in its starting prompt.
- Done when: the Knowledge screen shows cards written by agents, and the reads log shows them found later.

## 3. Skills from finished work (done, 2026-10-29)

Hermes writes a skill after a complex task. In MAP a skill is a proposal, never a silent write into the project.

- When a hard-won task is done (tiered heavy, retried, or sent back by verify, audit or review), MAP submits a
  one-task draft plan: an agent reads the task's record and commits, then writes `.claude/skills/<name>/SKILL.md`,
  improves the skill that already covers it, or changes nothing. No LLM call until the owner approves it.
- At most one skill draft open per project; tasks of the last day only (`skill_drafts.rs`).
- Done when: an approved draft is loaded by a later task (the Skills screen counts loads).

## 4. Search past sessions (done, 2026-10-29)

Hermes searches past sessions with FTS5 and summarizes the hits. MAP's knowledge search (FTS5 already) covers docs,
cards, tasks and plans, not what was said in the lanes.

- The Claude Code transcripts opened in the project's folder (the manager, the lanes; the owner's and MAP's homes)
  are indexed as `map://sessions/<id>`: the owner's and the agent's words only, each message cut to 600 characters
  and masked, the newest 40,000 characters of each of the 40 newest sessions of the last 30 days. No summarizing
  model: FTS5 finds the passage, which is what Hermes's search returns before it summarizes (`session_memory.rs`).
- Searched like any document, inside the RAG switch and the 1,500-token search budget.
- Done when: `map-ask --search` answers "what did we decide about X yesterday" from a session summary.

## 5. Compress a long manager (done, 2026-10-29)

Hermes has `/compress`. A project's manager runs for days and its context grows.

- The Manager panel shows its context (the last reply's tokens in its transcript). Past 120k tokens, or on Compress,
  MAP types `/compact` into it while it is idle and nobody is attached: the CLI summarises and carries on
  (`manager_compact.rs`; Claude Code and Codex).
- Done when: a manager past the limit restarts with its summary and still answers about earlier decisions.

## Not taken

- Sandboxed terminals (Docker, Modal, Daytona): the agents' isolated logins and the owner's tmux lanes would have to
  move into containers; worth it only when MAP runs agents for other people.
- Trajectory export for training: not MAP's job.
- More chat gateways: Telegram covers the owner; add one only when someone asks.

## Order

1 → 2 → 4 → 3 → 5. Step 1 first because learning without a way to undo is a risk; 2 is a one-line change with a
visible result; 4 and 3 need a cheap summarizing model call each; 5 touches the manager's lifecycle, the riskiest.
