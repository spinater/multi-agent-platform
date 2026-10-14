# Changes that need a careful audit

A change needs a careful audit when a mistake would cost money, leak access, lose data or be hard to undo.

Money: payments, payment gateway, checkout, billing, invoices, receipts, prices, pricing, discounts, coupons,
refunds, credits, wallet, balances, payouts, commissions, currency conversion, tax, VAT, rounding of amounts.

Access: login, logout, sign up, sessions, cookies, tokens, JWT, passwords, password reset, API keys, OAuth, single
sign-on, two-factor, roles, permissions, access control, admin rights, row level security, rate limits.

Data: database migrations, schema changes, backfills, deleting records, purging, truncating, cascading deletes,
retention, cleanup jobs, overwriting files, imports that replace existing data, personal data, medical records.

Production: deploy scripts, CI pipelines, Dockerfiles, infrastructure, environment variables, secrets, DNS,
certificates, cloud resources, production configuration, feature flags in production.

Security: input validation, escaping, SQL queries, shell commands, file uploads, file paths, encryption, hashing,
signatures, CORS, CSRF, dependency upgrades, anything a security report mentions.

Also a large change across many modules, or one whose summary says a check was skipped, a test was disabled or
something could not be verified.
