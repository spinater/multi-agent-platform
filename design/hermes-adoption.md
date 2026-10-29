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

## 1. Lessons on the Reflex screen

What the corpus learned, newest first: the decision, the domain, who taught it (panel or owner), what Reflex said
before. One click removes a lesson (deletes its line, restarts Reflex). Without this a wrong panel answer teaches
Reflex silently.

- Backend: `GET /projects/{slug}/corpus/lessons`, `DELETE …/lessons/{id}` over `corpus_lessons`.
- Done when: a lesson made on live shows up, and removing it removes its line from `learned.md`.

## 2. Nudge agents to save what they learned (memory nudges)

Hermes reminds its agent to persist knowledge. MAP has knowledge cards but waits for agents to think of it.

- With RAG on, a task's prompt ends with: if you found a fact about this project that the next task would need (a
  trap, a rule, where something lives), save it as a card in the project's card folder (an API to write
  one comes with this step).
- The manager gets the same line every 20 turns.
- Done when: the Knowledge screen shows cards written by agents, and the reads log shows them found later.

## 3. Skills from finished work

Hermes writes a skill after a complex task. In MAP a skill is a proposal, never a silent write into the project.

- When a heavy task passes its audit, a cheap model drafts `.claude/skills/<name>/SKILL.md` (what the task did, the
  steps, the checks) and submits it as a plan; the owner approves or drops it on the Plans screen.
- One draft per kind of task: a draft whose name or steps match an existing skill updates that one instead.
- Done when: an approved draft is loaded by a later task (the Skills screen counts loads).

## 4. Search past sessions

Hermes searches past sessions with FTS5 and summarizes the hits. MAP's knowledge search (FTS5 already) covers docs,
cards, tasks and plans, not what was said in the lanes.

- When a manager or persistent-lane session ends, or each night, a cheap model summarizes its transcript into a
  `session` document (decisions made, owner's words quoted, open questions) and indexes it.
- Searched like any document, inside the RAG switch and the 1,500-token search budget.
- Done when: `map-ask --search` answers "what did we decide about X yesterday" from a session summary.

## 5. Compress a long manager

Hermes has `/compress`. A project's manager runs for days and its context grows.

- The manager's card shows its context size (from the provider's own usage lines); above a limit
  (`[manager] compress_at_tokens`), MAP asks it for a summary, starts a new session, and hands it the summary.
- A Compress button does the same on demand.
- Done when: a manager past the limit restarts with its summary and still answers about earlier decisions.

## Not taken

- Sandboxed terminals (Docker, Modal, Daytona): the agents' isolated logins and the owner's tmux lanes would have to
  move into containers; worth it only when MAP runs agents for other people.
- Trajectory export for training: not MAP's job.
- More chat gateways: Telegram covers the owner; add one only when someone asks.

## Order

1 → 2 → 4 → 3 → 5. Step 1 first because learning without a way to undo is a risk; 2 is a one-line change with a
visible result; 4 and 3 need a cheap summarizing model call each; 5 touches the manager's lifecycle, the riskiest.
