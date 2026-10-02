// Invariants of the message history the loop builds: every tool_use gets its
// tool_result in the very next message, and nothing is left dangling.

use serde_json::{json, Value};

use super::tools::ToolOutput;

const PRUNED: &str = "[old tool output removed to save space]";

/// A copy of `messages` where every tool_result except those in the last
/// message has its content replaced by a short note — for a small local
/// context. The history itself keeps the full outputs.
pub fn prune_old_tool_results(messages: &[Value]) -> Vec<Value> {
    let last = messages.len().saturating_sub(1);
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut m = m.clone();
            if i == last {
                return m;
            }
            if let Some(blocks) = m.get_mut("content").and_then(Value::as_array_mut) {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                        block["content"] = json!([{ "type": "text", "text": PRUNED }]);
                    }
                }
            }
            m
        })
        .collect()
}

/// Ends a turn the step cap cut short: if the history ends on tool results
/// (or the question), the assistant gets the last word with `text`.
pub fn close_turn(messages: &mut Vec<Value>, text: &str) {
    if messages.last().and_then(|m| m.get("role")).and_then(Value::as_str) == Some("user") {
        messages.push(json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] }));
    }
}

/// The tool_result block answering tool_use `id`.
pub fn tool_result(id: &str, out: &ToolOutput) -> Value {
    let content = if out.for_model.is_empty() {
        json!("(no output)")
    } else {
        Value::Array(out.for_model.clone())
    };
    json!({ "type": "tool_result", "tool_use_id": id, "content": content, "is_error": out.is_error })
}

/// Panics unless `messages` is a history the Messages API accepts: starts with
/// the user, roles alternate, each tool_use is answered in the next message,
/// and no tool_result answers nothing.
#[cfg(test)]
pub fn assert_valid_history(messages: &[Value]) {
    let blocks_of = |m: &Value, kind: &str, field: &str| -> Vec<String> {
        m.get("content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some(kind))
                    .map(|b| b[field].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(!messages.is_empty(), "empty history");
    assert_eq!(messages[0]["role"], "user", "history must start with the user");
    for (i, m) in messages.iter().enumerate() {
        let role = m["role"].as_str().expect("message without role");
        if i > 0 {
            assert_ne!(messages[i - 1]["role"].as_str(), Some(role), "roles must alternate at {i}");
        }
        let results = blocks_of(m, "tool_result", "tool_use_id");
        if !results.is_empty() {
            assert_eq!(role, "user", "tool_result outside a user message at {i}");
            let calls = if i > 0 { blocks_of(&messages[i - 1], "tool_use", "id") } else { vec![] };
            for id in &results {
                assert!(calls.contains(id), "orphan tool_result {id} at {i}");
            }
        }
        let calls = blocks_of(m, "tool_use", "id");
        if !calls.is_empty() {
            assert_eq!(role, "assistant");
            let next = messages.get(i + 1).unwrap_or_else(|| panic!("tool_use at {i} is never answered"));
            let answered = blocks_of(next, "tool_result", "tool_use_id");
            for id in &calls {
                assert!(answered.contains(id), "tool_use {id} at {i} has no tool_result");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_last_message_and_the_original() {
        let out = ToolOutput::text("big output", "s");
        let messages = vec![
            json!({ "role": "user", "content": [{ "type": "text", "text": "q" }] }),
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "a", "name": "x", "input": {} }] }),
            json!({ "role": "user", "content": [tool_result("a", &out)] }),
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "b", "name": "x", "input": {} }] }),
            json!({ "role": "user", "content": [tool_result("b", &out)] }),
        ];
        assert_valid_history(&messages);
        let pruned = prune_old_tool_results(&messages);
        assert_eq!(pruned[2]["content"][0]["content"][0]["text"], PRUNED);
        assert_eq!(pruned[4]["content"][0]["content"][0]["text"], "big output");
        assert_eq!(messages[2]["content"][0]["content"][0]["text"], "big output");
    }

    #[test]
    fn close_turn_gives_the_assistant_the_last_word() {
        let mut messages = vec![json!({ "role": "user", "content": "q" })];
        close_turn(&mut messages, "stopped");
        assert_eq!(messages.len(), 2);
        close_turn(&mut messages, "stopped");
        assert_eq!(messages.len(), 2);
        assert_valid_history(&messages);
    }

    #[test]
    fn empty_output_still_answers() {
        let mut out = ToolOutput::text("", "s");
        out.for_model.clear();
        assert_eq!(tool_result("t", &out)["content"], "(no output)");
    }

    #[test]
    #[should_panic(expected = "never answered")]
    fn unanswered_tool_use_is_invalid() {
        assert_valid_history(&[
            json!({ "role": "user", "content": "q" }),
            json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "a", "name": "x", "input": {} }] }),
        ]);
    }
}
