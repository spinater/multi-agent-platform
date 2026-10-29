-- The example line a lesson added, so removing the lesson removes exactly that line (corpus_learn.rs).
ALTER TABLE corpus_lessons ADD COLUMN example TEXT NOT NULL DEFAULT '';
