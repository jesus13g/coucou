// The agent loop behind Mochi's chat: a turn runs model calls and local tool
// rounds until the model answers in text, the user stops it, or a step cap is
// hit. Nothing here depends on Tauri, so the whole loop runs in unit tests
// against a mock transport and fake tools.
//
// The shape follows Hermes' `_run_conversation_turn`: one iteration is
// begin → prepare → assemble → preflight → call (with retries) → normalize →
// tool round or final text.

pub mod cancel;
pub mod events;
pub mod history;
pub mod tools;
pub mod transport;

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::claude::{self, Provider};
use cancel::Cancel;
use events::{AgentEvent, AgentEventKind, EventSink, Usage};
use tools::{cap_output, canonical, decide, Decision, PolicyState, Registry, Risk, ToolCtx, ToolOutput};
use transport::Transport;

pub struct AgentConfig {
    pub max_steps: u32,
    pub max_retries: u32,
    pub retry_base: Duration,
    pub retry_cap: Duration,
    pub max_tool_calls_per_round: usize,
    /// Per tool result, in characters.
    pub max_output_chars: usize,
    /// Characters of JSON a request may carry (local models only).
    pub context_budget: Option<usize>,
}

impl AgentConfig {
    pub fn for_provider(provider: &Provider) -> Self {
        let local = matches!(provider, Provider::Local { .. });
        AgentConfig {
            max_steps: 25,
            max_retries: 3,
            retry_base: Duration::from_secs(1),
            retry_cap: Duration::from_secs(30),
            max_tool_calls_per_round: 8,
            max_output_chars: if local { 6_000 } else { 30_000 },
            context_budget: local.then_some(claude::LOCAL_CONTEXT_CHARS),
        }
    }
}

pub struct TurnInput {
    pub turn_id: u64,
    pub provider: Provider,
    pub model: String,
    pub history: Vec<Value>,
    pub user_message: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Done { text: String, usage: Usage, stopped: Option<String> },
    Cancelled,
    Failed(String),
}

pub struct TurnResult {
    pub outcome: TurnOutcome,
    /// The history to keep (`Some`), or nothing to commit: a cancelled or
    /// failed turn is rolled back whole.
    pub messages: Option<Vec<Value>>,
}

enum Flow {
    Proceed,
    Continue,
    Break,
    Return(TurnOutcome),
}

struct TurnState {
    turn_id: u64,
    provider: Provider,
    model: String,
    /// History plus this turn so far — the source of truth.
    messages: Vec<Value>,
    step: u32,
    max_steps: u32,
    usage: Usage,
    request: Option<Value>,
    response: Option<Value>,
    /// The last response stopped on "tool_use".
    wants_tools: bool,
    /// False after a local model rejected the tools.
    tools_enabled: bool,
    policy: PolicyState,
    last_interim_text: Option<String>,
    /// Tool name → runs, for the log line.
    tool_runs: Vec<(String, u32)>,
}

impl TurnState {
    fn emit(&self, events: &dyn EventSink, kind: AgentEventKind) {
        events.emit(AgentEvent { turn_id: self.turn_id, kind });
    }

    fn count_run(&mut self, name: &str) {
        match self.tool_runs.iter_mut().find(|(n, _)| n == name) {
            Some((_, count)) => *count += 1,
            None => self.tool_runs.push((name.to_string(), 1)),
        }
    }
}

pub async fn run_turn(
    input: TurnInput,
    cfg: &AgentConfig,
    transport: &dyn Transport,
    tools: &Registry,
    events: &dyn EventSink,
    cancel: &Cancel,
) -> TurnResult {
    let mut messages = input.history;
    messages.push(input.user_message);
    let mut st = TurnState {
        turn_id: input.turn_id,
        provider: input.provider,
        model: input.model,
        messages,
        step: 0,
        max_steps: cfg.max_steps,
        usage: Usage::default(),
        request: None,
        response: None,
        wants_tools: false,
        tools_enabled: true,
        policy: PolicyState::default(),
        last_interim_text: None,
        tool_runs: Vec::new(),
    };
    st.emit(events, AgentEventKind::TurnStart);

    loop {
        match begin_iteration(&mut st, cfg, cancel) {
            Flow::Break => break,
            Flow::Return(o) => return finalize_turn(st, Some(o), events),
            _ => {}
        }
        prepare_iteration(&mut st);
        assemble_request(&mut st, tools);
        if let Flow::Return(o) = preflight_gate(&mut st, cfg) {
            return finalize_turn(st, Some(o), events);
        }
        match api_retry_loop(&mut st, cfg, transport, events, cancel).await {
            Flow::Return(o) => return finalize_turn(st, Some(o), events),
            Flow::Continue => continue,
            _ => {}
        }
        match normalize_response(&mut st, events) {
            Flow::Return(o) => return finalize_turn(st, Some(o), events),
            Flow::Continue => continue, // pause_turn
            _ => {}
        }
        let flow = if st.wants_tools {
            run_tool_round(&mut st, cfg, tools, events, cancel).await
        } else {
            finish_text_response(&st)
        };
        match flow {
            Flow::Continue | Flow::Proceed => continue,
            Flow::Break => break,
            Flow::Return(o) => return finalize_turn(st, Some(o), events),
        }
    }
    finalize_turn(st, None, events)
}

fn begin_iteration(st: &mut TurnState, cfg: &AgentConfig, cancel: &Cancel) -> Flow {
    if cancel.is_cancelled() {
        return Flow::Return(TurnOutcome::Cancelled);
    }
    if st.step >= cfg.max_steps {
        return Flow::Break;
    }
    st.step += 1;
    Flow::Proceed
}

fn prepare_iteration(_st: &mut TurnState) {
    // phase 2: steering / doom-loop detection
}

fn assemble_request(st: &mut TurnState, tools: &Registry) {
    let schemas = if st.tools_enabled { tools.schemas() } else { Vec::new() };
    st.request = Some(claude::request_body(&st.provider, &st.model, st.messages.clone(), &schemas));
}

/// Local models only: prune old tool outputs and trim old turns from what is
/// sent, so the request fits. The history itself is left whole.
fn preflight_gate(st: &mut TurnState, cfg: &AgentConfig) -> Flow {
    let Some(budget) = cfg.context_budget else { return Flow::Proceed };
    let Some(request) = st.request.as_mut() else { return Flow::Proceed };
    let tools_len = request.get("tools").map_or(0, |t| t.to_string().len());
    let pruned = history::prune_old_tool_results(&st.messages);
    match claude::trim_for_budget(&pruned, budget.saturating_sub(tools_len)) {
        Ok(messages) => {
            request["messages"] = Value::Array(messages);
            Flow::Proceed
        }
        Err(e) => Flow::Return(TurnOutcome::Failed(e)),
    }
}

async fn api_retry_loop(
    st: &mut TurnState,
    cfg: &AgentConfig,
    transport: &dyn Transport,
    events: &dyn EventSink,
    cancel: &Cancel,
) -> Flow {
    let body = st.request.clone().unwrap_or(Value::Null);
    let mut attempt: u32 = 0;
    loop {
        let Some(result) = cancel.run(transport.send(&body)).await else {
            return Flow::Return(TurnOutcome::Cancelled);
        };
        let err = match result {
            Ok(response) => {
                st.response = Some(response);
                return Flow::Proceed;
            }
            Err(e) => e,
        };
        if err.retryable && attempt < cfg.max_retries {
            let backoff = cfg.retry_base.saturating_mul(2u32.saturating_pow(attempt));
            attempt += 1;
            let wait = err.retry_after.unwrap_or(backoff).min(cfg.retry_cap);
            st.emit(
                events,
                AgentEventKind::Retry { attempt, wait_ms: wait.as_millis() as u64, reason: err.message.clone() },
            );
            if cancel.run(tokio::time::sleep(wait)).await.is_none() {
                return Flow::Return(TurnOutcome::Cancelled);
            }
            continue;
        }
        // llama-server without --jinja refuses `tools`: carry on without them.
        if err.status == Some(400)
            && matches!(st.provider, Provider::Local { .. })
            && st.tools_enabled
            && st.step == 1
        {
            st.tools_enabled = false;
            log_line("agent: local model rejected tools; retrying without");
            return Flow::Continue;
        }
        return Flow::Return(TurnOutcome::Failed(err.message));
    }
}

fn block_type(b: &Value) -> &str {
    b.get("type").and_then(Value::as_str).unwrap_or("")
}

fn normalize_response(st: &mut TurnState, events: &dyn EventSink) -> Flow {
    let response = st.response.take().unwrap_or(Value::Null);
    let stop = response.get("stop_reason").and_then(Value::as_str).unwrap_or("").to_string();

    // A policy decline comes back as HTTP 200 with stop_reason "refusal".
    if stop == "refusal" {
        let why = response
            .get("stop_details")
            .and_then(|d| d.get("explanation"))
            .and_then(Value::as_str)
            .unwrap_or("Claude declined this one.");
        return Flow::Return(TurnOutcome::Failed(why.to_string()));
    }
    let Some(mut blocks) = response.get("content").and_then(Value::as_array).cloned() else {
        return Flow::Return(TurnOutcome::Failed("Unexpected API response.".into()));
    };
    if let Some(usage) = response.get("usage") {
        st.usage.input_tokens += usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        st.usage.output_tokens += usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    }

    // Server-side web searches already ran: report them after the fact.
    for block in blocks.iter().filter(|b| block_type(b) == "server_tool_use") {
        let id = block.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let name = block.get("name").and_then(Value::as_str).unwrap_or("web_search").to_string();
        let query = block.get("input").and_then(|i| i.get("query")).and_then(Value::as_str).unwrap_or("");
        let failed = blocks.iter().any(|b| {
            block_type(b) == "web_search_tool_result"
                && b.get("tool_use_id").and_then(Value::as_str) == Some(id.as_str())
                && b.get("content").and_then(|c| c.get("type")).and_then(Value::as_str)
                    == Some("web_search_tool_result_error")
        });
        st.emit(events, AgentEventKind::ToolStart { id: id.clone(), name, summary: query.to_string() });
        let summary = if failed { "search failed" } else { "" }.to_string();
        st.emit(events, AgentEventKind::ToolEnd { id, ok: !failed, summary });
    }

    // A tool_use that won't be run (cut by max_tokens, or alongside end_turn)
    // must not stay in the history without its tool_result.
    if stop != "tool_use" {
        blocks.retain(|b| block_type(b) != "tool_use");
    }
    if blocks.is_empty() {
        return Flow::Return(TurnOutcome::Failed("No response text.".into()));
    }
    let interim = joined_text(&blocks);

    // Persist before running anything. After a pause_turn the continuation
    // belongs to the same assistant message.
    let continues = st.messages.last().and_then(|m| m.get("role")).and_then(Value::as_str) == Some("assistant");
    if continues {
        if let Some(content) = st.messages.last_mut().and_then(|m| m["content"].as_array_mut()) {
            content.extend(blocks);
        }
    } else {
        st.messages.push(json!({ "role": "assistant", "content": blocks }));
    }

    if stop == "pause_turn" {
        return Flow::Continue;
    }
    if stop == "tool_use" && !interim.is_empty() {
        st.emit(events, AgentEventKind::Text { text: interim.clone() });
        st.last_interim_text = Some(interim);
    }
    st.wants_tools = stop == "tool_use";
    Flow::Proceed
}

fn joined_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter(|b| block_type(b) == "text")
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

async fn run_tool_round(
    st: &mut TurnState,
    cfg: &AgentConfig,
    tools: &Registry,
    events: &dyn EventSink,
    cancel: &Cancel,
) -> Flow {
    // Only client tools; server_tool_use blocks already ran on Anthropic's side.
    let calls: Vec<(String, String, Value)> = st
        .messages
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| block_type(b) == "tool_use")
                .map(|b| {
                    (
                        b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                        b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                        b.get("input").cloned().unwrap_or(Value::Null),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if calls.is_empty() {
        return finish_text_response(st);
    }

    let mut cache: HashMap<String, ToolOutput> = HashMap::new();
    let mut results = Vec::with_capacity(calls.len());
    for (i, (id, name, input)) in calls.into_iter().enumerate() {
        if cancel.is_cancelled() {
            return Flow::Return(TurnOutcome::Cancelled);
        }
        if i >= cfg.max_tool_calls_per_round {
            let out = ToolOutput::error("Too many tool calls in one step; this one was skipped.");
            results.push(history::tool_result(&id, &out));
            continue;
        }
        // The same call twice in one response runs once; each id still gets
        // its own tool_result.
        let key = format!("{name}\0{}", canonical(&input));
        if let Some(out) = cache.get(&key) {
            results.push(history::tool_result(&id, out));
            continue;
        }

        let out = match tools.get(&name) {
            None => {
                let available = tools.names().join(", ");
                st.emit(events, AgentEventKind::ToolStart { id: id.clone(), name: name.clone(), summary: String::new() });
                ToolOutput::error(format!("Unknown tool `{name}`. Available tools: {available}."))
            }
            Some(tool) => {
                st.emit(
                    events,
                    AgentEventKind::ToolStart { id: id.clone(), name: name.clone(), summary: tool.summarize(&input) },
                );
                if !input.is_object() {
                    ToolOutput::error("Tool input must be a JSON object.")
                } else {
                    let risk = tool.risk(&input);
                    match decide(risk, &st.policy) {
                        Decision::Deny(why) => ToolOutput::error(why),
                        Decision::Allow => {
                            let Some(mut out) = cancel.run(tool.run(input.clone(), ToolCtx { cancel })).await else {
                                return Flow::Return(TurnOutcome::Cancelled);
                            };
                            cap_output(&mut out, cfg.max_output_chars);
                            st.count_run(&name);
                            if risk == Risk::Read && !out.is_error {
                                st.policy.read_local = true;
                            }
                            out
                        }
                    }
                }
            }
        };
        st.emit(events, AgentEventKind::ToolEnd { id: id.clone(), ok: !out.is_error, summary: out.summary.clone() });
        results.push(history::tool_result(&id, &out));
        cache.insert(key, out);
    }
    st.messages.push(json!({ "role": "user", "content": results }));
    Flow::Continue
}

fn finish_text_response(st: &TurnState) -> Flow {
    let text = st
        .messages
        .last()
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|blocks| joined_text(blocks))
        .unwrap_or_default();
    if text.is_empty() {
        return Flow::Return(TurnOutcome::Failed("No response text.".into()));
    }
    Flow::Return(TurnOutcome::Done { text, usage: st.usage.clone(), stopped: None })
}

/// `outcome` None means the step cap ended the turn.
fn finalize_turn(mut st: TurnState, outcome: Option<TurnOutcome>, events: &dyn EventSink) -> TurnResult {
    let outcome = outcome.unwrap_or_else(|| {
        let text = format!("I stopped after {} steps without finishing. Ask me to continue.", st.max_steps);
        history::close_turn(&mut st.messages, &text);
        TurnOutcome::Done { text, usage: st.usage.clone(), stopped: Some("max_steps".into()) }
    });

    let label = match &outcome {
        TurnOutcome::Done { stopped: Some(s), .. } => s.as_str(),
        TurnOutcome::Done { .. } => "done",
        TurnOutcome::Cancelled => "cancelled",
        TurnOutcome::Failed(_) => "failed",
    };
    let runs: Vec<String> = st.tool_runs.iter().map(|(n, c)| format!("{n}×{c}")).collect();
    log_line(&format!(
        "agent turn {}: {} steps, tools [{}], outcome {label}",
        st.turn_id,
        st.step,
        runs.join(", ")
    ));

    let (kind, messages) = match &outcome {
        TurnOutcome::Done { text, usage, stopped } => (
            AgentEventKind::Done { text: text.clone(), usage: Some(usage.clone()), stopped: stopped.clone() },
            Some(std::mem::take(&mut st.messages)),
        ),
        TurnOutcome::Cancelled => (AgentEventKind::Cancelled, None),
        TurnOutcome::Failed(message) => (AgentEventKind::Error { message: message.clone() }, None),
    };
    st.emit(events, kind);
    TurnResult { outcome, messages }
}

/// Counts and outcomes only — never message, file or page contents. Tests keep
/// the user's log file untouched.
fn log_line(line: &str) {
    #[cfg(not(test))]
    crate::log::line(line);
    #[cfg(test)]
    let _ = line;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::claude::CallError;
    use events::VecSink;
    use history::assert_valid_history;
    use tools::Tool;
    use transport::{BoxFuture, MockReply, MockTransport};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    /// A configurable fake tool that counts its runs.
    struct Fake {
        name: &'static str,
        risk: Risk,
        runs: Arc<AtomicUsize>,
        self_cancel: bool,
    }

    impl Fake {
        fn new(name: &'static str, risk: Risk) -> (Self, Arc<AtomicUsize>) {
            let runs = Arc::new(AtomicUsize::new(0));
            (Fake { name, risk, runs: runs.clone(), self_cancel: false }, runs)
        }
    }

    impl Tool for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn schema(&self) -> Value {
            json!({ "name": self.name, "description": "fake", "input_schema": { "type": "object" } })
        }
        fn risk(&self, _: &Value) -> Risk {
            self.risk
        }
        fn summarize(&self, _: &Value) -> String {
            format!("{} summary", self.name)
        }
        fn run<'a>(&'a self, args: Value, ctx: ToolCtx<'a>) -> BoxFuture<'a, ToolOutput> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if self.self_cancel {
                    ctx.cancel.cancel();
                    std::future::pending::<()>().await;
                }
                ToolOutput::text(format!("ran with {}", canonical(&args)), "ran")
            })
        }
    }

    fn echo() -> (Registry, Arc<AtomicUsize>) {
        let (tool, runs) = Fake::new("echo", Risk::Read);
        (Registry::new(vec![Box::new(tool)]), runs)
    }

    fn cfg() -> AgentConfig {
        AgentConfig {
            retry_base: Duration::ZERO,
            retry_cap: Duration::ZERO,
            ..AgentConfig::for_provider(&Provider::Anthropic)
        }
    }

    fn local() -> Provider {
        Provider::Local { endpoint: claude::DEFAULT_LOCAL_ENDPOINT.into() }
    }

    fn input(provider: Provider) -> TurnInput {
        TurnInput {
            turn_id: 1,
            provider,
            model: "m".into(),
            history: vec![],
            user_message: json!({ "role": "user", "content": [{ "type": "text", "text": "hi" }] }),
        }
    }

    struct Run {
        result: TurnResult,
        sink: VecSink,
    }

    fn run_with(provider: Provider, cfg: &AgentConfig, transport: &MockTransport, tools: &Registry, cancel: &Cancel) -> Run {
        let sink = VecSink::default();
        let result = block_on(run_turn(input(provider), cfg, transport, tools, &sink, cancel));
        if let Some(messages) = &result.messages {
            assert_valid_history(messages);
        }
        Run { result, sink }
    }

    fn run(transport: &MockTransport, tools: &Registry) -> Run {
        run_with(Provider::Anthropic, &cfg(), transport, tools, &Cancel::new())
    }

    fn done_text(outcome: &TurnOutcome) -> &str {
        match outcome {
            TurnOutcome::Done { text, .. } => text,
            other => panic!("not done: {other:?}"),
        }
    }

    fn retryable(status: u16) -> MockReply {
        MockReply::Err(CallError {
            message: format!("Claude API {status}: Overloaded"),
            status: Some(status),
            retryable: true,
            retry_after: None,
        })
    }

    fn server_search(id: &str, query: &str) -> Vec<Value> {
        vec![
            json!({ "type": "server_tool_use", "id": id, "name": "web_search", "input": { "query": query } }),
            json!({ "type": "web_search_tool_result", "tool_use_id": id, "content": [] }),
        ]
    }

    #[test]
    fn run_turn_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![]);
        let sink = VecSink::default();
        let cancel = Cancel::new();
        let cfg = cfg();
        let fut = run_turn(input(Provider::Anthropic), &cfg, &transport, &tools, &sink, &cancel);
        assert_send(&fut);
    }

    #[test]
    fn text_reply_finishes_in_one_step() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![MockTransport::text_reply("hello")]);
        let r = run(&transport, &tools);
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        let names: Vec<&str> = sent[0]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"echo"));
        assert_eq!(
            r.sink.kinds(),
            vec![
                AgentEventKind::TurnStart,
                AgentEventKind::Done {
                    text: "hello".into(),
                    usage: Some(Usage { input_tokens: 10, output_tokens: 5 }),
                    stopped: None
                },
            ]
        );
        let messages = r.result.messages.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "assistant");
    }

    #[test]
    fn one_tool_round_then_text() {
        let (tools, runs) = echo();
        let transport = MockTransport::new(vec![
            MockTransport::tool_reply(vec![("t1", "echo", json!({ "x": 1 }))]),
            MockTransport::text_reply("done"),
        ]);
        let r = run(&transport, &tools);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(
            r.sink.kinds(),
            vec![
                AgentEventKind::TurnStart,
                AgentEventKind::ToolStart { id: "t1".into(), name: "echo".into(), summary: "echo summary".into() },
                AgentEventKind::ToolEnd { id: "t1".into(), ok: true, summary: "ran".into() },
                AgentEventKind::Done {
                    text: "done".into(),
                    usage: Some(Usage { input_tokens: 20, output_tokens: 10 }),
                    stopped: None
                },
            ]
        );
        let sent = transport.sent();
        let last = sent[1]["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"][0]["type"], "tool_result");
        assert_eq!(last["content"][0]["tool_use_id"], "t1");
        assert_eq!(r.result.messages.unwrap().len(), 4);
    }

    #[test]
    fn unknown_tool_returns_is_error_and_continues() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![
            MockTransport::tool_reply(vec![("t1", "nope", json!({}))]),
            MockTransport::text_reply("sorry"),
        ]);
        let r = run(&transport, &tools);
        assert_eq!(done_text(&r.result.outcome), "sorry");
        let messages = r.result.messages.unwrap();
        let result = &messages[2]["content"][0];
        assert_eq!(result["is_error"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("Unknown tool `nope`"));
        assert!(r.sink.kinds().contains(&AgentEventKind::ToolEnd {
            id: "t1".into(),
            ok: false,
            summary: ToolOutput::error("Unknown tool `nope`. Available tools: echo.").summary
        }));
    }

    #[test]
    fn step_cap_stops_and_closes_history() {
        let (tools, runs) = echo();
        let transport = MockTransport::new(
            (0..3)
                .map(|i| MockTransport::tool_reply(vec![(["a", "b", "c"][i], "echo", json!({ "i": i }))]))
                .collect(),
        );
        let cfg = AgentConfig { max_steps: 3, ..cfg() };
        let r = run_with(Provider::Anthropic, &cfg, &transport, &tools, &Cancel::new());
        assert_eq!(transport.sent().len(), 3);
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        match &r.result.outcome {
            TurnOutcome::Done { text, stopped, .. } => {
                assert_eq!(stopped.as_deref(), Some("max_steps"));
                assert!(text.contains("3 steps"));
            }
            other => panic!("{other:?}"),
        }
        let messages = r.result.messages.unwrap();
        assert_eq!(messages.last().unwrap()["role"], "assistant");
        assert!(matches!(r.sink.kinds().last(), Some(AgentEventKind::Done { stopped: Some(_), .. })));
    }

    #[test]
    fn cancel_during_tool_rolls_back() {
        let (mut tool, runs) = Fake::new("hang", Risk::Read);
        tool.self_cancel = true;
        let tools = Registry::new(vec![Box::new(tool)]);
        let transport = MockTransport::new(vec![MockTransport::tool_reply(vec![("t1", "hang", json!({}))])]);
        let r = run(&transport, &tools);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(r.result.outcome, TurnOutcome::Cancelled);
        assert!(r.result.messages.is_none());
        assert_eq!(r.sink.kinds().last(), Some(&AgentEventKind::Cancelled));
    }

    #[test]
    fn cancel_during_api_call() {
        let (tools, _) = echo();
        let cancel = Cancel::new();
        let transport = MockTransport::new(vec![MockReply::CancelAndHang(cancel.clone())]);
        let r = run_with(Provider::Anthropic, &cfg(), &transport, &tools, &cancel);
        assert_eq!(r.result.outcome, TurnOutcome::Cancelled);
        assert!(r.result.messages.is_none());
        assert_eq!(r.sink.kinds(), vec![AgentEventKind::TurnStart, AgentEventKind::Cancelled]);
    }

    #[test]
    fn identical_calls_run_once() {
        let (tools, runs) = echo();
        let transport = MockTransport::new(vec![
            MockTransport::tool_reply(vec![
                ("t1", "echo", json!({ "a": 1, "b": 2 })),
                ("t2", "echo", json!({ "b": 2, "a": 1 })),
            ]),
            MockTransport::text_reply("ok"),
        ]);
        let r = run(&transport, &tools);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let messages = r.result.messages.unwrap();
        let results = messages[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool_use_id"], "t1");
        assert_eq!(results[1]["tool_use_id"], "t2");
        assert_eq!(results[0]["content"], results[1]["content"]);
        let kinds = r.sink.kinds();
        assert_eq!(kinds.iter().filter(|k| matches!(k, AgentEventKind::ToolStart { .. })).count(), 1);
        assert_eq!(kinds.iter().filter(|k| matches!(k, AgentEventKind::ToolEnd { .. })).count(), 1);
    }

    #[test]
    fn retries_overloaded_then_succeeds() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![retryable(529), MockTransport::text_reply("back")]);
        let r = run(&transport, &tools);
        assert_eq!(done_text(&r.result.outcome), "back");
        assert_eq!(transport.sent().len(), 2);
        let kinds = r.sink.kinds();
        assert_eq!(
            kinds[1],
            AgentEventKind::Retry { attempt: 1, wait_ms: 0, reason: "Claude API 529: Overloaded".into() }
        );
        assert!(matches!(kinds[2], AgentEventKind::Done { .. }));
    }

    #[test]
    fn retries_exhausted_fails() {
        let (tools, _) = echo();
        let transport = MockTransport::new((0..4).map(|_| retryable(503)).collect());
        let r = run(&transport, &tools);
        assert_eq!(transport.sent().len(), 4);
        assert_eq!(r.result.outcome, TurnOutcome::Failed("Claude API 503: Overloaded".into()));
        assert!(r.result.messages.is_none());
        let kinds = r.sink.kinds();
        assert_eq!(kinds.iter().filter(|k| matches!(k, AgentEventKind::Retry { .. })).count(), 3);
        assert!(matches!(kinds.last(), Some(AgentEventKind::Error { .. })));
    }

    #[test]
    fn pause_turn_is_resent() {
        let (tools, runs) = echo();
        let transport = MockTransport::new(vec![
            MockTransport::raw("pause_turn", server_search("s1", "rust news")),
            MockTransport::text_reply("found it"),
        ]);
        let r = run(&transport, &tools);
        assert_eq!(done_text(&r.result.outcome), "found it");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        // Resent as is: the paused assistant message is last, no new user turn.
        assert_eq!(sent[1]["messages"].as_array().unwrap().last().unwrap()["role"], "assistant");
        assert!(r.sink.kinds().contains(&AgentEventKind::ToolStart {
            id: "s1".into(),
            name: "web_search".into(),
            summary: "rust news".into()
        }));
        // The continuation joins the paused message.
        let messages = r.result.messages.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn server_tool_blocks_are_not_executed() {
        let (tools, runs) = echo();
        let mut content = server_search("s1", "q");
        content.push(json!({ "type": "tool_use", "id": "t1", "name": "echo", "input": {} }));
        let transport = MockTransport::new(vec![MockTransport::raw("tool_use", content), MockTransport::text_reply("ok")]);
        let r = run(&transport, &tools);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let messages = r.result.messages.unwrap();
        let results = messages[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["tool_use_id"], "t1");
    }

    #[test]
    fn refusal_fails_without_commit() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![MockTransport::raw("refusal", vec![])]);
        let r = run(&transport, &tools);
        assert_eq!(r.result.outcome, TurnOutcome::Failed("Claude declined this one.".into()));
        assert!(r.result.messages.is_none());
        assert!(matches!(r.sink.kinds().last(), Some(AgentEventKind::Error { .. })));
    }

    #[test]
    fn stray_tool_use_on_end_turn_is_dropped() {
        let (tools, runs) = echo();
        let transport = MockTransport::new(vec![MockTransport::raw(
            "end_turn",
            vec![
                json!({ "type": "text", "text": "partial" }),
                json!({ "type": "tool_use", "id": "t1", "name": "echo", "input": {} }),
            ],
        )]);
        let r = run(&transport, &tools);
        assert_eq!(done_text(&r.result.outcome), "partial");
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        let messages = r.result.messages.unwrap();
        assert_eq!(messages[1]["content"].as_array().unwrap().len(), 1);

        // Nothing but a cut tool_use: no text to show, nothing committed.
        let transport = MockTransport::new(vec![MockTransport::raw(
            "max_tokens",
            vec![json!({ "type": "tool_use", "id": "t1", "name": "echo", "input": {} })],
        )]);
        let r = run(&transport, &tools);
        assert_eq!(r.result.outcome, TurnOutcome::Failed("No response text.".into()));
        assert!(r.result.messages.is_none());
    }

    #[test]
    fn external_denied_after_local_read() {
        let (read, read_runs) = Fake::new("readlike", Risk::Read);
        let (fetch, fetch_runs) = Fake::new("fetchlike", Risk::External);
        let tools = Registry::new(vec![Box::new(read), Box::new(fetch)]);
        let transport = MockTransport::new(vec![
            MockTransport::tool_reply(vec![("t1", "readlike", json!({})), ("t2", "fetchlike", json!({}))]),
            MockTransport::text_reply("ok"),
        ]);
        let r = run(&transport, &tools);
        assert_eq!(read_runs.load(Ordering::SeqCst), 1);
        assert_eq!(fetch_runs.load(Ordering::SeqCst), 0);
        let messages = r.result.messages.unwrap();
        let denied = &messages[2]["content"][1];
        assert_eq!(denied["tool_use_id"], "t2");
        assert_eq!(denied["is_error"], true);
        assert!(denied["content"][0]["text"].as_str().unwrap().contains("keep your files private"));
    }

    #[test]
    fn local_preflight_prunes_old_tool_results() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![
            MockTransport::tool_reply(vec![("t1", "echo", json!({ "n": 1 }))]),
            MockTransport::tool_reply(vec![("t2", "echo", json!({ "n": 2 }))]),
            MockTransport::text_reply("ok"),
        ]);
        let cfg = AgentConfig { context_budget: Some(100_000), ..cfg() };
        let r = run_with(local(), &cfg, &transport, &tools, &Cancel::new());
        let sent = transport.sent();
        assert_eq!(sent.len(), 3);
        let body = sent[2]["messages"].as_array().unwrap();
        let old = body[2]["content"][0]["content"][0]["text"].as_str().unwrap();
        let fresh = body[4]["content"][0]["content"][0]["text"].as_str().unwrap();
        assert_eq!(old, "[old tool output removed to save space]");
        assert!(fresh.starts_with("ran with"));
        let messages = r.result.messages.unwrap();
        assert!(messages[2]["content"][0]["content"][0]["text"].as_str().unwrap().starts_with("ran with"));
    }

    #[test]
    fn local_preflight_too_big_fails() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![]);
        let cfg = AgentConfig { context_budget: Some(10), ..cfg() };
        let r = run_with(local(), &cfg, &transport, &tools, &Cancel::new());
        assert!(matches!(&r.result.outcome, TurnOutcome::Failed(m) if m.contains("Too long")));
        assert!(r.result.messages.is_none());
        assert!(transport.sent().is_empty());
    }

    #[test]
    fn local_400_retries_without_tools() {
        let (tools, _) = echo();
        let transport = MockTransport::new(vec![
            MockReply::Err(CallError {
                message: "Local model 400 Bad Request: tools not supported".into(),
                status: Some(400),
                retryable: false,
                retry_after: None,
            }),
            MockTransport::text_reply("plain"),
        ]);
        let cfg = AgentConfig { context_budget: None, ..cfg() };
        let r = run_with(local(), &cfg, &transport, &tools, &Cancel::new());
        assert_eq!(done_text(&r.result.outcome), "plain");
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].get("tools").is_some());
        assert!(sent[1].get("tools").is_none());

        // Anthropic doesn't get that second chance.
        let transport = MockTransport::new(vec![MockReply::Err(CallError {
            message: "Claude API 400: bad".into(),
            status: Some(400),
            retryable: false,
            retry_after: None,
        })]);
        let r = run(&transport, &tools);
        assert_eq!(r.result.outcome, TurnOutcome::Failed("Claude API 400: bad".into()));
    }
}
