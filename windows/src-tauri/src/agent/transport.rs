// How a request reaches the model. The loop only sees `Transport`, so tests
// swap the HTTP client for scripted replies. Cancellation is the loop's job
// (it races `send` against the cancel token), not the transport's.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::claude::{self, CallError, Provider};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Transport: Send + Sync {
    fn send<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<Value, CallError>>;
}

/// The real thing: the Claude API or a local llama-server.
pub struct HttpTransport {
    provider: Provider,
    key: Option<String>,
}

impl HttpTransport {
    pub fn new(provider: Provider, key: Option<String>) -> Self {
        HttpTransport { provider, key }
    }
}

impl Transport for HttpTransport {
    fn send<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<Value, CallError>> {
        Box::pin(claude::call(&self.provider, self.key.as_deref(), body))
    }
}

#[cfg(test)]
pub use mock::{MockReply, MockTransport};

#[cfg(test)]
mod mock {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::{json, Value};

    use super::{BoxFuture, Transport};
    use crate::agent::cancel::Cancel;
    use crate::claude::CallError;

    pub enum MockReply {
        Ok(Value),
        Err(CallError),
        /// Cancels the token, then never answers — a request cut mid-flight.
        CancelAndHang(Cancel),
    }

    /// Answers with scripted replies, in order, and records every body sent.
    /// Running out of replies is a test bug: it panics.
    #[derive(Default)]
    pub struct MockTransport {
        replies: Mutex<VecDeque<MockReply>>,
        pub bodies: Mutex<Vec<Value>>,
    }

    impl MockTransport {
        pub fn new(replies: Vec<MockReply>) -> Self {
            MockTransport { replies: Mutex::new(replies.into()), bodies: Mutex::new(Vec::new()) }
        }

        pub fn sent(&self) -> Vec<Value> {
            self.bodies.lock().unwrap().clone()
        }

        /// A Messages API response with these content blocks.
        pub fn raw(stop_reason: &str, content: Vec<Value>) -> MockReply {
            MockReply::Ok(json!({
                "content": content,
                "stop_reason": stop_reason,
                "usage": { "input_tokens": 10, "output_tokens": 5 },
            }))
        }

        /// A plain text answer that ends the turn.
        pub fn text_reply(text: &str) -> MockReply {
            Self::raw("end_turn", vec![json!({ "type": "text", "text": text })])
        }

        /// A response asking for these tool calls: (id, name, input).
        pub fn tool_reply(calls: Vec<(&str, &str, Value)>) -> MockReply {
            let content = calls
                .into_iter()
                .map(|(id, name, input)| json!({ "type": "tool_use", "id": id, "name": name, "input": input }))
                .collect();
            Self::raw("tool_use", content)
        }
    }

    impl Transport for MockTransport {
        fn send<'a>(&'a self, body: &'a Value) -> BoxFuture<'a, Result<Value, CallError>> {
            self.bodies.lock().unwrap().push(body.clone());
            let reply = self.replies.lock().unwrap().pop_front().expect("MockTransport ran out of replies");
            Box::pin(async move {
                match reply {
                    MockReply::Ok(v) => Ok(v),
                    MockReply::Err(e) => Err(e),
                    MockReply::CancelAndHang(cancel) => {
                        cancel.cancel();
                        std::future::pending().await
                    }
                }
            })
        }
    }

    mod tests {
        use super::*;

        fn block_on<F: std::future::Future>(f: F) -> F::Output {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
        }

        #[test]
        fn mock_replays_in_order_and_records_bodies() {
            let err = CallError { message: "boom".into(), status: Some(529), retryable: true, retry_after: None };
            let mock = MockTransport::new(vec![
                MockTransport::text_reply("hi"),
                MockTransport::tool_reply(vec![("t1", "echo", json!({ "a": 1 }))]),
                MockReply::Err(err),
            ]);
            let first = block_on(mock.send(&json!({ "n": 1 }))).unwrap();
            assert_eq!(first["content"][0]["text"], "hi");
            assert_eq!(first["stop_reason"], "end_turn");
            assert_eq!(first["usage"]["input_tokens"], 10);
            let second = block_on(mock.send(&json!({ "n": 2 }))).unwrap();
            assert_eq!(second["stop_reason"], "tool_use");
            assert_eq!(second["content"][0]["name"], "echo");
            let third = block_on(mock.send(&json!({ "n": 3 }))).unwrap_err();
            assert_eq!(third.status, Some(529));
            assert_eq!(mock.sent().len(), 3);
        }

        #[test]
        fn cancel_and_hang_is_cut_by_the_token() {
            let cancel = Cancel::new();
            let mock = MockTransport::new(vec![MockReply::CancelAndHang(cancel.clone())]);
            let body = json!({});
            let out = block_on(cancel.run(mock.send(&body)));
            assert!(out.is_none());
            assert!(cancel.is_cancelled());
        }
    }
}
