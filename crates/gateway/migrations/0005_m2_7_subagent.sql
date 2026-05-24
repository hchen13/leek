-- L.E.E.K vault — M2.7 schema delta (subagent observability).
--
-- M2.7 lands the `task` tool and the subagent loop (ARCHITECTURE §4.2,
-- MILESTONES M2.7). A subagent runs the same agent loop as the main
-- agent — same guards, same hooks, same cost cap — so per-subagent
-- metrics need to land in the same `turn_metrics` table the main agent
-- already writes to, with a couple of fields to indicate the parent
-- linkage. Per ARCHITECTURE §6 ("每张新增字段都必须说明为什么当前阶段必须加入"):
--
--   turn_metrics.parent_turn_id — NULL for main-agent turns, non-NULL for
--     a subagent turn pointing at the parent turn that ran the `task`
--     call. M2.7 is the first milestone in which a single user prompt can
--     produce more than one `turn_metrics` row, so we need a way to walk
--     from a subagent row back to its parent. We do not enforce a FK
--     (the parent row is in the same table, and we already use plain
--     string ids elsewhere) — observability would degrade gracefully
--     if a parent row were ever missing.
--
--   turn_metrics.depth — 0 for main-agent turns, 1 for first-level
--     subagents, 2 for grand-subagents. Stored explicitly rather than
--     re-derived from the parent chain so a single query can filter
--     subagents and the loop can cheaply enforce the depth cap.
--
-- The earlier `stop_reason` vocabulary stays — a subagent's stop_reason
-- shares the same code path with the main agent (it could be cost_cap,
-- doom_loop, end_turn, …) plus one new value the loop now emits:
--   max_depth — the dispatcher refused to spawn deeper than depth=2.
--     (Recorded in turn_metrics for the spawn site; the task tool also
--     surfaces the same outcome to the model as a structured error.)
--
-- Migrations are immutable, so the stop_reason comment in 0002 stays
-- stale — vault/turn_metrics.rs remains the authoritative vocabulary.

ALTER TABLE turn_metrics ADD COLUMN parent_turn_id TEXT;
ALTER TABLE turn_metrics ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;

-- Look up a turn's subagents by parent — the workbench groups subagent
-- cards under their parent turn and a single index keeps that lookup
-- O(log n) instead of O(n).
CREATE INDEX idx_turn_metrics_parent ON turn_metrics (parent_turn_id);
