// Chat client: multi-turn chat with web search, and files sent as
// document/image/text blocks — against the Claude API, or against a local model
// (llama-server) that speaks the same Messages API without tools or vision.
//
// Everything happens here rather than in the island: the API key never leaves
// the Credential Manager, and file bytes never cross the IPC boundary.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::secrets;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Server-side fallback: on a policy decline the API retries the same request on
/// a fallback model inside the same call, so the island never shows a dead end.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 4096;
/// Text and code files are inlined; anything larger is skipped.
const MAX_INLINE_TEXT: u64 = 200_000;

pub const DEFAULT_MODEL: &str = "claude-opus-5";
pub const DEFAULT_LOCAL_ENDPOINT: &str = "http://127.0.0.1:8080/v1/messages";

/// A local model sees a few kilobytes, not a few hundred: llama-server gives
/// each slot CTX_SIZE / PARALLEL tokens (16384 / 2 by default).
const LOCAL_MAX_INLINE_TEXT: u64 = 12_000;
const LOCAL_CONTEXT_TOKENS: usize = 8192;
/// Tokens kept free for the answer.
const LOCAL_ANSWER_RESERVE: usize = 1536;
/// Deliberately pessimistic, so the estimate errs on the side of fitting.
const CHARS_PER_TOKEN: usize = 3;
/// How much JSON-encoded history a local request may carry.
const LOCAL_CONTEXT_CHARS: usize =
    (LOCAL_CONTEXT_TOKENS - LOCAL_ANSWER_RESERVE) * CHARS_PER_TOKEN - LOCAL_SYSTEM_PROMPT.len();

const SYSTEM_PROMPT: &str = "You are Mochi, a personal AI assistant living at the top of the user's screen. \
You have web search access and can help with absolutely anything — research, coding, finding places, recommendations, tasks, questions. \
Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

/// Same persona without the web search claim — told it can search, a local
/// model makes searches up.
const LOCAL_SYSTEM_PROMPT: &str = "You are Mochi, a personal AI assistant living at the top of the user's screen. \
You run locally on the user's computer and have no internet access: never claim to have searched or browsed. \
If something needs current information, say so. Help with coding, questions, writing and tasks. \
Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

/// Who answers the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    Local { endpoint: String },
}

impl Provider {
    pub fn from_settings(settings: &crate::settings::Settings) -> Self {
        if settings.provider == "local" {
            Provider::Local { endpoint: settings.local_endpoint.trim().to_string() }
        } else {
            Provider::Anthropic
        }
    }
}

#[derive(Default)]
pub struct Chat {
    /// Full multi-turn history, including tool_use / tool_result blocks.
    messages: Mutex<Vec<Value>>,
}

impl Chat {
    pub fn reset(&self) {
        self.messages.lock().unwrap().clear();
    }

    fn is_empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }

    fn push(&self, message: Value) {
        self.messages.lock().unwrap().push(message);
    }

    fn pop(&self) {
        self.messages.lock().unwrap().pop();
    }

    fn snapshot(&self) -> Vec<Value> {
        self.messages.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File { name: String, path: String },
    Window { app_name: String, title: String, url: Option<String> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
}

/// One chat turn. Returns the assistant's text, or a message the island shows
/// in the note view.
pub async fn send(
    chat: &Chat,
    provider: &Provider,
    model: &str,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let key = match provider {
        Provider::Anthropic => Some(
            secrets::get("anthropic-api-key")
                .ok_or_else(|| "API key missing. Open settings.".to_string())?,
        ),
        // Only a llama-server started with --api-key wants one.
        Provider::Local { .. } => secrets::get("local-api-key"),
    };

    let mut content: Vec<Value> = Vec::new();

    // File / window context rides along with the first message only, exactly
    // like ClaudeService.chat().
    if chat.is_empty() {
        match &context {
            Some(ChatContext::File { name, path }) => {
                if let Some(block) = file_block(path, provider)? {
                    content.push(block);
                }
                content.push(json!({ "type": "text", "text": format!("File: {name}") }));
            }
            Some(ChatContext::Window { app_name, title, url }) => {
                let mut text = format!("Context — App: {app_name}, Window: {title}");
                if let Some(url) = url {
                    text.push_str(&format!(", URL: {url}"));
                }
                content.push(json!({ "type": "text", "text": text }));
            }
            None => {}
        }
    }
    content.push(json!({ "type": "text", "text": query }));

    chat.push(json!({ "role": "user", "content": content }));

    let messages = match provider {
        Provider::Anthropic => chat.snapshot(),
        // The full history stays in `chat`; only what is sent gets trimmed.
        Provider::Local { .. } => match trim_for_budget(&chat.snapshot(), LOCAL_CONTEXT_CHARS) {
            Ok(m) => m,
            Err(err) => {
                chat.pop();
                return Err(err);
            }
        },
    };
    let body = request_body(provider, model, messages);

    let response = match call(provider, key.as_deref(), &body).await {
        Ok(v) => v,
        Err(err) => {
            chat.pop(); // keep the history consistent with what the model saw
            return Err(err);
        }
    };

    // A policy decline comes back as HTTP 200 with stop_reason "refusal".
    if response.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        chat.pop();
        let why = response
            .get("stop_details")
            .and_then(|d| d.get("explanation"))
            .and_then(Value::as_str)
            .unwrap_or("Claude declined this one.");
        return Err(why.to_string());
    }

    let Some(blocks) = response.get("content").and_then(Value::as_array).cloned() else {
        chat.pop();
        return Err("Unexpected API response.".into());
    };

    // Store the whole content — tool_use / tool_result blocks included — so the
    // next turn has the right context.
    chat.push(json!({ "role": "assistant", "content": blocks.clone() }));

    let text = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("No response text.".into());
    }
    Ok(ChatReply { text })
}

/// The local body leaves out what only Anthropic's servers do: web search and
/// server-side fallbacks.
fn request_body(provider: &Provider, model: &str, messages: Vec<Value>) -> Value {
    match provider {
        Provider::Anthropic => json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": SYSTEM_PROMPT,
            "tools": [{ "type": "web_search_20260209", "name": "web_search", "max_uses": 5 }],
            "fallbacks": "default",
            "messages": messages,
        }),
        Provider::Local { .. } => json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": LOCAL_SYSTEM_PROMPT,
            "messages": messages,
        }),
    }
}

/// Drops the oldest user + assistant turns until the history fits in `budget`
/// characters of JSON, so it still starts with a user message. The last
/// message — the one just asked — is never dropped.
fn trim_for_budget(messages: &[Value], budget: usize) -> Result<Vec<Value>, String> {
    let size = |m: &Value| m.to_string().len();
    let mut total: usize = messages.iter().map(size).sum();
    let mut start = 0;
    while total > budget {
        if messages.len() - start <= 1 {
            return Err("Too long for the local model's context. Try a shorter message or file.".into());
        }
        total -= size(&messages[start]) + size(&messages[start + 1]);
        start += 2;
    }
    Ok(messages[start..].to_vec())
}

async fn call(provider: &Provider, key: Option<&str>, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    let endpoint = match provider {
        Provider::Anthropic => ENDPOINT,
        Provider::Local { endpoint } => endpoint.as_str(),
    };
    let mut request = client
        .post(endpoint)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("x-api-key", key);
    }
    if *provider == Provider::Anthropic {
        request = request.header("anthropic-beta", FALLBACK_BETA);
    }

    let response = request.json(body).send().await.map_err(|e| match provider {
        Provider::Local { endpoint } if e.is_connect() || e.is_builder() => {
            format!("Can't reach the local model at {endpoint}. Is llama-server running?")
        }
        _ => format!("Network error: {e}"),
    })?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        // Surface the API's own message, which is what makes a bad key obvious.
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        let who = match provider {
            Provider::Anthropic => "Claude API",
            Provider::Local { .. } => "Local model",
        };
        return Err(format!("{who} {status}: {detail}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))
}

/// PDF → document block, image → image block, text/code → inline text.
/// A local model has no vision and silently ignores documents, so there a PDF
/// goes through pdftotext and an image is refused up front.
fn file_block(path: &str, provider: &Provider) -> Result<Option<Value>, String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let media_type = match ext.as_str() {
        "pdf" => Some(("document", "application/pdf")),
        "jpg" | "jpeg" => Some(("image", "image/jpeg")),
        "png" => Some(("image", "image/png")),
        "gif" => Some(("image", "image/gif")),
        "webp" => Some(("image", "image/webp")),
        _ => None,
    };

    let local = matches!(provider, Provider::Local { .. });
    match media_type {
        Some(("image", _)) if local => {
            return Err("Images need a model with vision — the local model can't read them.".into());
        }
        Some(("document", _)) if local => return pdf_text(path).map(Some),
        Some((block_type, media)) => {
            let Ok(bytes) = std::fs::read(path) else { return Ok(None) };
            return Ok(Some(json!({
                "type": block_type,
                "source": { "type": "base64", "media_type": media, "data": base64(&bytes) },
            })));
        }
        None => {}
    }

    let Ok(meta) = std::fs::metadata(path) else { return Ok(None) };
    if local && meta.len() > LOCAL_MAX_INLINE_TEXT {
        return Err(too_big_for_local());
    }
    if meta.len() > MAX_INLINE_TEXT {
        return Ok(None);
    }
    let Ok(text) = std::fs::read_to_string(path) else { return Ok(None) };
    Ok(Some(json!({ "type": "text", "text": format!("File contents:\n{text}") })))
}

fn too_big_for_local() -> String {
    format!(
        "This file is too big for the local model (over {} KB of text).",
        LOCAL_MAX_INLINE_TEXT / 1000
    )
}

/// The text layer of a PDF, via poppler's pdftotext when it is installed.
fn pdf_text(path: &str) -> Result<Value, String> {
    let mut cmd = std::process::Command::new("pdftotext");
    cmd.args(["-layout", "-enc", "UTF-8", path, "-"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .map_err(|_| "Install pdftotext (poppler) to read PDFs with the local model.".to_string())?;
    if !out.status.success() {
        return Err("pdftotext couldn't read this PDF.".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Err("This PDF has no text layer (scanned?), so the local model can't read it.".into());
    }
    if text.len() as u64 > LOCAL_MAX_INLINE_TEXT {
        return Err(too_big_for_local());
    }
    Ok(json!({ "type": "text", "text": format!("File contents:\n{text}") }))
}

/// Small standalone base64 encoder — not worth another dependency.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> Provider {
        Provider::Local { endpoint: DEFAULT_LOCAL_ENDPOINT.into() }
    }

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("coucou-claude-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn local_body_has_no_web_search_or_fallbacks() {
        let body = request_body(&local(), "m", vec![]);
        assert!(body.get("tools").is_none());
        assert!(body.get("fallbacks").is_none());
        assert!(!body["system"].as_str().unwrap().contains("web search"));

        let body = request_body(&Provider::Anthropic, "m", vec![]);
        assert_eq!(body["tools"][0]["name"], "web_search");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["system"], SYSTEM_PROMPT);
    }

    #[test]
    fn trim_drops_whole_turns_from_the_front() {
        let turn = |role: &str, n: usize| json!({ "role": role, "content": "x".repeat(n) });
        let messages = vec![turn("user", 100), turn("assistant", 100), turn("user", 100), turn("assistant", 100), turn("user", 10)];
        let size = |m: &Value| m.to_string().len();

        // Everything fits: untouched.
        let all: usize = messages.iter().map(size).sum();
        assert_eq!(trim_for_budget(&messages, all).unwrap().len(), 5);

        // One byte short: the first turn goes, a user message still leads.
        let trimmed = trim_for_budget(&messages, all - 1).unwrap();
        assert_eq!(trimmed.len(), 3);
        assert_eq!(trimmed[0]["role"], "user");
        assert_eq!(trimmed[0]["content"], "x".repeat(100));

        // Only the last question fits.
        let trimmed = trim_for_budget(&messages, size(&messages[4])).unwrap();
        assert_eq!(trimmed, vec![messages[4].clone()]);

        // Not even that.
        assert!(trim_for_budget(&messages, 5).is_err());
    }

    #[test]
    fn local_refuses_images_and_large_text() {
        let png = temp_file("pic.png", b"\x89PNG");
        assert!(file_block(png.to_str().unwrap(), &local()).is_err());
        assert!(file_block(png.to_str().unwrap(), &Provider::Anthropic).unwrap().is_some());

        let big = temp_file("big.txt", &vec![b'a'; LOCAL_MAX_INLINE_TEXT as usize + 1]);
        assert!(file_block(big.to_str().unwrap(), &local()).is_err());
        assert!(file_block(big.to_str().unwrap(), &Provider::Anthropic).unwrap().is_some());

        let small = temp_file("small.rs", b"fn main() {}");
        let block = file_block(small.to_str().unwrap(), &local()).unwrap().unwrap();
        assert_eq!(block["type"], "text");
    }

    #[test]
    fn local_reads_pdfs_as_text() {
        // A one-page PDF with "Hello Mochi" on it.
        let pdf = b"%PDF-1.1\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 100]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n\
4 0 obj<</Length 41>>stream\nBT /F1 18 Tf 20 40 Td (Hello Mochi) Tj ET\nendstream endobj\n\
5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n\
trailer<</Root 1 0 R>>\n%%EOF\n";
        let path = temp_file("doc.pdf", pdf);
        match file_block(path.to_str().unwrap(), &local()) {
            Ok(Some(block)) => {
                assert_eq!(block["type"], "text");
                assert!(block["text"].as_str().unwrap().contains("Hello Mochi"));
            }
            // No poppler here: the refusal must say so.
            Err(err) if std::process::Command::new("pdftotext").arg("-v").output().is_err() => {
                assert!(err.contains("pdftotext"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        // Claude reads the PDF itself.
        let block = file_block(path.to_str().unwrap(), &Provider::Anthropic).unwrap().unwrap();
        assert_eq!(block["type"], "document");
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
