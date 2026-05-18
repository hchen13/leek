# Historical Design Archive

This directory is historical reference material.

Current authority lives in:

1. `../docs/REQUIREMENTS.md` — product, UX, and acceptance requirements.
2. `../docs/ARCHITECTURE.md` — end-state architecture.
3. `../docs/MILESTONES.md` — phase order and completion state.
4. Current code — implementation truth.

Do not implement from files in this directory directly. In particular, older
specs here may still mention concepts that were removed from rebuild-clean:

- task entities
- deliverable classification
- user mandate / memory
- portfolio / holdings / decisions in the core vault
- `LlmProvider` trait / provider routing
- plan guard
- Reasoning DAG as a current UI contract

If a future milestone intentionally re-adopts any historical idea, it must be
restated in `docs/REQUIREMENTS.md`, `docs/ARCHITECTURE.md`, or
`docs/MILESTONES.md` first.
