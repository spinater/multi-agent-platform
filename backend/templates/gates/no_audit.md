# Changes that do not need an audit

A change does not need an audit when passing the verify commands is proof enough and a mistake is cheap to fix.

Documentation only: README, guides, docs folder, changelog, examples in the docs, comments, docstrings.

Copy only: user-facing text, wording, labels, button text, headings, typos, spelling, translations, error message
wording, placeholder text, tooltips.

Tests only: new tests, fixed tests, renamed tests, test fixtures and snapshots, without changing the code under test.

Formatting only: whitespace, indentation, lint fixes, import order, code style, removing unused imports.

Renames: a variable, a function, a file or a component renamed without changing behaviour.

Small styling: colours, spacing, padding, margins, icons, fonts, alignment, hover states, a small layout tweak on one
screen.
