//! Plugin system — `.claude-plugin/plugin.json` bundles of skills +
//! hooks discovered under `~/.leek/plugins/` and `<cwd>/.leek/plugins/`
//! (M2.5 research notes §6, Claude Code parity).
//!
//! A plugin directory layout (matches CC):
//!
//! ```text
//! my-plugin/
//! ├── .claude-plugin/plugin.json   # manifest (required)
//! ├── skills/<name>/SKILL.md       # skills bundled with the plugin (optional)
//! └── hooks/hooks.json             # hook handlers (optional)
//! ```
//!
//! Plugin-supplied skills register at the user layer (their priority is
//! above built-in but below an explicit `~/.leek/skills/` or project
//! skill of the same name). Hooks merge into the engine's set in plugin
//! discovery order, after user-supplied hooks.

mod loader;

pub use loader::load_all;
// Public types reached through the `load_all` return type — re-exports
// aren't strictly needed, but keeping the loader pub-by-design.
#[allow(unused_imports)]
pub use loader::{LoadedPlugin, PluginLoadError};
