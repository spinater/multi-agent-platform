# Heavy tasks

A heavy task is ready to start but needs deep reasoning: the cause is unknown, many parts interact, or a mistake is
costly. Use the strongest model.

Hard bugs: a bug with an unknown cause, an intermittent failure, a flaky test, a race condition, a deadlock, a memory
leak, data corruption, a crash in production, wrong results only under load, a production incident to investigate,
a regression nobody can explain, timezone and daylight saving bugs, floating point and rounding errors.

Performance and scale: slow queries, query optimisation, indexing strategy, caching strategy, cache invalidation,
reducing latency, high CPU, high memory, scaling to many users, concurrency, parallelism, async and threading, queues
and background workers under load, rate limiting design.

Risky flows: authentication and session handling, authorization and permissions logic, payment processing, refunds
and billing calculation, encryption and key handling, security vulnerabilities, injection and cross-site scripting
fixes, data migrations that rewrite existing rows, synchronisation between systems, offline sync and conflict
resolution.

Deep changes: a large refactor across modules, changing a core data model, changing a public API contract used
everywhere, designing an algorithm, a state machine, a scheduling or matching engine, a pricing engine, tricky
business rules with many edge cases.

Words that mark it: race, deadlock, leak, intermittent, flaky, unknown cause, investigate, root cause, performance,
optimise, scale, concurrency, security, vulnerability, encryption, algorithm, refactor across, edge cases, tricky.
