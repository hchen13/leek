//! Hook system — user-installed shell commands attached to agent
//! lifecycle events, aligned with Claude Code convention (M2.5 research
//! notes §§3–5).
//!
//! Configured in `~/.leek/config.json` (or each plugin's `hooks/hooks.json`)
//! under a top-level `hooks` field:
//!
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse": [
//!       {
//!         "matcher": "web_fetch",
//!         "hooks": [{ "type": "command", "command": "echo $1 >> /tmp/leek.log" }]
//!       }
//!     ]
//!   }
//! }
//! ```
//!
//! Event payload is fed to each matching hook as JSON on stdin. The hook
//! decides what happens via exit code + optional stdout JSON:
//!
//! - exit 0 + stdout JSON `{"decision":"block","reason":"..."}` → block
//! - exit 0 + stdout JSON `{"hookSpecificOutput":{"permissionDecision":"deny"}}` → block (PreToolUse)
//! - exit 2 → block, with the hook's stderr surfaced to the model
//! - other non-zero exits → non-blocking error (logged only)
//!
//! Blocking is only effective at *Pre* events; *Post* / *Stop* / *End* block
//! signals are advisory in this MVP (logged, but the action has already
//! happened).

mod config;
mod engine;
mod outcome;

pub use config::{load_hooks_file, load_user_hooks, HookSet};
pub use engine::HookEngine;
pub use outcome::{HookEvent, HookOutcome};
