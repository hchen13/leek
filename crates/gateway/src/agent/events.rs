//! The workbench event contract — M1.9.
//!
//! M1 emitted loop events ad hoc. M1.9 turns them into a contract the
//! frontend workbench can consume without guessing (REQUIREMENTS §2, §7.2).
//! Two things live here:
//!
//! 1. **Surface classification.** Every event is consumed by one workbench
//!    surface. `surface_for` maps an event kind to that surface — `chat`,
//!    `canvas`, `right_rail` or `lifecycle`. `AppState::emit` stamps every
//!    payload with its surface from this one table, so the frontend never
//!    hardcodes the map. The three content panels are REQUIREMENTS §2;
//!    `lifecycle` covers the turn-level observability events of §7.2, which
//!    belong to no content panel.
//!
//! 2. **The canvas-artifact envelope.** `note_trace`, `tool_lifecycle` and
//!    `search_lifecycle` all carry a `CanvasArtifact` payload: a stable
//!    `artifact_id`, a lifecycle `phase`, a `canvas_identity` for card
//!    merge / update (§2.4), and a kind-specific `data` object. One shape,
//!    three event kinds.
//!
//! The vocabulary is `kind` constants, not scattered string literals, so the
//! contract has a single greppable source of truth.

/// The event-kind vocabulary. Every event the gateway emits names itself
/// from one of these constants.
pub mod kind {
    // chat surface
    pub const MESSAGE_CREATED: &str = "message_created";
    pub const ASSISTANT_DELTA: &str = "assistant_delta";
    // canvas surface
    pub const NOTE_TRACE: &str = "note_trace";
    pub const TOOL_LIFECYCLE: &str = "tool_lifecycle";
    pub const SEARCH_LIFECYCLE: &str = "search_lifecycle";
    // right-rail surface
    pub const PLAN_UPDATED: &str = "plan_updated";
    // lifecycle surface
    pub const ASSISTANT_DONE: &str = "assistant_done";
    pub const TURN_METRICS_RECORDED: &str = "turn_metrics_recorded";
    pub const COMPACTION_STARTED: &str = "compaction_started";
    pub const COMPACTION_COMPLETED: &str = "compaction_completed";
    pub const ERROR: &str = "error";
}

/// Which workbench surface consumes an event (REQUIREMENTS §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The chat column — user messages, the final reply, the streamed reply.
    Chat,
    /// The canvas — Note Trace and tool / search process cards.
    Canvas,
    /// The right rail — the Plan / TODO widget.
    RightRail,
    /// Turn-level observability (REQUIREMENTS §7.2): metrics, compaction,
    /// lifecycle markers, errors. Not bound to a content panel.
    Lifecycle,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Chat => "chat",
            Surface::Canvas => "canvas",
            Surface::RightRail => "right_rail",
            Surface::Lifecycle => "lifecycle",
        }
    }
}

/// The surface that consumes `event_kind`. An unrecognized kind is treated
/// as `Lifecycle` — a safe default that keeps it out of the content panels.
pub fn surface_for(event_kind: &str) -> Surface {
    match event_kind {
        kind::MESSAGE_CREATED | kind::ASSISTANT_DELTA => Surface::Chat,
        kind::NOTE_TRACE | kind::TOOL_LIFECYCLE | kind::SEARCH_LIFECYCLE => Surface::Canvas,
        kind::PLAN_UPDATED => Surface::RightRail,
        kind::ASSISTANT_DONE
        | kind::TURN_METRICS_RECORDED
        | kind::COMPACTION_STARTED
        | kind::COMPACTION_COMPLETED
        | kind::ERROR => Surface::Lifecycle,
        _ => Surface::Lifecycle,
    }
}

/// Lifecycle phase of a canvas artifact. A note has no running state and is
/// always `Completion`; a tool or search call moves `Start` → `Completion`
/// (or `Start` → `Error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Start,
    Completion,
    Error,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Start => "start",
            Phase::Completion => "completion",
            Phase::Error => "error",
        }
    }
}

/// The unified envelope for a canvas process artifact (REQUIREMENTS §2.2).
/// Serialized as the payload of a `note_trace`, `tool_lifecycle` or
/// `search_lifecycle` event.
#[derive(Debug, Clone)]
pub struct CanvasArtifact {
    /// The turn this artifact belongs to.
    pub turn_id: String,
    /// Stable id — a `Start` frame and its later `Completion` / `Error`
    /// frame share it, so the frontend updates one card.
    pub artifact_id: String,
    /// Lifecycle phase.
    pub phase: Phase,
    /// Card merge / update key (REQUIREMENTS §2.4): two artifacts with the
    /// same identity are the same canvas card.
    pub canvas_identity: String,
    /// The loop iteration that produced it — lets the frontend order
    /// artifacts and correlate a note with its streamed `assistant_delta`s.
    pub iteration: usize,
    /// Kind-specific body (note text, tool display payload, search query).
    pub data: serde_json::Value,
}

impl CanvasArtifact {
    /// Render the envelope as an event payload. `AppState::emit` adds the
    /// `surface` field on top.
    pub fn into_payload(self) -> serde_json::Value {
        serde_json::json!({
            "turn_id": self.turn_id,
            "artifact_id": self.artifact_id,
            "phase": self.phase.as_str(),
            "canvas_identity": self.canvas_identity,
            "iteration": self.iteration,
            "data": self.data,
        })
    }

    /// A Note Trace artifact — the assistant's visible text emitted before a
    /// tool call (REQUIREMENTS §2.3). Notes are instantaneous: one frame,
    /// `Completion`. The identity is the artifact id (a note never merges by
    /// identity; the frontend merges adjacent notes by position).
    pub fn note(turn_id: &str, iteration: usize, text: &str) -> Self {
        let artifact_id = format!("note-{turn_id}-{iteration}");
        Self {
            turn_id: turn_id.to_string(),
            canvas_identity: artifact_id.clone(),
            artifact_id,
            phase: Phase::Completion,
            iteration,
            data: serde_json::json!({ "text": text }),
        }
    }

    /// A tool-call artifact frame (REQUIREMENTS §2.4). `data` is assembled by
    /// `tools::tool_artifact` from the tool's display payload and the UI
    /// registry; this only wraps it in the envelope.
    pub fn tool(
        turn_id: &str,
        iteration: usize,
        call_id: &str,
        canvas_identity: String,
        phase: Phase,
        data: serde_json::Value,
    ) -> Self {
        Self {
            turn_id: turn_id.to_string(),
            artifact_id: format!("tool-{call_id}"),
            phase,
            canvas_identity,
            iteration,
            data,
        }
    }

    /// A provider-side web-search artifact (REQUIREMENTS §4.3). One card per
    /// search call; the call id is both the artifact id and the identity.
    pub fn search(
        turn_id: &str,
        iteration: usize,
        call_id: &str,
        phase: Phase,
        query: Option<&str>,
    ) -> Self {
        Self {
            turn_id: turn_id.to_string(),
            artifact_id: format!("search-{call_id}"),
            phase,
            canvas_identity: format!("search-{call_id}"),
            iteration,
            data: serde_json::json!({ "query": query }),
        }
    }
}

/// Default canvas identity for a tool call (REQUIREMENTS §2.4): the tool name
/// plus its arguments. Two calls with the same identity render as one card; a
/// tool may override this, but M1's tools all use the default.
pub fn default_canvas_identity(tool: &str, args: &serde_json::Value) -> String {
    format!("{tool}::{args}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_for_classifies_each_panel() {
        assert_eq!(surface_for(kind::MESSAGE_CREATED), Surface::Chat);
        assert_eq!(surface_for(kind::ASSISTANT_DELTA), Surface::Chat);
        assert_eq!(surface_for(kind::NOTE_TRACE), Surface::Canvas);
        assert_eq!(surface_for(kind::TOOL_LIFECYCLE), Surface::Canvas);
        assert_eq!(surface_for(kind::SEARCH_LIFECYCLE), Surface::Canvas);
        assert_eq!(surface_for(kind::PLAN_UPDATED), Surface::RightRail);
        assert_eq!(surface_for(kind::TURN_METRICS_RECORDED), Surface::Lifecycle);
        assert_eq!(surface_for(kind::COMPACTION_STARTED), Surface::Lifecycle);
    }

    #[test]
    fn surface_for_unknown_kind_is_lifecycle() {
        assert_eq!(surface_for("something_new"), Surface::Lifecycle);
    }

    #[test]
    fn note_artifact_is_a_completed_frame() {
        let p = CanvasArtifact::note("turn-1", 2, "正在查行情…").into_payload();
        assert_eq!(p["phase"], "completion");
        assert_eq!(p["iteration"], 2);
        assert_eq!(p["data"]["text"], "正在查行情…");
        // A note's identity is its own id — it never merges by identity.
        assert_eq!(p["artifact_id"], p["canvas_identity"]);
    }

    #[test]
    fn tool_and_search_artifacts_keep_a_stable_id_across_phases() {
        let start = CanvasArtifact::tool(
            "t",
            1,
            "call_9",
            "echo::{}".into(),
            Phase::Start,
            serde_json::json!({}),
        );
        let done = CanvasArtifact::tool(
            "t",
            1,
            "call_9",
            "echo::{}".into(),
            Phase::Completion,
            serde_json::json!({}),
        );
        assert_eq!(start.artifact_id, done.artifact_id);

        let s = CanvasArtifact::search("t", 1, "ws_2", Phase::Completion, Some("q")).into_payload();
        assert_eq!(s["artifact_id"], "search-ws_2");
        assert_eq!(s["data"]["query"], "q");
    }

    #[test]
    fn canvas_identity_includes_tool_and_args() {
        let id = default_canvas_identity("web_fetch", &serde_json::json!({ "url": "x" }));
        assert!(id.starts_with("web_fetch::"));
        assert!(id.contains("url"));
    }
}
