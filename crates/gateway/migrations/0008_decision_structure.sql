-- 0008_decision_structure.sql
-- Promote the decision contract from "ticker + direction + rationale" to a
-- shape that actually forces an auditable thesis: required risks, opposing
-- case, invalidation conditions. corpus_refs_json + mandate_check_json
-- already exist as columns; the tool will start writing them.

ALTER TABLE decisions ADD COLUMN risks_json TEXT;
ALTER TABLE decisions ADD COLUMN opposing_case_md TEXT;
ALTER TABLE decisions ADD COLUMN invalidation_conditions_md TEXT;
