// web_fetch: the text of a public web page, by its exact URL. GET only, no
// cookies, no private or local hosts, text content types only.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::Url;
use serde_json::{json, Value};

use super::{Risk, Tool, ToolCtx, ToolOutput};
use crate::agent::transport::BoxFuture;

const MAX_URL_LEN: usize = 2048;
const MAX_BODY_BYTES: usize = 1_500_000;

pub struct WebFetch;

/// A URL the tool may open: http(s), a host, not local or private.
pub(crate) fn check_url(raw: &str) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.len() > MAX_URL_LEN {
        return Err("That URL is too long.".into());
    }
    let url = Url::parse(raw).map_err(|_| format!("Not a valid URL: {raw}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https pages can be opened.".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("That URL has no host.".into());
    }
    if is_blocked_host(&url) {
        return Err("Mochi doesn't open local or private network addresses.".into());
    }
    Ok(url)
}

/// Local names and literal private / loopback / link-local addresses. A public
/// name that resolves to a private address is not caught here.
pub(crate) fn is_blocked_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return true };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => blocked_v4(v4),
            IpAddr::V6(v6) => blocked_v6(v6),
        };
    }
    host == "localhost"
        || [".localhost", ".local", ".internal"].iter().any(|s| host.ends_with(s))
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || a == 0
        || (a == 100 && (b & 0xC0) == 64) // 100.64.0.0/10, carrier-grade NAT
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return blocked_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || (first & 0xfe00) == 0xfc00 // fc00::/7, unique local
        || (first & 0xffc0) == 0xfe80 // fe80::/10, link-local
}

async fn fetch(url: Url) -> ToolOutput {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("Coucou/", env!("CARGO_PKG_VERSION")))
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if is_blocked_host(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ToolOutput::error(e.to_string()),
    };
    let mut response = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => return ToolOutput::error(format!("Couldn't open the page: {e}")),
    };
    let status = response.status();
    if !status.is_success() {
        return ToolOutput::error(format!("HTTP {status}"));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = content_kind(&content_type);
    if kind == ContentKind::Other {
        return ToolOutput::error("Only text pages can be read.");
    }
    let final_url = response.url().clone();

    let mut body: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = MAX_BODY_BYTES - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
                if body.len() >= MAX_BODY_BYTES {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => return ToolOutput::error(format!("Couldn't read the page: {e}")),
        }
    }
    let raw = String::from_utf8_lossy(&body);
    let (title, text) = match kind {
        ContentKind::Html => html_to_text(&raw),
        _ => (None, raw.trim().to_string()),
    };
    let chars = text.chars().count();
    let mut out = format!("URL: {final_url}\n");
    if let Some(title) = &title {
        out.push_str(&format!("Title: {title}\n"));
    }
    out.push('\n');
    out.push_str(&text);
    let host = final_url.host_str().unwrap_or("").to_string();
    ToolOutput::text(out, format!("{host} · {chars} chars"))
}

#[derive(Debug, PartialEq, Eq)]
enum ContentKind {
    Html,
    Text,
    Other,
}

fn content_kind(content_type: &str) -> ContentKind {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    if mime.is_empty() {
        // No header: read it as plain text rather than refuse.
        ContentKind::Text
    } else if mime == "text/html" || mime.contains("xhtml") {
        ContentKind::Html
    } else if mime.starts_with("text/") || mime.contains("json") || mime.contains("xml") {
        ContentKind::Text
    } else {
        ContentKind::Other
    }
}

const SKIPPED: &[&str] = &["script", "style", "noscript", "svg", "template", "head"];
const BLOCKS: &[&str] = &[
    "br", "p", "div", "li", "tr", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "header", "footer",
    "ul", "ol", "table",
];

/// The title and readable text of an HTML page, without a parser dependency:
/// good enough for articles and docs, not a browser.
pub(crate) fn html_to_text(html: &str) -> (Option<String>, String) {
    // ASCII lowercase keeps byte offsets identical to `html`.
    let lower = html.to_ascii_lowercase();
    let title = lower.find("<title").and_then(|start| {
        let open_end = start + lower[start..].find('>')? + 1;
        let close = open_end + lower[open_end..].find("</title")?;
        let title = collapse_spaces(&decode_entities(&html[open_end..close]));
        (!title.is_empty()).then_some(title)
    });

    let mut text = String::new();
    let mut i = 0;
    while i < html.len() {
        let Some(lt) = html[i..].find('<').map(|p| i + p) else {
            text.push_str(&html[i..]);
            break;
        };
        text.push_str(&html[i..lt]);
        let rest = &lower[lt..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(html.len(), |p| lt + p + 3);
            continue;
        }
        let after = rest.as_bytes().get(1).copied().unwrap_or(b' ');
        if !(after.is_ascii_alphabetic() || after == b'/' || after == b'!' || after == b'?') {
            text.push('<');
            i = lt + 1;
            continue;
        }
        let Some(gt) = rest.find('>').map(|p| lt + p) else { break };
        let closing = after == b'/';
        let name: String = lower[lt + 1 + closing as usize..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        i = gt + 1;
        if !closing && SKIPPED.contains(&name.as_str()) && !lower[lt..gt].ends_with('/') {
            let end_tag = format!("</{name}");
            i = match lower[i..].find(&end_tag) {
                Some(p) => lower[i + p..].find('>').map_or(html.len(), |q| i + p + q + 1),
                None => html.len(),
            };
            continue;
        }
        if BLOCKS.contains(&name.as_str()) {
            text.push('\n');
        }
    }

    let decoded = decode_entities(&text);
    let mut out = String::new();
    let mut blank_run = 0;
    for line in decoded.lines() {
        let line = collapse_spaces(line);
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank_run > 0 { "\n\n" } else { "\n" });
        }
        out.push_str(&line);
        blank_run = 0;
    }
    (title, out)
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail[..tail.len().min(12)].find(';').and_then(|semi| {
            let entity = &tail[1..semi];
            let ch = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|num| {
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)
                }),
            }?;
            Some((ch, semi + 1))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

impl Tool for WebFetch {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn schema(&self) -> Value {
        json!({
            "name": "web_fetch",
            "description": "Open a web page by its exact URL and read its text. GET only.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The full http(s) URL." },
                },
                "required": ["url"],
            },
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::External
    }

    fn summarize(&self, args: &Value) -> String {
        let raw = args.get("url").and_then(Value::as_str).unwrap_or("");
        Url::parse(raw.trim())
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| raw.chars().take(40).collect())
    }

    fn run<'a>(&'a self, args: Value, ctx: ToolCtx<'a>) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Some(raw) = args.get("url").and_then(Value::as_str) else {
                return ToolOutput::error("Missing `url`.");
            };
            let url = match check_url(raw) {
                Ok(u) => u,
                Err(e) => return ToolOutput::error(e),
            };
            // Dropping the request on cancel aborts it; the loop reports the
            // cancellation itself.
            ctx.cancel.run(fetch(url)).await.unwrap_or_else(|| ToolOutput::error("Cancelled."))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_url_blocks_local_and_non_http() {
        for bad in [
            "file:///etc/passwd",
            "ftp://example.com/x",
            "http://127.0.0.1/",
            "http://[::1]:8080/",
            "http://192.168.1.1",
            "http://10.0.0.5/a",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.1.1",
            "http://0.0.0.0",
            "http://[::ffff:127.0.0.1]/",
            "http://[fd00::1]/",
            "http://localhost:8080",
            "http://api.localhost/",
            "http://foo.local",
            "http://printer.local./",
            "http://db.internal",
            "not a url",
        ] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(check_url(&format!("https://example.com/{}", "a".repeat(2100))).is_err());
        let ok = check_url("https://example.com/a?b=c").unwrap();
        assert_eq!(ok.host_str(), Some("example.com"));
        assert!(check_url("http://8.8.8.8/").is_ok());
    }

    #[test]
    fn html_to_text_strips_and_decodes() {
        let html = "<!doctype html><html><head><title>My &amp; Page</title>\
<style>body{color:red}</style><script>var x = '<p>no</p>';</script></head>\
<body><!-- hidden --><h1>Hello</h1><p>Fish &lt;3 &quot;chips&quot; &#39;n&#x27; caf&#233;&nbsp;bar</p>\
<div>one<br>two</div><noscript>nope</noscript><svg><text>nope</text></svg>\
<ul><li>a</li><li>b</li></ul>a < b</body></html>";
        let (title, text) = html_to_text(html);
        assert_eq!(title.as_deref(), Some("My & Page"));
        assert!(!text.contains("color:red"));
        assert!(!text.contains("var x"));
        assert!(!text.contains("nope"));
        assert!(!text.contains("hidden"));
        assert!(text.contains("Fish <3 \"chips\" 'n' café bar"));
        assert!(text.contains("one\ntwo"));
        assert!(text.contains("a\n\nb"));
        assert!(text.contains("a < b"));
        assert!(text.starts_with("Hello\n"));
        assert!(!text.contains("\n\n\n"));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_kind("text/html; charset=utf-8"), ContentKind::Html);
        assert_eq!(content_kind("application/xhtml+xml"), ContentKind::Html);
        assert_eq!(content_kind("text/plain"), ContentKind::Text);
        assert_eq!(content_kind("application/json"), ContentKind::Text);
        assert_eq!(content_kind("application/ld+json"), ContentKind::Text);
        assert_eq!(content_kind("application/pdf"), ContentKind::Other);
        assert_eq!(content_kind("image/png"), ContentKind::Other);
    }

    #[test]
    fn summary_is_the_host() {
        assert_eq!(WebFetch.summarize(&json!({ "url": "https://docs.rs/serde/latest" })), "docs.rs");
        assert_eq!(WebFetch.risk(&json!({})), Risk::External);
    }
}
