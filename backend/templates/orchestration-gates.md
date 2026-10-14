# Orchestration gates

The orchestrator asks this domain two questions before it spends an LLM call. After a task passes its verify
commands: does this change need a careful LLM audit before it is accepted? Before a task starts: is it a light,
standard or heavy task? Edit these rules to match this project; Reflex reads them again after every corpus change.

## Changes that need a careful audit

A change needs a careful audit when a mistake would cost money, leak access or lose data, or would be hard to undo.

Money: payments, billing, invoices, prices, discounts, refunds, credits, balances, payouts, currency conversion, tax.
A change to how an amount is calculated, rounded, charged or stored needs an audit.

Authentication and permissions: login, logout, sessions, cookies, tokens, passwords, API keys, OAuth, single sign-on,
roles, permissions, access control, admin rights, rate limits. A change to who can see or do something needs an audit.

Database schema and migrations: new or changed tables, columns, indexes and constraints, migration files, changes to a
stored data format, backfills. A migration that rewrites existing rows needs an audit.

Deploy and infrastructure: CI pipelines, Dockerfiles, release and deploy scripts, environment variables, secrets,
production configuration, DNS, certificates, cloud resources.

Data deletion: anything that deletes, truncates, purges, overwrites or expires user data or files, cleanup jobs,
retention rules, cascading deletes.

Security: input validation, escaping, SQL queries, shell commands, file uploads and paths, encryption, hashing,
signatures, sandboxing, CORS, dependency upgrades, anything a security report mentions.

A change that is large, touches many modules, or whose summary says a check was skipped, a test was disabled or
something could not be verified needs an audit too.

## Changes that do not need an audit

A change does not need an audit when passing the verify commands is proof enough and a mistake is cheap to fix.

Documentation only: README, guides, the docs folder, changelogs, examples in docs.
Comments and docstrings only.
Copy: user-facing text, wording, labels, typos, translations, error message wording.
Tests only: new or fixed tests that do not change the code under test.
Formatting: whitespace, lint fixes, import order, code style.
Renames: a variable, function, file or component renamed without changing what it does.
Small styling: colours, spacing, icons, fonts.

## Light tasks

A light task is small and mechanical, in one or two files, with no design decisions: fix a typo, change copy or a
label, reword a message, rename something, format code, add a comment or docstring, bump a version string, add one
test, change a colour or spacing, update documentation, fix a lint warning.

## Standard tasks

A standard task is ordinary feature or bug work in a handful of files with some judgement: add an endpoint, a form, a
screen, a command or a setting; fix a bug whose cause is known; add validation; write tests for a module; refactor
one module; wire in a library the project already uses; small performance fixes.

## Heavy tasks

A heavy task needs deep reasoning or touches many parts of the system: design a new subsystem or architecture, a
large refactor across modules, a database migration that changes data, concurrency, caching or performance problems,
a bug with an unknown cause, security fixes, authentication or payment flows, porting code to another language or
framework, anything the task calls tricky, risky or cross-cutting.
