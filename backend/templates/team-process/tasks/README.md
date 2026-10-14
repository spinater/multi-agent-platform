# Task cards

Each piece of work is one card, a Markdown file in one of three folders:

| Folder | Holds |
| --- | --- |
| `todo/` | The queue, worked in number order. |
| `todo-human/` | Cards blocked on a person. Only when you can quote who must answer what. Move the card back to `todo/` once it is answered. Not a place for work that is merely hard. |
| `done/` | Shipped cards, with their commit and the verification that was actually run. |

## Naming

`NNN-short-slug.md`, e.g. `014-export-invoices.md`.

- NNN is the highest number anywhere under `tasks/` (all three folders) plus one, zero-padded to three digits.
- A card split off another keeps its number with a letter suffix: `017b-retry-webhooks.md`.

## Moving a card

`bash scripts/task-move.sh NNN <todo|todo-human|done>` runs `git mv` and updates the card's status line. Never copy
a card; there is exactly one file per card.

## Closing a card

1. A test covers the logic the card adds. No runnable test means the card cannot close.
2. `bash scripts/verify.sh` is green. If it is RED, put the actual output in the card and leave it where it is.
3. `## Verification` lists the commands actually run and their result.
4. Move the card to `done/`, set `commit:` once the commit exists.
5. One commit for the card, its message ending `(task NNN)`; push immediately.

## Questions

Each option gets one short, concrete scenario of what happens if it is picked. A question with no scenario is not
ready to ask.

## Card template

```markdown
# NNN — <title>

- status: todo
- commit: <SHA once shipped>

## Source

Who asked, and their words, quoted.

## Goal / Scope

What changes and why. What this card does NOT do.

## Decisions

- <decision> — <who decided>, <YYYY-MM-DD>

## Questions

- <question>
  - A: <option> → <what happens if picked, one concrete scenario>
  - B: <option> → <what happens if picked>

## Verification

Commands actually run and their result. Green means the real command passed, not that it should pass.
```

`status` is `todo`, `todo-human` or `done`. A card that is partly done says so on the status line:
`- status: ✅ part A done · ⏸ part B waiting on <who>`.
