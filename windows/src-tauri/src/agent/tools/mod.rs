// Local tools the model may call, the policy that gates them, and the helpers
// they share: output capping, canonical JSON for de-duplication, and the path
// guard that keeps file tools inside the user's home folder and away from
// credentials.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};

pub mod list_dir;
pub mod read_file;

use super::cancel::Cancel;
use super::transport::BoxFuture;

/// How much harm a call could do — what the policy decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Reads local data.
    Read,
    /// Changes local data (no such tool yet).
    Write,
    /// Reaches a third-party service.
    External,
    /// Sends something on the user's behalf (no such tool yet).
    Send,
}

pub struct ToolCtx<'a> {
    pub cancel: &'a Cancel,
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Content blocks for the tool_result.
    pub for_model: Vec<Value>,
    /// Optional structured data for the island.
    pub for_ui: Option<Value>,
    /// One short line for the tool_end event — never file or page contents.
    pub summary: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>, summary: impl Into<String>) -> Self {
        ToolOutput {
            for_model: vec![json!({ "type": "text", "text": text.into() })],
            for_ui: None,
            summary: summary.into(),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        let summary = truncate_chars(&message, 60);
        ToolOutput {
            for_model: vec![json!({ "type": "text", "text": message })],
            for_ui: None,
            summary,
            is_error: true,
        }
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    /// `{"name", "description", "input_schema"}`, as the Messages API wants it.
    fn schema(&self) -> Value;
    fn risk(&self, args: &Value) -> Risk;
    /// What the tool_start event shows: a file name, a host…
    fn summarize(&self, args: &Value) -> String;
    fn run<'a>(&'a self, args: Value, ctx: ToolCtx<'a>) -> BoxFuture<'a, ToolOutput>;
}

pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Registry { tools }
    }

    /// The phase 1 set: tools that only read, confined to `roots`.
    pub fn read_only(roots: Vec<PathBuf>) -> Self {
        Registry::new(vec![
            Box::new(read_file::ReadFile::new(roots.clone())),
            Box::new(list_dir::ListDir::new(roots)),
        ])
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    pub fn schemas(&self) -> Vec<Value> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
}

/// What the turn has done so far that changes what is allowed next.
#[derive(Debug, Clone, Default)]
pub struct PolicyState {
    /// A Read tool ran successfully this turn: local data is in the context.
    pub read_local: bool,
}

/// Phase 1 policy. Phase 2 hooks ask/deny prompts in here.
pub fn decide(risk: Risk, state: &PolicyState) -> Decision {
    match risk {
        Risk::Read => Decision::Allow,
        // Taint rule: once local files are in the context, a prompt injection
        // could smuggle them out through a URL.
        Risk::External if state.read_local => Decision::Deny(
            "web_fetch is off after reading local files in the same request, to keep your files private. \
Answer with what you have."
                .into(),
        ),
        Risk::External => Decision::Allow,
        Risk::Write | Risk::Send => Decision::Deny("This action isn't available yet.".into()),
    }
}

/// Keeps the text blocks of `out` within `max_chars` characters in all: the
/// head stays, the rest is replaced by a note saying how much was cut.
pub fn cap_output(out: &mut ToolOutput, max_chars: usize) {
    let mut left = max_chars;
    for block in &mut out.for_model {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = block.get("text").and_then(Value::as_str) else { continue };
        let count = text.chars().count();
        if count <= left {
            left -= count;
            continue;
        }
        let head: String = text.chars().take(left).collect();
        let cut = count - left;
        block["text"] = Value::String(format!("{head}\n[… {cut} more characters cut]"));
        left = 0;
    }
}

/// JSON with object keys sorted at every level, so the same call written in
/// a different key order is recognised as the same call.
pub fn canonical(v: &Value) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&map[k]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(v).to_string()
}

/// `~`, `~/x`, `~\x` and relative paths resolve against `home`; absolute
/// paths stay as they are.
pub(crate) fn resolve_path(raw: &str, home: &Path) -> PathBuf {
    let raw = raw.trim();
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return home.join(rest);
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        home.join(path)
    }
}

/// The canonical form of `path` if it lies inside one of `roots` and isn't a
/// credential store. Canonicalising both sides resolves `..` and symlinks, and
/// on Windows gives the same `\\?\C:\…` form on both sides.
pub(crate) fn guard_path(path: &Path, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let real = path
        .canonicalize()
        .map_err(|_| format!("Not found: {}", path.display()))?;
    let inside = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .find_map(|root| real.strip_prefix(&root).ok().map(Path::to_path_buf));
    let Some(relative) = inside else {
        return Err("That path is outside what Mochi may read.".into());
    };
    if is_sensitive(&relative) {
        return Err("Mochi doesn't read credential files.".into());
    }
    Ok(real)
}

const SENSITIVE_DIRS: &[&str] = &[
    ".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".password-store", "keyrings", "gcloud",
    ".mozilla", "google-chrome", "chromium", "bravesoftware", "user data", "credentials", "protect",
];
const SENSITIVE_FILES: &[&str] = &[
    ".netrc", ".npmrc", ".pypirc", ".git-credentials", ".pgpass", ".credentials.json", "credentials.json",
    "cookies", "login data", "key4.db", "logins.json",
];
const SENSITIVE_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", ".env"];
const SENSITIVE_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".kdbx"];

/// Credential stores and secrets, judged on the part of the path below the
/// root, case-insensitively.
fn is_sensitive(relative: &Path) -> bool {
    let names: Vec<String> = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    if names.iter().any(|n| SENSITIVE_DIRS.contains(&n.as_str())) {
        return true;
    }
    let Some(file) = names.last() else { return false };
    SENSITIVE_FILES.contains(&file.as_str())
        || SENSITIVE_PREFIXES.iter().any(|p| file.starts_with(p))
        || SENSITIVE_SUFFIXES.iter().any(|s| file.ends_with(s))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    struct Fake;
    impl Tool for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn schema(&self) -> Value {
            json!({ "name": "fake", "description": "d", "input_schema": { "type": "object" } })
        }
        fn risk(&self, _: &Value) -> Risk {
            Risk::Read
        }
        fn summarize(&self, _: &Value) -> String {
            String::new()
        }
        fn run<'a>(&'a self, _: Value, _: ToolCtx<'a>) -> BoxFuture<'a, ToolOutput> {
            Box::pin(async { ToolOutput::text("ok", "ok") })
        }
    }

    pub(crate) fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coucou-tools-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn registry_finds_tools_by_name() {
        let registry = Registry::new(vec![Box::new(Fake)]);
        assert_eq!(registry.names(), vec!["fake"]);
        assert_eq!(registry.schemas()[0]["name"], "fake");
        assert!(registry.get("fake").is_some());
        assert!(registry.get("nope").is_none());

        let registry = Registry::read_only(vec![std::env::temp_dir()]);
        assert_eq!(registry.names(), vec!["read_file", "list_dir"]);
    }

    #[test]
    fn canonical_ignores_key_order() {
        let a = json!({ "a": 1, "b": { "y": [1, { "q": 1, "p": 2 }], "x": null } });
        let b = json!({ "b": { "x": null, "y": [1, { "p": 2, "q": 1 }] }, "a": 1 });
        assert_eq!(canonical(&a), canonical(&b));
        assert_ne!(canonical(&json!({ "a": 1 })), canonical(&json!({ "a": 2 })));
        // Array order still matters.
        assert_ne!(canonical(&json!([1, 2])), canonical(&json!([2, 1])));
    }

    #[test]
    fn cap_output_keeps_utf8_whole() {
        let mut out = ToolOutput::text("abcé€xyz", "s");
        cap_output(&mut out, 4);
        assert_eq!(out.for_model[0]["text"], "abcé\n[… 4 more characters cut]");

        let mut out = ToolOutput::text("éé", "s");
        cap_output(&mut out, 2);
        assert_eq!(out.for_model[0]["text"], "éé");

        // The budget is shared across text blocks.
        let mut out = ToolOutput::text("aaaa", "s");
        out.for_model.push(json!({ "type": "text", "text": "bbbb" }));
        cap_output(&mut out, 6);
        assert_eq!(out.for_model[0]["text"], "aaaa");
        assert_eq!(out.for_model[1]["text"], "bb\n[… 2 more characters cut]");
    }

    #[test]
    fn error_summary_is_short() {
        let out = ToolOutput::error("x".repeat(100));
        assert!(out.is_error);
        assert_eq!(out.summary.chars().count(), 60);
        assert_eq!(out.for_model[0]["text"], "x".repeat(100));
    }

    #[test]
    fn decide_follows_the_table() {
        let clean = PolicyState::default();
        let tainted = PolicyState { read_local: true };
        assert_eq!(decide(Risk::Read, &clean), Decision::Allow);
        assert_eq!(decide(Risk::Read, &tainted), Decision::Allow);
        assert_eq!(decide(Risk::External, &clean), Decision::Allow);
        match decide(Risk::External, &tainted) {
            Decision::Deny(why) => assert!(why.contains("keep your files private")),
            other => panic!("{other:?}"),
        }
        for risk in [Risk::Write, Risk::Send] {
            assert!(matches!(decide(risk, &clean), Decision::Deny(_)));
        }
    }

    #[test]
    fn resolve_path_expands_home() {
        let home = Path::new("/home/me");
        assert_eq!(resolve_path("~", home), PathBuf::from("/home/me"));
        assert_eq!(resolve_path("~/a/b.txt", home), home.join("a/b.txt"));
        assert_eq!(resolve_path("~\\a", home), home.join("a"));
        assert_eq!(resolve_path("notes.txt", home), home.join("notes.txt"));
        let abs = std::env::temp_dir().join("x");
        assert_eq!(resolve_path(abs.to_str().unwrap(), home), abs);
    }

    #[test]
    fn guard_path_confines_to_roots() {
        let root = temp_root("guard");
        let roots = vec![root.clone()];
        std::fs::write(root.join("notes.txt"), "hi").unwrap();
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh").join("id_rsa"), "secret").unwrap();
        std::fs::write(root.join("server.PEM"), "secret").unwrap();
        let outside = temp_root("guard-outside");
        std::fs::write(outside.join("other.txt"), "x").unwrap();

        assert_eq!(guard_path(&root.join("notes.txt"), &roots).unwrap(), root.join("notes.txt").canonicalize().unwrap());
        assert!(guard_path(&root, &roots).is_ok());

        let escape = root.join("..").join(outside.file_name().unwrap()).join("other.txt");
        assert_eq!(guard_path(&escape, &roots).unwrap_err(), "That path is outside what Mochi may read.");

        assert_eq!(guard_path(&root.join(".ssh").join("id_rsa"), &roots).unwrap_err(), "Mochi doesn't read credential files.");
        assert!(guard_path(&root.join(".ssh"), &roots).is_err());
        assert!(guard_path(&root.join("server.PEM"), &roots).is_err());
        assert!(guard_path(&root.join("missing.txt"), &roots).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn guard_path_follows_symlinks_out_of_the_root() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-outside");
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
        assert_eq!(
            guard_path(&root.join("link.txt"), &[root.clone()]).unwrap_err(),
            "That path is outside what Mochi may read."
        );
    }
}
