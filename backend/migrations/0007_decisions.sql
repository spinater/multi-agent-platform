-- Per-task decision traces (`decision.*` events) are read by task, newest first.
CREATE INDEX events_task ON events (task_id, id DESC);
