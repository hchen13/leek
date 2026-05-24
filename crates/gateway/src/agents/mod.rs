//! Agent system — subagents addressable by name, dispatched via the `task`
//! tool (M2.7). Architecturally a sibling of, but strictly separate from,
//! the `skills` module (ARCHITECTURE §4.2):
//!
//! - File names: `SKILL.md` vs `AGENT.md`
//! - Directories: `harness/skills/` vs `harness/agents/`
//! - Registries: `SkillRegistry` vs `AgentRegistry`
//! - Invocation: `use_skill(name)` injects a body into the current loop
//!   vs `task(agent_name, input)` spawns a child loop.
//!
//! An AGENT.md is a YAML frontmatter + markdown body. The body is the
//! subagent's full system prompt (the parent's five-section prompt is NOT
//! reused — a subagent is a focused worker with a custom mandate).
//!
//! Three discovery layers (same precedence rule as skills: project > user >
//! built-in):
//!
//! 1. `harness/agents/<name>/AGENT.md`  — built-in, shipped with the repo
//! 2. `~/.leek/agents/<name>/AGENT.md`  — user-global, hand-installed
//! 3. `<cwd>/.leek/agents/<name>/AGENT.md` — project-local
//!
//! The loader reuses the skill loader's YAML splitter (`skills::loader::
//! split_frontmatter` is `pub(crate)`) so the two formats share parser
//! behavior without sharing types.

mod agent_def;
mod loader;

pub use agent_def::{AgentDef, AgentLayer, AgentRegistry};
pub use loader::{builtin_agents_dir, load_layered, user_agents_dir};
// `AgentLoadError` and `expand_home` are reachable via the module
// re-export for callers that need them, but neither is referenced from
// the rest of the crate yet — kept reachable, not eagerly used.
#[allow(unused_imports)]
pub use loader::{expand_home, AgentLoadError};
