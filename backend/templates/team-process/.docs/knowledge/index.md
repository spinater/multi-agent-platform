# Knowledge index

Start here. One row per card: read the card for your area before grepping docs or source.

## Conventions

- A card is a curated map of one area, 40–120 lines and never more than 200. It points at code and docs; it never
  restates a schema or copies code.
- Every card starts with the files it summarises:

  ```md
  ---
  sources:
    - path/to/file-or-folder
  ---
  # Title
  ```

- A commit that changes a card's sources updates the card in the same commit. MAP's Knowledge screen flags a card
  whose sources were committed after it, a card over the cap, a card missing from this index and broken links.
- A card that outgrows the cap is split into focused cards (its own commit), each with narrower `sources:`; old
  content is moved, never silently deleted.
- Agents can also search everything here, plus past tasks' outcomes, through `$MAP_KNOWLEDGE_URL/search`.

## Cards

| Card | Scope |
| --- | --- |
<!-- MAP:FILL cards: one row `| [<title>](<domain or ops>/<file>.md) | <one-line scope> |` per card you write under .docs/knowledge/domain/ or .docs/knowledge/ops/; leave the table empty (delete this comment) when you write none. -->
