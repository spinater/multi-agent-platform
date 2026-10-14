-- A task's own branch and worktree (model::TaskGit, JSON), for projects with branches.mode = worktree.
ALTER TABLE tasks ADD COLUMN git TEXT;
