// read_file: a text file inside the allowed roots, by line range.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::{guard_path, resolve_path, Risk, Tool, ToolCtx, ToolOutput};
use crate::agent::transport::BoxFuture;

const DEFAULT_LIMIT: u64 = 300;
const MAX_LIMIT: u64 = 2000;
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_LINE_CHARS: usize = 2000;
const NOT_TEXT: &[&str] = &["pdf", "png", "jpg", "jpeg", "gif", "webp"];

pub struct ReadFile {
    /// The first root is also what `~` and relative paths resolve against.
    roots: Vec<PathBuf>,
}

impl ReadFile {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        ReadFile { roots }
    }

    fn read(&self, args: &Value) -> ToolOutput {
        let Some(raw) = args.as_object().and_then(|o| o.get("path")).and_then(Value::as_str) else {
            return ToolOutput::error("Missing `path`.");
        };
        let Some(home) = self.roots.first() else {
            return ToolOutput::error("No folder is open to Mochi.");
        };
        let path = match guard_path(&resolve_path(raw, home), &self.roots) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(e),
        };
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => return ToolOutput::error(format!("Can't read {raw}: {e}")),
        };
        if !meta.is_file() {
            return ToolOutput::error(format!("{raw} is not a file. Use list_dir for folders."));
        }
        if meta.len() > MAX_FILE_BYTES {
            return ToolOutput::error("This file is over 10 MB; too big to read.");
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if NOT_TEXT.contains(&ext.as_str()) {
            return ToolOutput::error("This is a PDF/image; ask the user to drop it on the island instead.");
        }
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return ToolOutput::error(format!("Can't read {raw}: {e}")),
        };
        if bytes.iter().take(8192).any(|&b| b == 0) {
            return ToolOutput::error("Binary file.");
        }
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let n = lines.len() as u64;

        let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(1).max(1);
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let shown = raw.trim();
        if offset > n {
            return ToolOutput::text(
                format!("{shown} — {n} lines; offset {offset} is past the end."),
                format!("past the end ({n} lines)"),
            );
        }
        let first = offset;
        let last = (offset + limit - 1).min(n);
        let mut out = format!("{shown} — lines {first}-{last} of {n}\n");
        for (i, line) in lines[(first - 1) as usize..last as usize].iter().enumerate() {
            let line: String = line.chars().take(MAX_LINE_CHARS).collect();
            out.push_str(&format!("{}\t{line}\n", first + i as u64));
        }
        ToolOutput::text(out, format!("lines {first}-{last} of {n}"))
    }
}

/// The last component of a path argument, for the island's chips.
pub(crate) fn short_name(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    if name.is_empty() { raw.trim().to_string() } else { name.to_string() }
}

impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn schema(&self) -> Value {
        json!({
            "name": "read_file",
            "description": "Read a text file on this computer. Paths may start with ~. Use offset/limit for long files.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path, e.g. ~/notes.txt" },
                    "offset": { "type": "integer", "description": "First line to read, 1-based." },
                    "limit": { "type": "integer", "description": "How many lines (default 300, at most 2000)." },
                },
                "required": ["path"],
            },
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::Read
    }

    fn summarize(&self, args: &Value) -> String {
        args.get("path").and_then(Value::as_str).map(short_name).unwrap_or_default()
    }

    fn run<'a>(&'a self, args: Value, _: ToolCtx<'a>) -> BoxFuture<'a, ToolOutput> {
        // Bounded synchronous I/O (10 MB at most): not worth spawn_blocking.
        Box::pin(async move { self.read(&args) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools::tests::temp_root;

    fn text(out: &ToolOutput) -> &str {
        out.for_model[0]["text"].as_str().unwrap()
    }

    #[test]
    fn reads_a_line_range() {
        let root = temp_root("read-range");
        let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("notes.txt"), body).unwrap();
        let tool = ReadFile::new(vec![root.clone()]);

        let out = tool.read(&json!({ "path": "notes.txt", "offset": 3, "limit": 2 }));
        assert!(!out.is_error, "{}", text(&out));
        assert_eq!(text(&out), "notes.txt — lines 3-4 of 10\n3\tline 3\n4\tline 4\n");
        assert_eq!(out.summary, "lines 3-4 of 10");

        let out = tool.read(&json!({ "path": "~/notes.txt" }));
        assert_eq!(out.summary, "lines 1-10 of 10");
        assert_eq!(tool.summarize(&json!({ "path": "~/a/notes.txt" })), "notes.txt");
    }

    #[test]
    fn offset_past_the_end_is_empty_not_an_error() {
        let root = temp_root("read-past");
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let out = ReadFile::new(vec![root]).read(&json!({ "path": "a.txt", "offset": 9 }));
        assert!(!out.is_error);
        assert!(text(&out).contains("past the end"));
    }

    #[test]
    fn binary_and_bad_input_are_errors() {
        let root = temp_root("read-bin");
        std::fs::write(root.join("blob.dat"), b"ab\0cd").unwrap();
        std::fs::write(root.join("pic.png"), b"x").unwrap();
        let tool = ReadFile::new(vec![root]);
        let out = tool.read(&json!({ "path": "blob.dat" }));
        assert!(out.is_error);
        assert_eq!(text(&out), "Binary file.");
        assert!(text(&tool.read(&json!({ "path": "pic.png" }))).contains("drop it on the island"));
        assert!(tool.read(&json!("notes.txt")).is_error);
        assert!(tool.read(&json!({})).is_error);
        assert!(tool.read(&json!({ "path": "." })).is_error); // a folder
    }

    #[test]
    fn outside_the_root_is_an_error() {
        let root = temp_root("read-root");
        let outside = temp_root("read-outside");
        std::fs::write(outside.join("x.txt"), "x").unwrap();
        let out = ReadFile::new(vec![root]).read(&json!({ "path": outside.join("x.txt").to_str().unwrap() }));
        assert!(out.is_error);
        assert!(text(&out).contains("outside"));
    }
}
