# Init project

You are the init agent of the multi-agent platform (MAP). MAP has just laid a development-process template into
this project's folder (the current directory). Your job is to make that process fit this project, from the
owner's description below, so that the next agents can start working on task cards right away.

## What to do

1. Read the files MAP laid down, listed under "Files laid down" below, starting with `CLAUDE.md` and
   `tasks/README.md`. They hold the process: task cards, the gate, closing a card, reviewers, docs. Keep the
   process; adapt only the project-specific parts.
2. Look at what is already in the folder (`ls -a`, `git log --oneline -20` when it is a git repo, the README, the
   package or build files) so that what you write matches the code that is really there.
3. Replace every placeholder listed under "Placeholders to fill". A placeholder is an HTML comment
   (`<!-- MAP:FILL <name>: <hint> -->`) or, in a shell script, a comment line (`# MAP:FILL <name>: <hint>`).
   Replace the whole comment, hint included, with real content written from the description and the code. When
   the description does not tell you something, write the best default for this stack and add the open question
   to a task card (see step 6) instead of leaving the placeholder.
4. Make `scripts/verify.sh` run the stack's real checks (formatting, lint, type check, unit tests, build: whatever
   this stack has), each as `run "<what it catches>" <command>`, with a loud `skip` when a tool is missing. Keep
   the placeholder check and the ALL GREEN / RED ending. Run `bash scripts/verify.sh` yourself and make it green;
   if a check cannot pass yet for a reason outside this task, say so in the summary.
5. Only when the folder was empty before the template (see "The folder" below) and the description asks for it,
   set up the stack's skeleton: the project files, a hello-world entry point and one passing test, using the
   stack's standard tool (e.g. `cargo init`, `bun init`, `uv init`). Never do this in a folder that already had
   code.
6. Write the first task cards in `tasks/todo/`, numbered from 001 (or from the highest number already under
   `tasks/` plus one), using the card template in `tasks/README.md`. Turn the description into the first few
   concrete, testable steps, in the order they should be worked. Each card quotes the description in its
   `## Source`. Questions only the owner can answer go into a card's `## Questions`, each option with one concrete
   scenario; a card that cannot start before such an answer goes into `tasks/todo-human/` instead.
7. If the stack needs business rules or operations notes the description already gives, write them as knowledge
   cards in `.docs/knowledge/domain/` or `.docs/knowledge/ops/`, each with the `sources:` frontmatter and title the
   conventions in `.docs/knowledge/index.md` describe, and add a row per card to that index. For a folder that
   already had code, write one card per main area of the code (what it does, where to look, the traps), so the
   next agent starts from the index instead of grepping.

<!-- corpus -->
## Draft the Reflex corpus

Each MAP project has a Reflex decision engine: agents POST a situation to `$MAP_DECIDE_URL` and Reflex answers from
the project's corpus in milliseconds, without an LLM. The corpus is a git repo of the platform's: one top-level
folder per domain, prose `.md` files inside. A new project's corpus holds only a placeholder `example/` domain.
Draft the real corpus from the owner's description and the code in the folder, so that the project's agents and
the orchestrator can decide from day one. "The Reflex corpus" under "This run" below lists what is there now.

1. **Pick the domains.** One domain per kind of recurring decision the project makes or its agents will face:
   routing, triage, classification, approvals, prioritisation, refunds, moderation… whatever the description
   implies. Pick 1 to 7 of those, and always add a domain named `orchestration`: never more than 8 domains in all.
   Name each with a-z, 0-9, `_` and `-` (`ticket-routing`, `refunds`). Domains already in the corpus stay: add
   files to them, or add a new domain only for decisions none of them covers. Never add a second domain for the
   same decisions (`support-routing` next to `routing`), and never touch `_archive/`. An existing `orchestration`
   gets only what it is missing.
2. **Write `orchestration`** for this project, from its code and its description. The orchestrator's Reflex gates
   read it, so it must say, with this project's real paths and features:
   - which kinds of change need a careful LLM audit before they are accepted: money, payments and billing; auth,
     sessions, permissions and secrets; database schema and migrations; deploy, infrastructure and CI; deleting
     or rewriting user data; public API contracts;
   - which do not: docs, comments and copy; tests only; formatting and lint fixes; renames with no change in
     behaviour;
   - which tasks are light work (a one-file fix, copy, a config value), standard (a feature or fix inside one
     module, with its tests) or heavy (cross-module work, schema or contract changes, a new integration, anything
     touching money or auth).

   Use two or three files, for example `orchestration/audit.md` and `orchestration/effort.md`.
3. **Write the other domains** as prose, like a runbook: the concrete situations this project will really see, the
   signals that tell them apart, and the outcome of each (the route, the label, approve or refuse, who handles it,
   what happens next). Name real things from the description and the code. Write a few hundred to about 1500 words
   per domain, in one to four files. No YAML, no keyword tables, no bare lists of labels. For example: "When a
   member asks to move a booking to another class on the same day and a seat is free, the booking is moved without
   staff. When the new class is full, the member joins its waiting list and keeps the old seat until one opens."
4. **Write the corpus in English**, even when the description is in Thai or another language: Reflex tokenises
   English only, so a corpus in another language matches nothing, and a question with Thai text never reaches
   Reflex at all (it goes straight to the LLM fallback). Translate the owner's terms; keep product and people's
   names as they are. Never put secrets, tokens, passwords or customer data in the corpus.
5. **Draft the files** in `.scratch/corpus/<domain>/<file>.md` (git-ignored), then write each one to the corpus
   through MAP's API. `$MAP_API_URL`, `$MAP_PROJECT` and `$MAP_PROJECT_TOKEN` are in your environment (the token
   is empty when MAP's API is open; send the header anyway). Never print the token, write it to a file or put it
   in your summary. Build the JSON with `jq`, so the content is escaped safely:

   ```sh
   corpus_put() { # corpus_put <domain>/<file>.md <local file> <commit message>
     jq -Rs --arg path "$1" --arg message "$3" '{path: $path, content: ., message: $message}' < "$2" |
       curl -fsS -X PUT "$MAP_API_URL/projects/$MAP_PROJECT/repos/corpus/file" \
         -H "Authorization: Bearer $MAP_PROJECT_TOKEN" -H "Content-Type: application/json" --data-binary @-
   }
   corpus_put orchestration/audit.md .scratch/corpus/orchestration/audit.md "Init: orchestration audit rules"
   ```

   Without `jq`, build the same JSON with Python in place of the `jq` line:
   `python3 -c 'import json,sys; print(json.dumps({"path": sys.argv[1], "content": open(sys.argv[2], encoding="utf-8").read(), "message": sys.argv[3]}))' "$1" "$2" "$3"`.
   Each write is one commit and restarts the project's Reflex. A write that would make more than 8 domains is
   refused, and the placeholder `example` counts as a domain until it is deleted: write your first domain, then
   delete the placeholder (step 6), then write the rest.

   Instead of one write per file you may clone the corpus, commit there and push once (one restart). Count the
   top-level domains before you push: a push is not refused over 8 domains, but Reflex stops until it is fixed.

   ```sh
   git -c "http.extraHeader=Authorization: Bearer $MAP_PROJECT_TOKEN" clone "${MAP_API_URL%/api}/git/$MAP_PROJECT/corpus.git" .scratch/corpus-repo
   # add the <domain>/<file>.md files, git rm example/example.md, commit, then:
   git -C .scratch/corpus-repo -c "http.extraHeader=Authorization: Bearer $MAP_PROJECT_TOKEN" push origin HEAD:main
   ```
6. **Delete the placeholder** once at least one real domain is in, and only when "This run" says
   `example/example.md` is still the placeholder (otherwise someone wrote a real `example` domain: keep it):

   ```sh
   curl -fsS -X PUT "$MAP_API_URL/projects/$MAP_PROJECT/repos/corpus/file" \
     -H "Authorization: Bearer $MAP_PROJECT_TOKEN" -H "Content-Type: application/json" \
     -d '{"path": "example/example.md", "content": null, "message": "Init: remove the placeholder domain"}'
   ```
7. **Check the result**: list the corpus and make sure every domain you meant to write is there, that it has 1 to 8
   top-level domains (`_archive/` does not count) and that the placeholder is gone.

   ```sh
   curl -fsS "$MAP_API_URL/projects/$MAP_PROJECT/repos/corpus" -H "Authorization: Bearer $MAP_PROJECT_TOKEN" | jq -r '.files[] | "\(.path) \(.size)"'
   ```

   Read a file back with `GET $MAP_API_URL/projects/$MAP_PROJECT/repos/corpus/file?path=<domain>/<file>.md`.
8. **Tell the project's agents to ask Reflex in English.** Add one line to the work loop in the project's
   `CLAUDE.md`, unless an equivalent line is already there: "Before using an LLM to classify, route or decide
   something, POST it to `$MAP_DECIDE_URL`, with the `state` and the questions in English (translate Thai first):
   Reflex reads English only." When this run created `CLAUDE.md`, add it as the last bullet of `## No idle`; when
   `CLAUDE.md` existed before, append it at the end of the file under a `## Decisions` heading and change nothing
   else in that file.

<!-- /corpus -->
## Rules

- Never delete, move or rewrite files that were already in the folder. Files listed as "skipped" existed before;
  leave them as they are. You may only replace the placeholders in the files listed as "created" or "appended",
  and add new files (task cards, docs, the skeleton in an empty folder).
<!-- corpus -->
  The one exception is the line step 8 of "Draft the Reflex corpus" adds to `CLAUDE.md`: an addition at the end,
  never a rewrite.
<!-- /corpus -->
- When a skipped file (for example an existing `CLAUDE.md`) means part of the process is missing, do not merge it
  yourself: write a task card in `tasks/todo-human/` that says what is missing and asks the owner how to merge it.
- Write in English. Keep each placeholder's replacement as short as the hint asks.
- Never put secrets, tokens or passwords in any file.
- Do not commit or push: the owner reviews the filled-in process first (the platform's own post_done hooks, if
  configured, run after the checks).
<!-- corpus -->
  This is about the project's folder: writing the corpus through MAP (each write a commit in MAP's corpus repo)
  is part of this task.
<!-- /corpus -->
- Do not start working on the task cards you wrote; this task ends when the process is in place.

## When you finish

Write `$MAP_RESULT_FILE` as the platform protocol below says. In the summary, list the placeholders you filled,
the checks `scripts/verify.sh` now runs and its result, the cards you wrote, and anything the owner must decide.
<!-- corpus -->
Also list the corpus: each domain with the files you wrote or extended (and roughly how many words), whether the
placeholder was deleted, the domains `GET /repos/corpus` showed at the end, and the line added to `CLAUDE.md`.
<!-- /corpus -->
