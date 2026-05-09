-- 0009_plan_resolution.sql
-- Per design/decisions/0012-plan-resolution-and-budget-recovery.md:
-- lifecycle (pending / in_progress / completed) and resolution
-- (done / blocked / deferred / superseded / satisfied_by_proxy /
--  insufficient_evidence) are different questions. `completed` is the
-- terminal lifecycle state regardless of success; `resolution` carries the
-- why. Pending/in_progress items must have NULL resolution.

ALTER TABLE agent_plan_items ADD COLUMN resolution TEXT;

-- Backfill existing completed rows with `done` so the new invariant holds
-- without rewriting history. Pending / in_progress rows keep NULL.
UPDATE agent_plan_items
SET resolution = 'done'
WHERE status = 'completed' AND resolution IS NULL;
