// What the island hears while a turn runs, on the "agent-event" channel. Only
// short summaries travel here — never the contents of a file or a page.

use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub turn_id: u64,
    #[serde(flatten)]
    pub kind: AgentEventKind,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEventKind {
    TurnStart,
    ToolStart { id: String, name: String, summary: String },
    ToolEnd { id: String, ok: bool, summary: String },
    /// Interim assistant text sent alongside tool calls.
    Text { text: String },
    #[serde(rename_all = "camelCase")]
    Retry { attempt: u32, wait_ms: u64, reason: String },
    /// `stopped`: None, or "max_steps" when the step cap ended the turn.
    Done { text: String, usage: Option<Usage>, stopped: Option<String> },
    Cancelled,
    Error { message: String },
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

/// Collects events for assertions.
#[cfg(test)]
#[derive(Default)]
pub struct VecSink(pub std::sync::Mutex<Vec<AgentEvent>>);

#[cfg(test)]
impl VecSink {
    pub fn kinds(&self) -> Vec<AgentEventKind> {
        self.0.lock().unwrap().iter().map(|e| e.kind.clone()).collect()
    }
}

#[cfg(test)]
impl EventSink for VecSink {
    fn emit(&self, event: AgentEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn to_json(kind: AgentEventKind) -> serde_json::Value {
        serde_json::to_value(AgentEvent { turn_id: 3, kind }).unwrap()
    }

    #[test]
    fn events_serialize_to_the_island_contract() {
        assert_eq!(to_json(AgentEventKind::TurnStart), json!({ "turnId": 3, "type": "turn_start" }));
        assert_eq!(
            to_json(AgentEventKind::ToolStart { id: "toolu_01".into(), name: "read_file".into(), summary: "notes.txt".into() }),
            json!({ "turnId": 3, "type": "tool_start", "id": "toolu_01", "name": "read_file", "summary": "notes.txt" })
        );
        assert_eq!(
            to_json(AgentEventKind::ToolEnd { id: "toolu_01".into(), ok: true, summary: "lines 1-120 of 120".into() }),
            json!({ "turnId": 3, "type": "tool_end", "id": "toolu_01", "ok": true, "summary": "lines 1-120 of 120" })
        );
        assert_eq!(
            to_json(AgentEventKind::Retry { attempt: 1, wait_ms: 1000, reason: "Claude API 529: Overloaded".into() }),
            json!({ "turnId": 3, "type": "retry", "attempt": 1, "waitMs": 1000, "reason": "Claude API 529: Overloaded" })
        );
        assert_eq!(
            to_json(AgentEventKind::Done {
                text: "hi".into(),
                usage: Some(Usage { input_tokens: 812, output_tokens: 95 }),
                stopped: None,
            }),
            json!({ "turnId": 3, "type": "done", "text": "hi", "usage": { "inputTokens": 812, "outputTokens": 95 }, "stopped": null })
        );
        assert_eq!(to_json(AgentEventKind::Cancelled), json!({ "turnId": 3, "type": "cancelled" }));
        assert_eq!(
            to_json(AgentEventKind::Error { message: "x".into() }),
            json!({ "turnId": 3, "type": "error", "message": "x" })
        );
    }

    #[test]
    fn vec_sink_keeps_order() {
        let sink = VecSink::default();
        sink.emit(AgentEvent { turn_id: 1, kind: AgentEventKind::TurnStart });
        sink.emit(AgentEvent { turn_id: 1, kind: AgentEventKind::Cancelled });
        assert_eq!(sink.kinds(), vec![AgentEventKind::TurnStart, AgentEventKind::Cancelled]);
    }
}
