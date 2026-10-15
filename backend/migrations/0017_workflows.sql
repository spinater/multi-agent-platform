-- Configure › Workflows: the workflow a task runs (pinned, or chosen by a trigger) and where it is in the graph
-- (model::TaskWorkflowState, JSON), saved at every step so a restart resumes there.
ALTER TABLE tasks ADD COLUMN workflow TEXT;
ALTER TABLE tasks ADD COLUMN workflow_state TEXT;
