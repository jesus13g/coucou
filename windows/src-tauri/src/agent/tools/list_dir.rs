// list_dir: the entries of a folder inside the allowed roots.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::read_file::short_name;
use super::{guard_path, resolve_path, Risk, Tool, ToolCtx, ToolOutput};
use crate::agent::transport::BoxFuture;

const MAX_ENTRIES: usize = 200;

pub struct ListDir {
    /// The first root is also what `~` and relative paths resolve against.
    roots: Vec<PathBuf>,
}

impl ListDir {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        ListDir { roots }
    }

    fn list(&self, args: &Value) -> ToolOutput {
        if !args.is_object() {
            return ToolOutput::error("Tool input must be a JSON object.");
        }
        let raw = args.get("path").and_then(Value::as_str).unwrap_or("~");
        let Some(home) = self.roots.first() else {
            return ToolOutput::error("No folder is open to Mochi.");
        };
        let path = match guard_path(&resolve_path(raw, home), &self.roots) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let entries = match std::fs::read_dir(&path) {
            Ok(e) => e,
            Err(e) => return ToolOutput::error(format!("Can't list {raw}: {e}")),
        };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.metadata() {
                Ok(m) if m.is_dir() => dirs.push(name),
                Ok(m) => files.push((name, m.len())),
                Err(_) => files.push((name, 0)),
            }
        }
        dirs.sort_by_key(|n| n.to_lowercase());
        files.sort_by_key(|(n, _)| n.to_lowercase());

        let total = dirs.len() + files.len();
        let lines: Vec<String> = dirs
            .into_iter()
            .map(|d| format!("{d}/"))
            .chain(files.into_iter().map(|(f, size)| format!("{f}  {}", human_size(size))))
            .take(MAX_ENTRIES)
            .collect();
        let mut out = format!("{} — {total} entries\n", raw.trim());
        for line in &lines {
            out.push_str(line);
            out.push('\n');
        }
        if total > MAX_ENTRIES {
            out.push_str(&format!("… and {} more\n", total - MAX_ENTRIES));
        }
        ToolOutput::text(out, format!("{total} entries"))
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1000.0;
    let mut unit = 0;
    while size >= 1000.0 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn schema(&self) -> Value {
        json!({
            "name": "list_dir",
            "description": "List the files and folders in a folder on this computer. Paths may start with ~; the default is the home folder.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Folder path, e.g. ~/Downloads. Defaults to ~." },
                },
            },
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }

    fn summarize(&self, args: &Value) -> String {
        match args.get("path").and_then(Value::as_str) {
            Some(raw) if !raw.trim().is_empty() => short_name(raw),
            _ => "~".into(),
        }
    }

    fn run<'a>(&'a self, args: Value, _: ToolCtx<'a>) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move { self.list(&args) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::tests::temp_root;

    #[test]
    fn folders_first_then_files_with_sizes() {
        let root = temp_root("list-order");
        std::fs::create_dir_all(root.join("zeta")).unwrap();
        std::fs::create_dir_all(root.join("Alpha")).unwrap();
        std::fs::write(root.join("b.txt"), vec![b'x'; 12_345]).unwrap();
        std::fs::write(root.join("A.md"), b"hi").unwrap();
        let tool = ListDir::new(vec![root]);
        let out = tool.list(&json!({}));
        assert!(!out.is_error);
        let text = out.for_model[0]["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines, vec!["Alpha/", "zeta/", "A.md  2 B", "b.txt  12.3 KB"]);
        assert_eq!(out.summary, "4 entries");
        assert_eq!(tool.summarize(&json!({})), "~");
        assert_eq!(tool.summarize(&json!({ "path": "~/Downloads/" })), "Downloads");
    }

    #[test]
    fn caps_the_listing() {
        let root = temp_root("list-cap");
        for i in 0..205 {
            std::fs::write(root.join(format!("f{i:03}.txt")), b"").unwrap();
        }
        let out = ListDir::new(vec![root]).list(&json!({ "path": "~" }));
        let text = out.for_model[0]["text"].as_str().unwrap();
        assert_eq!(text.lines().count(), 1 + 200 + 1);
        assert!(text.ends_with("… and 5 more\n"));
        assert_eq!(out.summary, "205 entries");
    }

    #[test]
    fn outside_the_root_is_an_error() {
        let root = temp_root("list-root");
        let out = ListDir::new(vec![root]).list(&json!({ "path": ".." }));
        assert!(out.is_error);
    }
}
