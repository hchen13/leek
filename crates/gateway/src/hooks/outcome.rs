//! `HookEvent` enum (the 7 events leek triggers) + `HookOutcome` (what
//! the agent loop does with a hook's verdict).
//!
//! The CC reference exposes ~22 event types. leek implements the subset
//! that maps cleanly onto its agent loop (M2.5 research notes §4):
//!
//! - SessionStart / SessionEnd                — per-session, called by the API layer
//! - UserPromptSubmit                          — before each turn fires
//! - PreToolUse / PostToolUse                  — around every tool dispatch
//! - PreCompact                                — before auto-compaction
//! - Stop                                      — when the turn ends
//!
//! `SubagentStop` / `Notification` are recognized in config (so users can
//! pre-register handlers) but have no trigger point yet — they're scoped
//! for M2.7.

/// The lifecycle events leek can dispatch hooks for. Stringly named to
/// match CC's `hook_event_name` so config files port cleanly between the
/// two ecosystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PreCompact,
    Stop,
    /// Registered but never triggered in this milestone — present so plugin
    /// authors can pre-wire handlers.
    SubagentStop,
    /// Registered but never triggered in this milestone.
    Notification,
}

impl HookEvent {
    /// CC-compatible event name. Used both for matching in configs and
    /// for the `hook_event_name` field in the JSON payload.
    pub fn as_str(&self) -> &'static str {
        match self {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::UserPromptSubmit => "UserPromptSubmit",
            HookEvent::PreToolUse => "PreToolUse",
            HookEvent::PostToolUse => "PostToolUse",
            HookEvent::PreCompact => "PreCompact",
            HookEvent::Stop => "Stop",
            HookEvent::SubagentStop => "SubagentStop",
            HookEvent::Notification => "Notification",
        }
    }

    /// Parse a CC-style event name from a config file. `None` for
    /// unrecognized names so the loader can warn + skip them.
    pub fn from_name(s: &str) -> Option<HookEvent> {
        match s {
            "SessionStart" => Some(HookEvent::SessionStart),
            "SessionEnd" => Some(HookEvent::SessionEnd),
            "UserPromptSubmit" => Some(HookEvent::UserPromptSubmit),
            "PreToolUse" => Some(HookEvent::PreToolUse),
            "PostToolUse" => Some(HookEvent::PostToolUse),
            "PreCompact" => Some(HookEvent::PreCompact),
            "Stop" => Some(HookEvent::Stop),
            "SubagentStop" => Some(HookEvent::SubagentStop),
            "Notification" => Some(HookEvent::Notification),
            _ => None,
        }
    }

    /// `true` for events where a hook's "block" verdict actually cancels
    /// the pending action. For Post*/Stop the action has already been
    /// dispatched, so a block is advisory only (logged, not enforced).
    pub fn can_block(self) -> bool {
        matches!(
            self,
            HookEvent::PreToolUse | HookEvent::UserPromptSubmit | HookEvent::PreCompact
        )
    }
}

/// The verdict returned by running a hook (or the merged verdict from a
/// chain of hooks for the same event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// No hook objected — proceed with the pending action.
    Continue,
    /// At least one hook returned `decision: block` (or exit 2). The agent
    /// loop should cancel the pending action and surface `reason` to the
    /// model so it can recover.
    Block { reason: String },
}

impl HookOutcome {
    pub fn is_block(&self) -> bool {
        matches!(self, HookOutcome::Block { .. })
    }
}
