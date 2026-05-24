//! Skill system — lazy-loaded specialized context, aligned with Claude Code
//! `SKILL.md` convention (see `docs/dispatches/M2.5-research-notes.md`).
//!
//! A skill is a `SKILL.md` file with YAML frontmatter + a markdown body.
//! Only the (short) descriptions go into the system prompt at startup; the
//! body is fetched on demand via the `use_skill` tool. This keeps the
//! steady-state prompt small while letting the model pull in deep
//! domain guidance when the task warrants it.
//!
//! Three discovery layers (priority project > user > built-in, all visible):
//!
//! 1. `harness/skills/<name>/SKILL.md`   — built-in, shipped with the repo
//! 2. `~/.leek/skills/<name>/SKILL.md`   — user-global, hand-installed
//! 3. `<cwd>/.leek/skills/<name>/SKILL.md` — project-local
//!
//! Plus plugin-bundled skills (see `crate::plugins`) which register at the
//! user layer.

mod loader;
mod skill;

pub use loader::{builtin_skills_dir, expand_home, load_layered, user_skills_dir};
// `SkillLoadError` is consumed in tests + reached via Display in startup
// logs (main.rs uses `{e}` formatting through `iter()`).
#[allow(unused_imports)]
pub use loader::SkillLoadError;
pub use skill::{Skill, SkillLayer, SkillRegistry};
