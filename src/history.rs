//! Every saved conversation, read from the transcripts each tool keeps: Claude
//! Code's here, Codex's rollouts in `codex`. Transcripts are only ever opened for reading.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use memchr::memmem;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::accounts;
use crate::codex;
use crate::loops::{Gatherer, Work};
use crate::pins::Pins;
use crate::store::{self, state_file};
use crate::titles::Titles;
use crate::tools::Tool;
use crate::util::{iso_to_ms, path_str, squash, Error, Result};

const PREVIEW_CHARS: usize = 300;
/// The tools whose use says what work was done: commands run and files edited.
const WORK_TOOLS: [&[u8]; 4] = [b"\"name\":\"Bash\"", b"\"name\":\"Edit\"", b"\"name\":\"Write\"", b"\"name\":\"MultiEdit\""];
/// Raise when `Entry` gains something old cache entries do not have.
const CACHE_VERSION: u32 = 7;
/// How many prompts are kept for searching: this many from each end of a conversation.
const SEARCH_PROMPTS: usize = 20;
const ARTIFACT_URLS: [&str; 2] = ["https://claude.ai/artifact/", "https://claude.ai/code/artifact/"];

/// A page the conversation published to, or opened on, claude.ai.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Artifact {
    pub url: String,
    pub title: String,
    pub updated_at: Option<i64>,
    /// The local file it was published from; what ties its two link forms together.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Tokens used, as Claude Code records them on each reply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Tokens {
    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_read += other.cache_read;
    }
}

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
    /// What the person typed, one prompt to a line, for the page's search.
    pub prompt_text: String,
    pub artifacts: Vec<Artifact>,
    pub tokens: Tokens,
    /// Tokens by the hour they were used in (hours since 1970, UTC), for the usage view.
    pub hours: Vec<(i64, Tokens)>,
    pub models: BTreeMap<String, Tokens>,
    /// What was done, for spotting sessions going round in circles.
    pub work: Work,
    /// When the conversation was last worked in, when the transcript says and its
    /// file's time may not (Codex rewrites old rollouts when it compresses them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_at: Option<i64>,
    /// Part of another conversation rather than one of its own: a sub-agent's, say.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cached {
    #[serde(default)]
    v: u32,
    size: u64,
    mtime: u64,
    entry: Entry,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub account: String,
    pub tool: Tool,
    pub account_dir: String,
    pub id: String,
    pub session_id: String,
    pub title: String,
    /// True when `title` is a name given through this tool.
    pub renamed: bool,
    /// Pinned to the top of the page through this tool.
    pub pinned: bool,
    /// The folder the conversation ran in; empty when the transcript does not say.
    pub cwd: String,
    /// `cwd`, or the transcript's folder name when there is none.
    pub folder_label: String,
    pub started_at: Option<i64>,
    pub last_activity: i64,
    pub prompts: u32,
    pub first_prompt: Option<String>,
    pub last_prompts: Vec<String>,
    #[serde(skip)]
    pub prompt_text: String,
    pub artifacts: Vec<Artifact>,
    /// With its sub-agents'.
    pub tokens: Tokens,
    /// Without its sub-agents': what the loop check weighs its own edits against.
    #[serde(skip)]
    pub own_tokens: Tokens,
    #[serde(skip)]
    pub hours: Vec<(i64, Tokens)>,
    #[serde(skip)]
    pub models: BTreeMap<String, Tokens>,
    #[serde(skip)]
    pub work: Work,
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

/// Add an artifact link found in a transcript, or bring the one already
/// listed up to date. `source` is a `frame-link` record or an Artifact tool's
/// result; anything without a claude.ai artifact link is ignored.
fn note_artifact(artifacts: &mut Vec<Artifact>, source: &Value, url_key: &str, at: Option<i64>) {
    let text = |key: &str| source.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let Some(url) = text(url_key).filter(|u| ARTIFACT_URLS.iter().any(|prefix| u.starts_with(prefix))) else { return };
    let path = text("path");
    let title = text("title").map(|t| truncate(&squash(&t), PREVIEW_CHARS));
    // One artifact has a short link and a longer /code/ one; the file it was
    // published from, or its id, is what shows they are the same.
    let id = text("artifact_id");
    let known = artifacts
        .iter_mut()
        .find(|a| a.url == url || (path.is_some() && a.path == path) || id.as_deref().is_some_and(|id| a.url.ends_with(id)));
    match known {
        Some(known) => {
            if known.url.starts_with(ARTIFACT_URLS[1]) {
                known.url = url;
            }
            known.title = title.unwrap_or_else(|| known.title.clone());
            known.updated_at = at.or(known.updated_at);
            known.path = path.or(known.path.take());
        }
        None => {
            let file_name = path.as_deref().and_then(|p| Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned());
            let title = title.or(file_name).unwrap_or_else(|| "Untitled artifact".into());
            artifacts.push(Artifact { url, title, updated_at: at, path });
        }
    }
}

/// The id of the reply a line belongs to. Claude Code writes one line per part
/// of a reply, each carrying the reply's whole usage, so a reply counts once.
fn reply_id(line: &[u8]) -> Option<&[u8]> {
    let start = memmem::find(line, b"\"id\":\"msg_")? + 6;
    let len = memchr::memchr(b'"', &line[start..])?;
    Some(&line[start..start + len])
}

fn note_usage(entry: &mut Entry, rec: &Value) {
    let Some(message) = rec.get("message") else { return };
    let model = message.get("model").and_then(Value::as_str).unwrap_or("");
    // "<synthetic>" replies are written by Claude Code itself, not answered by a model.
    if model.starts_with('<') {
        return;
    }
    let Some(usage) = message.get("usage") else { return };
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let used = Tokens {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_write: count("cache_creation_input_tokens"),
        cache_read: count("cache_read_input_tokens"),
    };
    if used == Tokens::default() {
        return;
    }
    let at = rec.get("timestamp").and_then(Value::as_str).and_then(iso_to_ms);
    note_tokens(entry, model, &used, at);
}

/// Add one reply's tokens: to the whole, to its model and to the hour it was used in.
pub fn note_tokens(entry: &mut Entry, model: &str, used: &Tokens, at: Option<i64>) {
    entry.tokens.add(used);
    entry.models.entry(model.to_string()).or_default().add(used);
    if let Some(at) = at {
        let hour = at.div_euclid(3_600_000);
        match entry.hours.last_mut() {
            Some((last, tokens)) if *last == hour => tokens.add(used),
            _ => entry.hours.push((hour, *used)),
        }
    }
}

/// Note one prompt the person typed. `typed` keeps what is searched: how a long
/// conversation began and where it got to.
pub fn note_prompt(entry: &mut Entry, typed: &mut Vec<String>, prompt: &str) {
    let prompt = truncate(prompt, PREVIEW_CHARS);
    entry.prompts += 1;
    if entry.first_prompt.is_none() {
        entry.first_prompt = Some(prompt.clone());
    }
    typed.push(prompt.clone());
    if typed.len() > 2 * SEARCH_PROMPTS {
        typed.remove(SEARCH_PROMPTS);
    }
    entry.last_prompts.push(prompt);
    if entry.last_prompts.len() > 3 {
        entry.last_prompts.remove(0);
    }
}

/// Read one saved conversation into a small summary. Never writes to it.
fn index_transcript(path: &Path) -> Entry {
    let mut entry = Entry {
        session_id: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        ..Entry::default()
    };
    let (mut custom, mut ai, mut agent) = (None, None, None);
    let mut typed: Vec<String> = Vec::new();
    let mut replies: HashSet<Vec<u8>> = HashSet::new();
    let mut work = Gatherer::default();
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
        let is_link = memmem::find(head, b"\"frame-link\"").is_some();
        // An Artifact tool's result names the page it published or opened.
        let has_artifact =
            is_user && memmem::find(&line, b"\"toolUseResult\"").is_some() && memmem::find(&line, b"claude.ai/").is_some();
        let needs_meta = entry.cwd.is_none() || entry.started_at.is_none();
        // A model's reply, with the tokens it used. Each reply is read once, however many lines it spans.
        if memmem::find(&line, b"\"type\":\"assistant\"").is_some() && memmem::find(&line, b"\"usage\"").is_some() {
            let new_reply = reply_id(&line).is_none_or(|id| replies.insert(id.to_vec()));
            // Commands run and edits made, from any line of a reply.
            let tools = memmem::find(&line, b"\"tool_use\"").is_some() && WORK_TOOLS.iter().any(|name| memmem::find(&line, name).is_some());
            if new_reply || tools {
                if let Ok(rec) = serde_json::from_slice::<Value>(&line) {
                    if new_reply {
                        work.reply();
                        note_usage(&mut entry, &rec);
                    }
                    if tools {
                        work.assistant(&rec);
                    }
                }
            }
            if !needs_meta {
                continue;
            }
        }
        // A command that failed.
        if is_user && memmem::find(&line, b"\"is_error\":true").is_some() {
            if let Ok(rec) = serde_json::from_slice::<Value>(&line) {
                work.results(&rec);
            }
        }
        if !(is_title || is_link || is_user || (needs_meta && memmem::find(&line, b"\"cwd\"").is_some())) {
            continue;
        }
        if is_user && !needs_meta && !has_artifact && memmem::find(&line, b"\"type\":\"tool_result\"").is_some() {
            continue;
        }
        let Ok(rec) = serde_json::from_slice::<Value>(&line) else { continue };
        let text = |key: &str| rec.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        let kind = rec.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "frame-link" => {
                let at = text("timestamp").and_then(|t| iso_to_ms(&t));
                note_artifact(&mut entry.artifacts, &rec, "frameUrl", at);
                continue;
            }
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
            if let Some(result) = rec.get("toolUseResult").filter(|r| has_artifact && r.is_object()) {
                let at = text("timestamp").and_then(|t| iso_to_ms(&t));
                note_artifact(&mut entry.artifacts, result, "url", at);
            }
            if let Some(prompt) = prompt_text(&rec) {
                work.prompt(&prompt);
                note_prompt(&mut entry, &mut typed, &prompt);
            }
        }
    }
    let custom = custom.filter(|t| t != "Untitled session");
    let from_prompt = entry.first_prompt.as_deref().map(|p| truncate(p, 80)).filter(|p| !p.is_empty());
    entry.title = custom.or(ai).or(agent).or(from_prompt);
    entry.prompt_text = typed.join("\n");
    entry.work = work.finish();
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
    let titles = Titles::load()?;
    let pins = Pins::load()?;
    let mut fresh: BTreeMap<String, Cached> = BTreeMap::new();
    let mut items = Vec::new();
    let mut changed = false;

    for acc in &accounts {
        let (paths, names) = match acc.tool {
            Tool::Claude => (transcripts(Path::new(&acc.dir)), Default::default()),
            Tool::Codex => (codex::transcripts(Path::new(&acc.dir)), codex::thread_names(Path::new(&acc.dir))),
        };
        let first = items.len();
        let mut agents = Vec::new();
        for path in paths {
            let Ok(meta) = fs::metadata(&path) else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let key = path_str(&path);
            let entry = match cache.get(&key) {
                Some(hit) if hit.v == CACHE_VERSION && hit.size == meta.len() && hit.mtime == mtime => hit.entry.clone(),
                _ => {
                    changed = true;
                    match acc.tool {
                        Tool::Claude => index_transcript(&path),
                        Tool::Codex => codex::index_rollout(&path),
                    }
                }
            };
            let folder = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let key_entry = (key, Cached { v: CACHE_VERSION, size: meta.len(), mtime, entry: entry.clone() });
            fresh.insert(key_entry.0, key_entry.1);
            if let Some(session) = (acc.tool == Tool::Claude).then(|| agent_of(Path::new(&acc.dir), &path)).flatten() {
                agents.push((session, entry));
                continue;
            }
            if entry.hidden {
                continue;
            }
            let given = titles.get(&acc.dir, &entry.session_id);
            // The name Codex has for it, which it keeps apart from the conversation.
            let own = names.get(&entry.session_id).cloned().or(entry.title.clone());
            items.push(HistoryItem {
                account: acc.name.clone(),
                tool: acc.tool,
                account_dir: acc.dir.clone(),
                id: acc.tool.short_id(&entry.session_id),
                session_id: entry.session_id.clone(),
                title: given.map(String::from).or(own).unwrap_or_else(|| format!("(no title) {file_name}")),
                renamed: given.is_some(),
                pinned: pins.has_session(&acc.dir, &entry.session_id),
                cwd: entry.cwd.clone().unwrap_or_default(),
                folder_label: entry.cwd.clone().unwrap_or(folder),
                started_at: entry.started_at,
                last_activity: entry.last_at.unwrap_or((mtime / 1_000_000) as i64),
                prompts: entry.prompts,
                first_prompt: entry.first_prompt.clone(),
                last_prompts: entry.last_prompts.clone(),
                prompt_text: entry.prompt_text.clone(),
                artifacts: entry.artifacts.clone(),
                tokens: entry.tokens,
                own_tokens: entry.tokens,
                hours: entry.hours.clone(),
                models: entry.models.clone(),
                work: entry.work,
            });
        }
        for (session, agent) in &agents {
            if let Some(item) = items[first..].iter_mut().find(|item| &item.session_id == session) {
                add_agent(item, agent);
            }
        }
    }
    if changed || !fresh.keys().eq(cache.keys()) {
        let value = serde_json::to_value(&fresh).map_err(|e| Error(e.to_string()))?;
        store::write_json(&cache_file, &value, false)?;
    }
    items.sort_by_key(|item| -item.last_activity);
    Ok(items)
}

/// `<account>/projects/<folder>/<session>.jsonl`, and the sub-agents' transcripts
/// under `<folder>/<session>/subagents/` (a workflow's a folder further down).
fn transcripts(account_dir: &Path) -> Vec<PathBuf> {
    let Ok(projects) = fs::read_dir(account_dir.join("projects")) else { return Vec::new() };
    let mut found = Vec::new();
    for project in projects.flatten() {
        let Ok(files) = fs::read_dir(project.path()) else { continue };
        for file in files.flatten() {
            let path = file.path();
            if file.file_type().is_ok_and(|t| t.is_dir()) {
                jsonl_below(&path.join("subagents"), &mut found);
            } else if path.extension().is_some_and(|x| x == "jsonl") && path.is_file() {
                found.push(path);
            }
        }
    }
    found
}

fn jsonl_below(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(files) = fs::read_dir(dir) else { return };
    for file in files.flatten() {
        let path = file.path();
        match file.file_type() {
            Ok(t) if t.is_dir() => jsonl_below(&path, found),
            Ok(t) if t.is_file() && path.extension().is_some_and(|x| x == "jsonl") => found.push(path),
            _ => {}
        }
    }
}

/// The session a sub-agent's transcript belongs to: the folder it is under,
/// `<folder>/<session>/subagents/...`. None for a session's own transcript.
fn agent_of(account_dir: &Path, path: &Path) -> Option<String> {
    let mut parts = path.strip_prefix(account_dir.join("projects")).ok()?.components();
    let (_folder, session, _below) = (parts.next()?, parts.next()?, parts.next()?);
    Some(session.as_os_str().to_string_lossy().into_owned())
}

/// A sub-agent's tokens, counted with the session that started it, as Claude Code
/// counts them. They are kept out of `own_tokens`: its edits are not the session's.
fn add_agent(item: &mut HistoryItem, agent: &Entry) {
    item.tokens.add(&agent.tokens);
    for (model, tokens) in &agent.models {
        item.models.entry(model.clone()).or_default().add(tokens);
    }
    item.hours.extend(agent.hours.iter().copied());
    item.hours.sort_by_key(|(hour, _)| *hour);
    item.hours.dedup_by(|(hour, tokens), (kept, sum)| {
        let same = hour == kept;
        if same {
            sum.add(tokens);
        }
        same
    });
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
        let dir = std::env::temp_dir().join(format!("paddock-history-{}", std::process::id()));
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
            // Published: the tool result has the long link, the frame-link record the short one.
            r#"{"type":"user","timestamp":"2026-11-10T06:21:00.000Z","message":{"role":"user","content":[{"type":"tool_result","content":"Published"}]},"toolUseResult":{"url":"https://claude.ai/code/artifact/2f2f4da7-5f8a","path":"/tmp/plan.html","title":"Plan"}}"#,
            r#"{"type":"frame-link","sessionId":"x","path":"/tmp/plan.html","frameUrl":"https://claude.ai/artifact/6pwcDR94","title":"The plan","artifactCount":1,"timestamp":"2026-11-10T06:22:00.000Z"}"#,
            r#"{"type":"frame-link","artifactCount":1,"sessionId":"x","timestamp":"2026-11-10T06:22:01.000Z"}"#,
            // Opened, not published: no file, so the link is all there is.
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"Opened"}]},"toolUseResult":{"opened":true,"url":"https://claude.ai/artifact/Puen6az3","title":"Other page"}}"#,
            // Links that are only talked about are not artifacts of this conversation.
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"see https://claude.ai/artifact/NotMine1"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]},"toolUseResult":{"url":"https://claude.ai/chat/abc"}}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        let entry = index_transcript(&path);
        assert_eq!(entry.session_id, "0f0f0f0f-aaaa-bbbb-cccc-121212121212");
        assert_eq!(entry.cwd.as_deref(), Some("/work/app"));
        assert_eq!(entry.started_at, Some(1794291596638));
        assert_eq!(entry.prompts, 2);
        assert_eq!(entry.first_prompt.as_deref(), Some("first question"));
        assert_eq!(entry.last_prompts, vec!["first question", "second question"]);
        assert_eq!(entry.prompt_text, "first question\nsecond question");
        assert_eq!(entry.title.as_deref(), Some("Better title"));
        let links: Vec<(&str, &str)> = entry.artifacts.iter().map(|a| (a.url.as_str(), a.title.as_str())).collect();
        assert_eq!(links, [("https://claude.ai/artifact/6pwcDR94", "The plan"), ("https://claude.ai/artifact/Puen6az3", "Other page")]);
        assert_eq!(entry.artifacts[0].updated_at, Some(1794291720000));

        // One reply over three lines counts once; Claude Code's own "<synthetic>" replies not at all.
        let reply = |part: &str| format!(
            r#"{{"type":"assistant","timestamp":"2026-11-10T06:30:00.000Z","message":{{"model":"claude-opus-5-5","id":"msg_01abc","content":[{{"type":"text","text":"{part}"}}],"usage":{{"input_tokens":10,"output_tokens":200,"cache_creation_input_tokens":3000,"cache_read_input_tokens":40000}}}}}}"#
        );
        let mut lines: Vec<String> = ["a", "b", "c"].iter().map(|p| reply(p)).collect();
        lines.push(r#"{"type":"assistant","timestamp":"2026-11-10T07:10:00.000Z","message":{"model":"claude-opus-5-5","id":"msg_02def","usage":{"input_tokens":1,"output_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":100}}}"#.into());
        lines.push(r#"{"type":"assistant","message":{"model":"<synthetic>","id":"msg_03","usage":{"input_tokens":0,"output_tokens":0}}}"#.into());
        fs::write(&path, lines.join("\n")).unwrap();
        let entry = index_transcript(&path);
        assert_eq!(entry.tokens, Tokens { input: 11, output: 205, cache_write: 3000, cache_read: 40100 });
        assert_eq!(entry.models.len(), 1);
        let hours: Vec<i64> = entry.hours.iter().map(|(h, _)| *h).collect();
        assert_eq!(hours, [1794292200000 / 3_600_000, 1794294600000 / 3_600_000]);

        // A long conversation keeps its first and last prompts for searching.
        let many: Vec<String> = (0..50)
            .map(|n| format!(r#"{{"type":"user","cwd":"/work/app","message":{{"role":"user","content":"prompt {n}"}}}}"#))
            .collect();
        fs::write(&path, many.join("\n")).unwrap();
        let entry = index_transcript(&path);
        let kept: Vec<&str> = entry.prompt_text.lines().collect();
        assert_eq!(entry.prompts, 50);
        assert_eq!(kept.len(), 2 * SEARCH_PROMPTS);
        assert_eq!((kept[0], kept[SEARCH_PROMPTS - 1], kept[SEARCH_PROMPTS]), ("prompt 0", "prompt 19", "prompt 30"));
        assert_eq!(kept.last(), Some(&"prompt 49"));
        let _ = fs::remove_dir_all(&dir);
    }
}
