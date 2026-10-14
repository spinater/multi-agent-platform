-- The intake gate's verdict per task (model::TaskIntake, JSON): asked of the project's Reflex before any LLM runs it.
ALTER TABLE tasks ADD COLUMN intake TEXT;
