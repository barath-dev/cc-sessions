//! Every saved conversation, read from the transcripts Claude Code keeps.
//! Transcripts are only ever opened for reading.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use memchr::memmem;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::accounts;
use crate::store::{self, state_file};
use crate::util::{iso_to_ms, path_str, squash, Error, Result};

const PREVIEW_CHARS: usize = 300;

/// What is remembered about one transcript between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Entry {
    pub session_id: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<i64>,
    pub prompts: u32,
    pub first_prompt: Option<String>,
    pub last_prompts: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cached {
    size: u64,
    mtime: u64,
    entry: Entry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub account: String,
    pub account_dir: String,
    pub id: String,
    pub session_id: String,
    pub title: String,
    /// The folder the conversation ran in; empty when the transcript does not say.
    pub cwd: String,
    /// `cwd`, or the transcript's folder name when there is none.
    pub folder_label: String,
    pub started_at: Option<i64>,
    pub last_activity: i64,
    pub prompts: u32,
    pub first_prompt: Option<String>,
    pub last_prompts: Vec<String>,
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// The text of a real user prompt, or None for tool results and meta records.
fn prompt_text(rec: &Value) -> Option<String> {
    let flag = |key: &str| rec.get(key).and_then(Value::as_bool).unwrap_or(false);
    if flag("isMeta") || flag("isSidechain") {
        return None;
    }
    let content = rec.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let kind = |b: &Value| b.get("type").and_then(Value::as_str).unwrap_or("").to_string();
            if blocks.iter().any(|b| kind(b) == "tool_result") {
                return None;
            }
            blocks
                .iter()
                .filter(|b| kind(b) == "text")
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.starts_with('<') {
        // Slash commands are stored as tags; everything else tagged is machinery.
        // A command with no arguments (/clear, /model) says nothing about the
        // conversation, so only ones that carry text count as prompts.
        let name = between(text, "<command-name>", "</command-name>")?.trim();
        let args = between(text, "<command-args>", "</command-args>")?.trim();
        if name.is_empty() || name.contains(char::is_whitespace) || args.is_empty() {
            return None;
        }
        let slash = if name.starts_with('/') { "" } else { "/" };
        return Some(squash(&format!("{slash}{name} {args}")));
    }
    Some(squash(text))
}

fn truncate(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// Read one saved conversation into a small summary. Never writes to it.
fn index_transcript(path: &Path) -> Entry {
    let mut entry = Entry {
        session_id: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        ..Entry::default()
    };
    let (mut custom, mut ai, mut agent) = (None, None, None);
    let Ok(file) = File::open(path) else {
        entry.unreadable = true;
        return entry;
    };
    let mut reader = BufReader::with_capacity(1 << 16, file);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => {
                entry.unreadable = true;
                break;
            }
        }
        // Lines can be megabytes of tool output; look before parsing.
        let head = &line[..line.len().min(40)];
        let is_user = memmem::find(&line, b"\"type\":\"user\"").is_some();
        let is_title = memmem::find(head, b"-title\"").is_some() || memmem::find(head, b"\"agent-name\"").is_some();
        let needs_meta = entry.cwd.is_none() || entry.started_at.is_none();
        if !(is_title || is_user || (needs_meta && memmem::find(&line, b"\"cwd\"").is_some())) {
            continue;
        }
        if is_user && !needs_meta && memmem::find(&line, b"\"type\":\"tool_result\"").is_some() {
            continue;
        }
        let Ok(rec) = serde_json::from_slice::<Value>(&line) else { continue };
        let text = |key: &str| rec.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        let kind = rec.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "custom-title" => {
                custom = text("customTitle").or(custom);
                continue;
            }
            "ai-title" => {
                ai = text("aiTitle").or(ai);
                continue;
            }
            "agent-name" => {
                agent = text("agentName").or(agent);
                continue;
            }
            _ => {}
        }
        if entry.cwd.is_none() {
            entry.cwd = text("cwd");
        }
        if entry.started_at.is_none() {
            entry.started_at = text("timestamp").and_then(|t| iso_to_ms(&t));
        }
        if kind == "user" {
            if let Some(prompt) = prompt_text(&rec) {
                let prompt = truncate(&prompt, PREVIEW_CHARS);
                entry.prompts += 1;
                if entry.first_prompt.is_none() {
                    entry.first_prompt = Some(prompt.clone());
                }
                entry.last_prompts.push(prompt);
                if entry.last_prompts.len() > 3 {
                    entry.last_prompts.remove(0);
                }
            }
        }
    }
    let custom = custom.filter(|t| t != "Untitled session");
    let from_prompt = entry.first_prompt.as_deref().map(|p| truncate(p, 80)).filter(|p| !p.is_empty());
    entry.title = custom.or(ai).or(agent).or(from_prompt);
    entry
}

static INDEXING: Mutex<()> = Mutex::new(());

/// Every saved conversation of every active account, newest first.
///
/// Transcripts are indexed once and cached by path, size and modification
/// time, so later calls only read what changed.
pub fn history_items() -> Result<Vec<HistoryItem>> {
    let accounts = accounts::load_accounts(false)?;
    let _one_at_a_time = INDEXING.lock().unwrap_or_else(|p| p.into_inner());
    let cache_file = state_file("history-cache.json");
    let cache: BTreeMap<String, Cached> =
        serde_json::from_value(store::read_json_or_null(&cache_file)).unwrap_or_default();
    let mut fresh: BTreeMap<String, Cached> = BTreeMap::new();
    let mut items = Vec::new();
    let mut changed = false;

    for acc in &accounts {
        for path in transcripts(Path::new(&acc.dir)) {
            let Ok(meta) = fs::metadata(&path) else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let key = path_str(&path);
            let entry = match cache.get(&key) {
                Some(hit) if hit.size == meta.len() && hit.mtime == mtime => hit.entry.clone(),
                _ => {
                    changed = true;
                    index_transcript(&path)
                }
            };
            let folder = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            items.push(HistoryItem {
                account: acc.name.clone(),
                account_dir: acc.dir.clone(),
                id: entry.session_id.chars().take(8).collect(),
                session_id: entry.session_id.clone(),
                title: entry.title.clone().unwrap_or_else(|| format!("(no title) {file_name}")),
                cwd: entry.cwd.clone().unwrap_or_default(),
                folder_label: entry.cwd.clone().unwrap_or(folder),
                started_at: entry.started_at,
                last_activity: (mtime / 1_000_000) as i64,
                prompts: entry.prompts,
                first_prompt: entry.first_prompt.clone(),
                last_prompts: entry.last_prompts.clone(),
            });
            fresh.insert(key, Cached { size: meta.len(), mtime, entry });
        }
    }
    if changed || !fresh.keys().eq(cache.keys()) {
        let value = serde_json::to_value(&fresh).map_err(|e| Error(e.to_string()))?;
        store::write_json(&cache_file, &value, false)?;
    }
    items.sort_by_key(|item| -item.last_activity);
    Ok(items)
}

/// `<account>/projects/<folder>/<session>.jsonl`. Top level only: nested
/// folders hold sub-agent transcripts, which are not sessions.
fn transcripts(account_dir: &Path) -> Vec<PathBuf> {
    let Ok(projects) = fs::read_dir(account_dir.join("projects")) else { return Vec::new() };
    let mut found = Vec::new();
    for project in projects.flatten() {
        let Ok(files) = fs::read_dir(project.path()) else { continue };
        found.extend(files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl") && p.is_file()));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompts_skip_machinery() {
        assert_eq!(prompt_text(&json!({"message": {"content": "  fix   the\nbug "}})), Some("fix the bug".into()));
        assert_eq!(prompt_text(&json!({"isMeta": true, "message": {"content": "hello"}})), None);
        assert_eq!(prompt_text(&json!({"isSidechain": true, "message": {"content": "hello"}})), None);
        assert_eq!(prompt_text(&json!({"message": {"content": "<system-reminder>x</system-reminder>"}})), None);
        assert_eq!(prompt_text(&json!({"message": {"content": [{"type": "tool_result", "content": "x"}]}})), None);
        assert_eq!(
            prompt_text(&json!({"message": {"content": [{"type": "text", "text": "one"}, {"type": "text", "text": "two"}]}})),
            Some("one two".into())
        );
    }

    #[test]
    fn slash_commands_count_only_with_arguments() {
        let with_args = "<command-message>plan</command-message><command-name>/plan</command-name><command-args>how to do it</command-args>";
        assert_eq!(prompt_text(&json!({"message": {"content": with_args}})), Some("/plan how to do it".into()));
        let bare = "<command-name>/clear</command-name><command-args></command-args>";
        assert_eq!(prompt_text(&json!({"message": {"content": bare}})), None);
    }

    #[test]
    fn indexes_a_transcript() {
        let dir = std::env::temp_dir().join(format!("cc-sessions-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0f0f0f0f-aaaa-bbbb-cccc-121212121212.jsonl");
        let lines = [
            r#"{"type":"mode","mode":"normal","sessionId":"x"}"#,
            r#"{"type":"user","isMeta":true,"cwd":"/work/app","timestamp":"2026-11-10T06:19:56.638Z","message":{"role":"user","content":"<local-command-caveat>x</local-command-caveat>"}}"#,
            r#"{"type":"user","cwd":"/work/app","timestamp":"2026-11-10T06:20:00.000Z","message":{"role":"user","content":"first question"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"\"type\":\"user\" is only quoted here"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"big output"}]}}"#,
            r#"{"type":"ai-title","aiTitle":"First title","sessionId":"x"}"#,
            r#"{"type":"user","message":{"role":"user","content":"second question"}}"#,
            r#"{"type":"ai-title","aiTitle":"Better title","sessionId":"x"}"#,
            "this line is not json",
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        let entry = index_transcript(&path);
        assert_eq!(entry.session_id, "0f0f0f0f-aaaa-bbbb-cccc-121212121212");
        assert_eq!(entry.cwd.as_deref(), Some("/work/app"));
        assert_eq!(entry.started_at, Some(1794291596638));
        assert_eq!(entry.prompts, 2);
        assert_eq!(entry.first_prompt.as_deref(), Some("first question"));
        assert_eq!(entry.last_prompts, vec!["first question", "second question"]);
        assert_eq!(entry.title.as_deref(), Some("Better title"));
        let _ = fs::remove_dir_all(&dir);
    }
}
