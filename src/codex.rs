//! What only Codex has: its saved conversations ("rollouts"), the names it gives
//! them, its running sessions, and who is signed in. Everything under an
//! account's folder is only ever read.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use memchr::memmem;
use ruzstd::decoding::StreamingDecoder;
use serde_json::{json, Value};

use crate::accounts::Account;
use crate::history::{self, Entry, Tokens};
use crate::loops::Gatherer;
use crate::os;
use crate::tools::{self, AuthInfo};
use crate::util::{err, iso_to_ms, squash, Error, Result};

// ------------------------------------------------------------------- rollouts

/// `<account>/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, compressed with zstd
/// (`.jsonl.zst`) once Codex is done with it. Conversations archived in Codex
/// (`archived_sessions`) are left out, as Codex itself leaves them out of its list.
pub fn transcripts(account_dir: &Path) -> Vec<PathBuf> {
    let mut found: HashMap<String, PathBuf> = HashMap::new();
    let mut folders = vec![(account_dir.join("sessions"), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else { continue };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                if depth < 3 {
                    folders.push((path, depth + 1));
                }
                continue;
            }
            let Some(id) = rollout_id(&path) else { continue };
            // While a finished one is being compressed both copies exist for a moment; keep the newer.
            let newer = found.get(&id).is_none_or(|other| modified(&path) > modified(other));
            if newer {
                found.insert(id, path);
            }
        }
    }
    found.into_values().collect()
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The conversation id in a rollout's file name, or None for any other file.
pub fn rollout_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy();
    let stem = name.strip_suffix(".jsonl.zst").or_else(|| name.strip_suffix(".jsonl"))?;
    if !stem.starts_with("rollout-") || stem.len() < 36 + 9 {
        return None;
    }
    let id = &stem[stem.len() - 36..];
    (id.chars().filter(|c| *c == '-').count() == 4).then(|| id.to_string())
}

/// A rollout's lines, decompressed if need be.
fn open(path: &Path) -> io::Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    if !path.to_string_lossy().ends_with(".zst") {
        return Ok(Box::new(BufReader::with_capacity(1 << 16, file)));
    }
    // A zstd file can hold more than one frame, one after another.
    let mut source = BufReader::new(file);
    let mut out = Vec::new();
    while !source.fill_buf()?.is_empty() {
        let mut frame = StreamingDecoder::new(&mut source).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        frame.read_to_end(&mut out)?;
    }
    Ok(Box::new(Cursor::new(out)))
}

/// A record's kind and what it carries. Rollouts from before mid-2025 have no
/// `payload` wrapper: their records are the payload, and the first line is the meta.
fn split(rec: &Value) -> (&str, &Value) {
    let kind = rec.get("type").and_then(Value::as_str).unwrap_or("");
    match rec.get("payload") {
        Some(payload) => (kind, payload),
        None if kind.is_empty() && rec.get("id").is_some() => ("session_meta", rec),
        None => ("response_item", rec),
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

/// What a review is of, to name it by: `codex review` asks in a sub-agent's
/// thread, so its own conversation has no prompt.
fn review_name(item: &Value) -> Option<String> {
    let review = item.get("item").filter(|i| i.get("type").and_then(Value::as_str) == Some("EnteredReviewMode"))?;
    Some(match text(review, "user_facing_hint").filter(|hint| !hint.is_empty()) {
        Some(hint) => format!("Review of {hint}"),
        None => "Code review".to_string(),
    })
}

/// What the person typed, from a user message; None for what Codex adds to the
/// conversation itself (instructions, the environment, app events).
fn prompt_text(item: &Value) -> Option<String> {
    if item.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    // The desktop app marks what was typed; anything else it sends is machinery.
    if let Some(kinds) = item.pointer("/internal_chat_message_metadata_passthrough/content_item_kinds").and_then(Value::as_array) {
        if !kinds.iter().any(|k| k.as_str() == Some("user.text")) {
            return None;
        }
    }
    let parts: Vec<&str> = item
        .get("content")?
        .as_array()?
        .iter()
        .filter(|p| matches!(p.get("type").and_then(Value::as_str), Some("input_text" | "text")))
        .filter_map(|p| p.get("text").and_then(Value::as_str))
        .collect();
    let text = parts.join(" ");
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with("# AGENTS.md instructions") {
        return None;
    }
    Some(squash(text))
}

/// Tokens as Codex counts them. Its input includes the cached part; here they are apart.
fn tokens(usage: &Value) -> Tokens {
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = count("cached_input_tokens");
    Tokens {
        input: count("input_tokens").saturating_sub(cached),
        output: count("output_tokens"),
        cache_write: count("cache_write_input_tokens"),
        cache_read: cached,
    }
}

fn minus(a: &Tokens, b: &Tokens) -> Tokens {
    Tokens {
        input: a.input.saturating_sub(b.input),
        output: a.output.saturating_sub(b.output),
        cache_write: a.cache_write.saturating_sub(b.cache_write),
        cache_read: a.cache_read.saturating_sub(b.cache_read),
    }
}

/// Tool calls that run a command. "exec" is the desktop app's code mode, which runs commands from a script.
const COMMAND_TOOLS: [&str; 6] = ["shell", "exec_command", "local_shell", "shell_command", "container.exec", "exec"];

/// The patches (`*** Begin Patch` ... `*** End Patch`) in a tool call's input, which
/// may be JSON or a script that quotes them with `\n` for each line break.
fn patches(input: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("*** Begin Patch") {
        let body = &rest[start..];
        let end = body.find("*** End Patch").map_or(body.len(), |e| e + "*** End Patch".len());
        let patch = &body[..end];
        found.push(if patch.contains('\n') {
            patch.to_string()
        } else {
            patch.replace("\\n", "\n").replace("\\t", "\t").replace("\\\"", "\"").replace("\\\\", "\\")
        });
        rest = &body[end..];
    }
    found
}

enum Change {
    /// A file changed in places: each pair is text taken out and the text put in.
    Update(String, Vec<(String, String)>),
    Add(String, String),
}

/// What a patch does to each file. Lines kept as context are left out of both sides.
fn changes(patch: &str) -> Vec<Change> {
    let mut out = Vec::new();
    let mut current: Option<Change> = None;
    let (mut old, mut new) = (String::new(), String::new());
    let flush = |current: &mut Option<Change>, old: &mut String, new: &mut String| {
        if let Some(Change::Update(_, pairs)) = current {
            if !old.is_empty() || !new.is_empty() {
                pairs.push((std::mem::take(old), std::mem::take(new)));
            }
        }
    };
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("*** ") {
            flush(&mut current, &mut old, &mut new);
            out.extend(current.take());
            if let Some(file) = header.strip_prefix("Update File: ") {
                current = Some(Change::Update(file.trim().to_string(), Vec::new()));
            } else if let Some(file) = header.strip_prefix("Add File: ") {
                current = Some(Change::Add(file.trim().to_string(), String::new()));
            }
            continue;
        }
        match &mut current {
            Some(Change::Add(_, content)) => {
                if let Some(added) = line.strip_prefix('+') {
                    content.push_str(added);
                    content.push('\n');
                }
            }
            Some(Change::Update(..)) => {
                if line.starts_with("@@") {
                    flush(&mut current, &mut old, &mut new);
                } else if let Some(taken) = line.strip_prefix('-') {
                    old.push_str(taken);
                    old.push('\n');
                } else if let Some(added) = line.strip_prefix('+') {
                    new.push_str(added);
                    new.push('\n');
                }
            }
            None => {}
        }
    }
    flush(&mut current, &mut old, &mut new);
    out.extend(current);
    out
}

/// A tool call's output as text, and its exit code when it says one.
fn output(item: &Value) -> (String, Option<i64>) {
    let parts: Vec<String> = match item.get("output") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str).map(String::from)).collect(),
        Some(other) => vec![other.get("content").and_then(Value::as_str).unwrap_or("").to_string()],
        None => Vec::new(),
    };
    // Commands answer with JSON: the CLI's shell tool {"output": ..., "metadata": {"exit_code": ...}},
    // and each one a code-mode script runs {"exit_code": ..., "output": ...}, after a line about the script.
    let mut runs: Vec<(String, Option<i64>)> = parts
        .iter()
        .filter_map(|part| {
            let Ok(Value::Object(map)) = serde_json::from_str::<Value>(part) else { return None };
            let out = map.get("output").and_then(Value::as_str)?;
            let code = map.get("exit_code").or_else(|| map.get("metadata").and_then(|m| m.get("exit_code")));
            Some((out.to_string(), code.and_then(Value::as_i64)))
        })
        .collect();
    if let Some(at) = runs.iter().position(|(_, code)| code.is_some_and(|c| c != 0)) {
        return runs.swap_remove(at);
    }
    if let Some(run) = runs.pop() {
        return run;
    }
    let text = parts.join("\n");
    let code = ["Process exited with code ", "Exit code: ", "exit code ", "\"exit_code\":"]
        .iter()
        .find_map(|label| text.find(label).map(|at| &text[at + label.len()..]))
        .and_then(|after| {
            let after = after.trim_start();
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
            digits.parse().ok()
        });
    (text, code)
}

/// Read one rollout into a small summary. Never writes to it.
pub fn index_rollout(path: &Path) -> Entry {
    let mut entry = Entry { session_id: rollout_id(path).unwrap_or_default(), ..Entry::default() };
    let mut typed: Vec<String> = Vec::new();
    let mut work = Gatherer::default();
    let mut model = String::new();
    let mut total = Tokens::default();
    // A side conversation of another one, in the desktop app.
    let mut side = false;
    let Ok(mut reader) = open(path) else {
        entry.unreadable = true;
        return entry;
    };
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
        // Lines can be megabytes of tool output; look before parsing. The record's
        // kind and its payload's kind (and a message's role) come early in the line.
        let head = &line[..line.len().min(320)];
        let wanted = [
            &b"\"session_meta\""[..],
            b"\"turn_context\"",
            b"\"token_count\"",
            b"\"thread_settings_applied\"",
            b"\"role\":\"user\"",
            b"\"EnteredReviewMode\"",
            b"_call\"",
            b"_call_output\"",
        ];
        let first = entry.started_at.is_none() && line.first() == Some(&b'{');
        if !first && !wanted.iter().any(|w| memmem::find(head, w).is_some()) {
            continue;
        }
        let Ok(rec) = serde_json::from_slice::<Value>(&line) else { continue };
        let (kind, item) = split(&rec);
        let at = text(&rec, "timestamp").and_then(|t| iso_to_ms(&t));
        if entry.started_at.is_none() {
            entry.started_at = text(item, "timestamp").and_then(|t| iso_to_ms(&t)).or(at);
        }
        // Work done, as opposed to settings Codex writes again when it migrates a rollout.
        if kind == "response_item" || item.get("type").and_then(Value::as_str) == Some("token_count") {
            entry.last_at = at.or(entry.last_at);
        }
        match (kind, item.get("type").and_then(Value::as_str).unwrap_or("")) {
            ("session_meta", _) => {
                if entry.cwd.is_none() {
                    entry.cwd = text(item, "cwd");
                }
                if entry.session_id.is_empty() {
                    entry.session_id = text(item, "id").unwrap_or_default();
                }
                // A sub-agent's thread is part of the conversation that started it.
                let spawned = item.get("thread_source").and_then(Value::as_str) == Some("subagent") || item.pointer("/source/subagent").is_some();
                entry.hidden = entry.hidden || spawned;
                side = side || item.get("attached_thread").and_then(Value::as_bool).unwrap_or(false);
            }
            ("turn_context", _) => {
                model = text(item, "model").unwrap_or(model);
                if entry.cwd.is_none() {
                    entry.cwd = text(item, "cwd");
                }
            }
            ("event_msg", "thread_settings_applied") => {
                model = item.pointer("/thread_settings/model").and_then(Value::as_str).map(String::from).unwrap_or(model);
            }
            // Codex writes the running total after each reply (sometimes twice); the
            // reply's own use is what the total grew by.
            ("event_msg", "token_count") => {
                let Some(now) = item.pointer("/info/total_token_usage").map(tokens) else { continue };
                let last = item.pointer("/info/last_token_usage").map(tokens);
                // A forked conversation's first total carries its parent's, which the parent
                // already counts; and a total that went down started again. Either way the
                // reply's own figure is the one to take.
                let restarted = total == Tokens::default() || now.input + now.cache_read < total.input + total.cache_read;
                let grew = if restarted { last.unwrap_or(now) } else { minus(&now, &total) };
                total = now;
                if grew == Tokens::default() {
                    continue;
                }
                work.reply();
                history::note_tokens(&mut entry, if model.is_empty() { "codex" } else { &model }, &grew, at);
            }
            ("response_item", "message") => {
                if let Some(prompt) = prompt_text(item) {
                    work.prompt(&prompt);
                    history::note_prompt(&mut entry, &mut typed, &prompt);
                }
            }
            ("event_msg", "item_completed") if entry.first_prompt.is_none() => {
                entry.first_prompt = review_name(item);
            }
            ("response_item", "function_call" | "custom_tool_call" | "local_shell_call") => {
                let name = text(item, "name").unwrap_or_default();
                let id = text(item, "call_id").unwrap_or_default();
                if COMMAND_TOOLS.contains(&name.as_str()) || item.get("type").and_then(Value::as_str) == Some("local_shell_call") {
                    work.command(&id);
                }
                let input = text(item, "input").or_else(|| text(item, "arguments")).unwrap_or_default();
                for patch in patches(&input) {
                    for change in changes(&patch) {
                        match change {
                            Change::Update(file, pairs) => work.edited(&file, &pairs),
                            Change::Add(file, content) => work.wrote(&file, &content),
                        }
                    }
                }
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                let (out, code) = output(item);
                if code.is_some_and(|c| c != 0) {
                    work.failed(&text(item, "call_id").unwrap_or_default(), &out);
                }
            }
            _ => {}
        }
    }
    // A side conversation opened and left without a word is no conversation of its own.
    entry.hidden = entry.hidden || (side && entry.prompts == 0);
    entry.title = entry.first_prompt.as_deref().map(|p| p.chars().take(80).collect()).filter(|p: &String| !p.is_empty());
    entry.prompt_text = typed.join("\n");
    entry.work = work.finish();
    entry
}

/// The names Codex gives its conversations (and the ones it was told), by id.
pub fn thread_names(account_dir: &Path) -> HashMap<String, String> {
    let Ok(content) = fs::read_to_string(account_dir.join("session_index.jsonl")) else { return HashMap::new() };
    // One line per change, oldest first, so a later line wins.
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|rec| Some((text(&rec, "id")?, text(&rec, "thread_name")?)))
        .collect()
}

// ------------------------------------------------------------- live sessions

/// A Codex session running on this machine.
#[derive(Debug, Clone)]
pub struct Running {
    /// Its `codex` command, in a terminal. None for a turn still running in
    /// Codex's background service after its window was closed.
    pub pid: Option<u32>,
    /// The account folder it runs out of, as registered.
    pub account_dir: PathBuf,
    pub rollout: PathBuf,
    pub session_id: String,
    pub cwd: String,
    pub started_at: Option<i64>,
    /// With no window: its turn is over, but a command it started still runs.
    pub commands_only: bool,
}

/// Options of the `codex` command that take a value, so the value is not taken for a command.
const VALUE_OPTIONS: &[&str] = &[
    "-c", "--config", "--enable", "--disable", "--remote", "--remote-auth-token-env", "-i", "--image", "-m", "--model",
    "--local-provider", "-p", "--profile", "-s", "--sandbox", "-C", "--cd", "--add-dir", "-a", "--ask-for-approval",
];
/// Its commands that hold a conversation.
const SESSION_COMMANDS: &[&str] = &["resume", "fork", "exec", "e", "review"];
/// And the ones that don't: its background service, helpers, one-off chores.
const OTHER_COMMANDS: &[&str] = &[
    "agents", "login", "logout", "mcp", "mcp-server", "plugin", "app-server", "remote-control", "app", "completion",
    "update", "doctor", "sandbox", "debug", "apply", "a", "queue", "archive", "delete", "unarchive", "migrate-rollouts",
    "migrate-nfs-db", "cloud", "exec-server", "features", "help", "proto", "responses-api-proxy", "stdio-to-uds",
];

/// A `codex` command line that holds a conversation.
#[derive(Debug, Default, PartialEq)]
struct Invocation {
    /// What follows `codex resume`: the conversation's id or name, perhaps then a prompt.
    resumes: Option<String>,
    /// The folder it was pointed at with `-C`.
    cd: Option<String>,
}

/// What a command line runs, if it is a Codex session: not the desktop app's
/// server, the background service, a helper (`codex-code-mode-host`, ...) or a
/// command like `codex login`. Words of a prompt are never read as a command.
fn invocation(args: &str) -> Option<Invocation> {
    let mut words = args.split_whitespace();
    let program = words.next()?;
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program).trim_end_matches(".exe");
    // Older npm installs ran the binary under its platform's name.
    if program != "codex" && !program.starts_with("codex-aarch64") && !program.starts_with("codex-x86_64") {
        return None;
    }
    let mut found = Invocation::default();
    let mut command: Option<&str> = None;
    while let Some(word) = words.next() {
        if word == "--" {
            break;
        }
        if word.len() > 1 && word.starts_with('-') {
            if VALUE_OPTIONS.contains(&word) {
                let value = words.next();
                if matches!(word, "-C" | "--cd") {
                    found.cd = value.map(str::to_string);
                }
            } else if let Some(dir) = word.strip_prefix("--cd=") {
                found.cd = Some(dir.to_string());
            }
            continue;
        }
        match command {
            None if SESSION_COMMANDS.contains(&word) => command = Some(word),
            None if OTHER_COMMANDS.contains(&word) => return None,
            Some("resume") => {
                found.resumes = Some(std::iter::once(word).chain(words).collect::<Vec<_>>().join(" "));
                break;
            }
            // A prompt.
            _ => break,
        }
    }
    Some(found)
}

fn is_session_process(args: &str) -> bool {
    invocation(args).is_some()
}

/// The folder a session works in: the one it was given with `-C`, else its own.
fn place(inv: &Invocation, files: Option<&os::OpenFiles>) -> Option<PathBuf> {
    let cwd = files.and_then(|f| f.cwd.clone())?;
    Some(match &inv.cd {
        Some(dir) => cwd.join(dir),
        None => cwd,
    })
}

/// The first record of a rollout: where it ran and when it began.
fn meta(rollout: &Path) -> (String, Option<i64>) {
    let rec = first_record(rollout);
    let (_, item) = split(&rec);
    (text(item, "cwd").unwrap_or_default(), text(item, "timestamp").and_then(|t| iso_to_ms(&t)))
}

fn first_record(rollout: &Path) -> Value {
    let Ok(mut reader) = open(rollout) else { return Value::Null };
    let mut first = String::new();
    let _ = reader.read_line(&mut first);
    serde_json::from_str(&first).unwrap_or(Value::Null)
}

/// A sub-agent's thread (a review's too): part of the conversation that started it, never a session of its own.
fn spawned(rollout: &Path) -> bool {
    let rec = first_record(rollout);
    let (_, item) = split(&rec);
    item.get("thread_source").and_then(Value::as_str) == Some("subagent") || item.pointer("/source/subagent").is_some()
}

/// What the person first typed in a conversation, for a running one Codex has not named yet.
pub fn first_prompt(rollout: &Path) -> Option<String> {
    let reader = open(rollout).ok()?;
    reader
        .split(b'\n')
        .take(400)
        .map_while(|line| line.ok())
        .filter(|line| {
            let head = &line[..line.len().min(320)];
            memmem::find(head, b"\"role\":\"user\"").is_some() || memmem::find(head, b"\"EnteredReviewMode\"").is_some()
        })
        .filter_map(|line| serde_json::from_slice::<Value>(&line).ok())
        .find_map(|rec| prompt_text(split(&rec).1).or_else(|| review_name(split(&rec).1)))
        .map(|p| p.chars().take(80).collect())
}

/// The newest `sessions/YYYY/MM/DD` folder of an account, where a new conversation's
/// file appears (which changes the folder's modified time).
pub fn newest_day(account_dir: &Path) -> Option<PathBuf> {
    let newest = |dir: &Path| fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).max();
    newest(&newest(&newest(&account_dir.join("sessions"))?)?)
}

/// The ids of running Codex sessions, cheaply, to notice one starting or ending.
pub fn session_pids() -> Vec<u32> {
    os::processes().into_iter().filter(|p| is_session_process(&p.args)).map(|p| p.pid).collect()
}

/// The Codex windows open on this machine.
#[derive(Debug, Clone, Default)]
pub struct Windows {
    pub sessions: Vec<Running>,
    /// The folders of windows whose conversation was not found.
    pub unmatched: Vec<PathBuf>,
}

/// Codex sessions running out of the given account folders. With `--no-daemon`
/// (and before Codex 0.160) a session writes its rollout itself and is matched
/// by the file it holds open. Otherwise its turns run in Codex's background
/// service, which holds the file, and it is matched by the conversation it
/// resumes, or by its folder and the conversations started there since it started.
pub fn running(account_dirs: &[PathBuf]) -> Windows {
    let procs: Vec<(os::Process, Invocation)> =
        os::processes().into_iter().filter_map(|p| invocation(&p.args).map(|inv| (p, inv))).collect();
    if procs.is_empty() {
        return Windows::default();
    }
    // The system names open files by their real path, links resolved.
    let real: Vec<(PathBuf, &PathBuf)> = account_dirs.iter().map(|d| (fs::canonicalize(d).unwrap_or_else(|_| d.clone()), d)).collect();
    let owner = |file: &Path| real.iter().find(|(r, _)| file.starts_with(r)).map(|(_, d)| (*d).clone());
    let pids: Vec<u32> = procs.iter().map(|(p, _)| p.pid).collect();
    let open = os::open_files(&pids);
    let mut found: Vec<Running> = Vec::new();
    let mut unmatched: Vec<PathBuf> = Vec::new();
    let add = |proc: &os::Process, rollout: PathBuf, found: &mut Vec<Running>| -> bool {
        let rollout = fs::canonicalize(&rollout).unwrap_or(rollout);
        let (Some(session_id), Some(account_dir)) = (rollout_id(&rollout), owner(&rollout)) else { return false };
        if found.iter().any(|r| r.session_id == session_id) {
            return false;
        }
        let (cwd, started) = meta(&rollout);
        found.push(Running { pid: Some(proc.pid), account_dir, rollout, session_id, cwd, started_at: started.or(proc.started_at), commands_only: false });
        true
    };
    let mut by_folder: Vec<(&os::Process, PathBuf)> = Vec::new();
    for (proc, inv) in &procs {
        let files = open.get(&proc.pid);
        let held = files.into_iter().flat_map(|f| f.files.iter()).find(|f| rollout_id(f).is_some() && owner(f).is_some()).cloned();
        match held.or_else(|| inv.resumes.as_deref().and_then(|named| resumed(account_dirs, named))) {
            Some(rollout) => {
                add(proc, rollout, &mut found);
            }
            None => by_folder.extend(place(inv, files).map(|place| (proc, place))),
        }
    }
    // Windows sharing a folder each get their own conversation: the newest window first.
    by_folder.sort_by_key(|(proc, _)| std::cmp::Reverse(proc.started_at));
    for (proc, place) in by_folder {
        let taken: Vec<String> = found.iter().map(|r| r.session_id.clone()).collect();
        if !newest_in(account_dirs, &place, proc.started_at, &taken).is_some_and(|rollout| add(proc, rollout, &mut found)) {
            unmatched.push(place);
        }
    }
    Windows { sessions: found, unmatched }
}

/// The rollout of the conversation `codex resume` was given, by id or by the
/// name Codex gave it. A command line has lost its quotes, so a name is the
/// longest one the words after `resume` begin with.
fn resumed(account_dirs: &[PathBuf], words: &str) -> Option<PathBuf> {
    let first = words.split(' ').next()?;
    account_dirs.iter().find_map(|dir| {
        let rollouts = transcripts(dir);
        let by_id = |id: &str| rollouts.iter().find(|p| rollout_id(p).as_deref() == Some(id)).cloned();
        by_id(first).or_else(|| {
            let names = thread_names(dir);
            let starts = |name: &str| words.strip_prefix(name).is_some_and(|rest| rest.is_empty() || rest.starts_with(' '));
            let (id, _) = names.iter().filter(|(_, name)| !name.is_empty() && starts(name)).max_by_key(|(_, name)| name.len())?;
            by_id(id)
        })
    })
}

/// Rollouts already found not to be running in the background service, as they were then.
static NOT_IN_BACKGROUND: Mutex<Vec<(PathBuf, SystemTime)>> = Mutex::new(Vec::new());

/// Turns still running in Codex's background service after their window was
/// closed, and commands still running there after their turn ended. The rollout
/// says a turn began and has not ended, or that its last turn left a command
/// running; the service is asked whether it really still runs, since a crash
/// leaves rollouts like that too.
pub fn background(acc: &Account, windows: &Windows) -> Vec<Running> {
    let shown: Vec<String> = windows.sessions.iter().map(|r| r.session_id.clone()).collect();
    let day_ago = SystemTime::now() - Duration::from_secs(24 * 3600);
    let known = NOT_IN_BACKGROUND.lock().map(|k| k.clone()).unwrap_or_default();
    let candidates: Vec<(String, PathBuf, SystemTime)> = transcripts(Path::new(&acc.dir))
        .into_iter()
        .filter(|p| !p.to_string_lossy().ends_with(".zst"))
        .filter_map(|p| Some((rollout_id(&p)?, modified(&p)?, p)))
        .filter(|(id, at, p)| *at >= day_ago && !shown.contains(id) && !known.contains(&(p.clone(), *at)))
        .filter(|(_, _, p)| (running_turn(p).is_some() || left_command(p)) && !spawned(p))
        // Perhaps that window's own: it is not said to be closed.
        .filter(|(_, _, p)| !windows.unmatched.iter().any(|place| same_place(place, Path::new(&meta(p).0))))
        .map(|(id, at, p)| (id, p, at))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let asks: Vec<Value> = candidates
        .iter()
        .flat_map(|(id, _, _)| {
            [json!({"method": "thread/read", "params": {"threadId": id}}), json!({"method": "thread/backgroundTerminals/list", "params": {"threadId": id}})]
        })
        .collect();
    // No answer says nothing about them: ask again next time.
    let Ok(replies) = service(acc, &asks) else { return Vec::new() };
    let mut found = Vec::new();
    for (i, (session_id, rollout, at)) in candidates.into_iter().enumerate() {
        let state = replies.get(2 * i).and_then(|r| r.pointer("/result/thread/status/type")).and_then(Value::as_str);
        let commands = replies.get(2 * i + 1).and_then(|r| r.pointer("/result/data")).and_then(Value::as_array).is_some_and(|d| !d.is_empty());
        if state != Some("active") && !commands {
            if let Ok(mut k) = NOT_IN_BACKGROUND.lock() {
                k.retain(|(_, seen)| *seen >= day_ago);
                k.push((rollout, at));
            }
            continue;
        }
        let (cwd, started_at) = meta(&rollout);
        let commands_only = state != Some("active");
        found.push(Running { pid: None, account_dir: PathBuf::from(&acc.dir), rollout, session_id, cwd, started_at, commands_only });
    }
    found
}

fn same_place(a: &Path, b: &Path) -> bool {
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(a) == real(b)
}

/// The conversation a window opened in `cwd` at `since` is on: the newest one
/// started there since, not taken by another window; else, for one it went back
/// to from inside, the one written to last since.
fn newest_in(account_dirs: &[PathBuf], cwd: &Path, since: Option<i64>, taken: &[String]) -> Option<PathBuf> {
    let since = since?;
    let since_time = std::time::UNIX_EPOCH + Duration::from_millis(since.max(0) as u64);
    let here: Vec<(SystemTime, Option<i64>, PathBuf)> = account_dirs
        .iter()
        .flat_map(|dir| transcripts(dir))
        .filter(|p| !p.to_string_lossy().ends_with(".zst"))
        .filter(|p| rollout_id(p).is_some_and(|id| !taken.contains(&id)))
        .filter_map(|p| Some((modified(&p)?, p)))
        .filter(|(at, p)| *at >= since_time && !spawned(p))
        .filter_map(|(at, p)| {
            let (place, started) = meta(&p);
            same_place(Path::new(&place), cwd).then_some((at, started, p))
        })
        .collect();
    // Process start times are whole seconds.
    let started_since = |s: &Option<i64>| s.is_some_and(|s| s >= since - 1000);
    here.iter()
        .filter(|(_, started, _)| started_since(started))
        .max_by_key(|(_, started, _)| *started)
        .or_else(|| here.iter().max_by_key(|(at, _, _)| *at))
        .map(|(_, _, p)| p.clone())
}

/// The end of a rollout, where what it is doing now is.
fn tail(rollout: &Path) -> Vec<u8> {
    const TAIL: u64 = 256 * 1024;
    let Ok(mut file) = File::open(rollout) else { return Vec::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(TAIL)));
    let mut tail = Vec::new();
    let _ = file.read_to_end(&mut tail);
    tail
}

/// What a running session is doing, from the end of its rollout: working on a
/// turn, waiting for the person to approve or answer something, or idle.
pub fn status(rollout: &Path) -> String {
    for line in tail(rollout).split(|b| *b == b'\n').rev() {
        let head = &line[..line.len().min(200)];
        let says = |what: &[u8]| memmem::find(head, what).is_some();
        if says(b"\"task_started\"") {
            return "working".into();
        }
        if says(b"\"task_complete\"") || says(b"\"turn_aborted\"") {
            return "idle".into();
        }
        if says(b"_approval_request\"") || says(b"\"request_user_input") {
            return "blocked".into();
        }
    }
    "idle".into()
}

/// What sessions whose rollout says they are working are really doing, asked of
/// the background service: it does not write down that it is waiting for an
/// approval, and writes the end of a turn a moment late. Sessions it does not
/// run (`--no-daemon`), or when it does not answer, keep what their rollout says.
pub fn live_statuses(acc: &Account, sessions: &[(String, String)]) -> HashMap<String, String> {
    let working: Vec<&String> = sessions.iter().filter(|(_, status)| status == "working").map(|(id, _)| id).collect();
    if working.is_empty() {
        return HashMap::new();
    }
    let asks: Vec<Value> = working.iter().map(|id| json!({"method": "thread/read", "params": {"threadId": id}})).collect();
    let Ok(replies) = service(acc, &asks) else { return HashMap::new() };
    working
        .into_iter()
        .zip(replies)
        .filter_map(|(id, reply)| {
            let state = reply.pointer("/result/thread/status")?;
            let status = match state.get("type").and_then(Value::as_str)? {
                "active" if state.get("activeFlags").and_then(Value::as_array).is_some_and(|f| !f.is_empty()) => "blocked",
                "active" => "working",
                "idle" => "idle",
                _ => return None,
            };
            Some((id.clone(), status.to_string()))
        })
        .collect()
}

/// Whether the last turn of a conversation left a command running, as its output says
/// ("Process running with session ID", or a `session_id` instead of an exit code).
fn left_command(rollout: &Path) -> bool {
    for line in tail(rollout).split(|b| *b == b'\n').rev() {
        let head = &line[..line.len().min(200)];
        if memmem::find(head, b"\"task_started\"").is_some() {
            return false;
        }
        if memmem::find(head, b"_call_output\"").is_some() && (memmem::find(line, b"session ID").is_some() || memmem::find(line, b"session_id").is_some()) {
            return true;
        }
    }
    false
}

/// The turn a conversation is in the middle of, from the end of its rollout.
fn running_turn(rollout: &Path) -> Option<String> {
    for line in tail(rollout).split(|b| *b == b'\n').rev() {
        let head = &line[..line.len().min(200)];
        if memmem::find(head, b"\"task_complete\"").is_some() || memmem::find(head, b"\"turn_aborted\"").is_some() {
            return None;
        }
        if memmem::find(head, b"\"task_started\"").is_some() {
            let rec: Value = serde_json::from_slice(line).ok()?;
            return text(split(&rec).1, "turn_id");
        }
    }
    None
}

/// Make sure `pid` still is the Codex session holding this conversation before
/// anything is sent to it. True when it writes the conversation itself (`--no-daemon`).
pub fn verify(pid: u32, session_id: &str, rollout: &Path, cwd: &str) -> Result<bool> {
    let Some(proc) = os::processes().into_iter().find(|p| p.pid == pid) else {
        return err("That session has already ended.");
    };
    let Some(inv) = invocation(&proc.args) else {
        return err("That process is not a Codex session any more. Nothing was stopped.");
    };
    let open = os::open_files(&[pid]);
    let files = open.get(&pid);
    let holds = files.is_some_and(|f| f.files.iter().any(|p| same_place(p, rollout)));
    // `<account>/sessions/YYYY/MM/DD/<rollout>`: resumed by its id, or by its title.
    let account_dir: Vec<PathBuf> = rollout.ancestors().nth(5).map(Path::to_path_buf).into_iter().collect();
    let names_it = inv.resumes.as_deref().is_some_and(|words| {
        words.split(' ').next() == Some(session_id) || resumed(&account_dir, words).is_some_and(|p| same_place(&p, rollout))
    });
    let same_folder = place(&inv, files).is_some_and(|p| same_place(&p, Path::new(cwd)));
    if !holds && !names_it && !same_folder {
        return err("That process now belongs to a different session. Refresh and try again.");
    }
    Ok(holds)
}

/// Stop the turn a conversation is in the middle of, and the commands it left
/// running, through the background service, which carries on with both after the
/// session's window is closed. True when there was anything to stop.
pub fn interrupt(acc: &Account, session_id: &str, rollout: &Path) -> Result<bool> {
    let turn = running_turn(rollout);
    let mut requests: Vec<Value> = turn.iter().map(|turn| json!({"method": "turn/interrupt", "params": {"threadId": session_id, "turnId": turn}})).collect();
    // The commands it started run on in "background terminals" after the turn (until /stop).
    requests.push(json!({"method": "thread/backgroundTerminals/list", "params": {"threadId": session_id}}));
    requests.push(json!({"method": "thread/backgroundTerminals/clean", "params": {"threadId": session_id}}));
    let replies = match service(acc, &requests) {
        Ok(replies) => replies,
        // No service running: nothing of this conversation's is running in it either.
        Err(_) if turn.is_none() => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut replies = replies.iter();
    if turn.is_some() {
        if let Some(problem) = replies.next().and_then(|r| r.pointer("/error/message")).and_then(Value::as_str) {
            return err(format!("Codex would not stop its turn: {problem}"));
        }
    }
    // A Codex without background terminals answers these with an error: nothing to stop.
    let commands = replies.next().and_then(|r| r.pointer("/result/data")).and_then(Value::as_array).map_or(0, Vec::len);
    let deadline = Instant::now() + Duration::from_secs(5);
    while turn.is_some() && running_turn(rollout).is_some() {
        if Instant::now() > deadline {
            return err("Codex did not stop its turn within 5 seconds.");
        }
        thread::sleep(Duration::from_millis(200));
    }
    Ok(turn.is_some() || commands > 0)
}

// ------------------------------------------------------ the background service

/// Ask Codex's background service (the `app-server` daemon that sessions started
/// in a terminal run their turns in) through `codex app-server proxy`, which
/// speaks WebSocket on its input and output. The answers come back in the order
/// asked. Nothing is started: with no service running for the account this fails.
fn service(acc: &Account, requests: &[Value]) -> Result<Vec<Value>> {
    let mut cmd = tools::command(acc)?;
    cmd.args(["app-server", "proxy"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| Error(format!("Could not run codex: {e}")))?;
    let (Some(mut input), Some(mut output)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return err("Could not talk to codex.");
    };
    let requests = requests.to_vec();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(converse(&mut input, &mut output, &requests));
    });
    let answer = rx.recv_timeout(Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();
    match answer {
        Ok(Ok(replies)) => Ok(replies),
        Ok(Err(e)) => err(format!("Codex's background service did not answer ({e}).")),
        Err(_) => err("Codex's background service did not answer within 10 seconds."),
    }
}

fn converse(input: &mut impl Write, output: &mut impl Read, requests: &[Value]) -> io::Result<Vec<Value>> {
    input.write_all(
        b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
          Sec-WebSocket-Key: Y2Mtc2Vzc2lvbnMtY29kZXg=\r\nSec-WebSocket-Version: 13\r\n\r\n",
    )?;
    input.flush()?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        output.read_exact(&mut byte)?;
        head.push(byte[0]);
        if head.len() > 16 * 1024 {
            return Err(io::Error::other("no answer to the handshake"));
        }
    }
    if !head.starts_with(b"HTTP/1.1 101") {
        return Err(io::Error::other("the service turned the connection down"));
    }
    let id = |m: &Value| m.get("id").and_then(Value::as_u64);
    // Background terminals are among the "experimental" requests, offered only to clients that ask.
    let hello = json!({"id": 0, "method": "initialize", "params": {
        "clientInfo": {"name": "paddock", "version": env!("CARGO_PKG_VERSION")},
        "capabilities": {"experimentalApi": true},
    }});
    send_message(input, &hello)?;
    while id(&read_message(output)?) != Some(0) {}
    send_message(input, &json!({"method": "initialized"}))?;
    for (i, request) in requests.iter().enumerate() {
        let mut request = request.clone();
        request["id"] = json!(i + 1);
        send_message(input, &request)?;
    }
    let mut replies = vec![Value::Null; requests.len()];
    let mut left = requests.len();
    while left > 0 {
        let message = read_message(output)?;
        let Some(i) = id(&message).and_then(|i| usize::try_from(i).ok()).filter(|i| (1..=requests.len()).contains(i)) else { continue };
        if replies[i - 1].is_null() {
            replies[i - 1] = message;
            left -= 1;
        }
    }
    Ok(replies)
}

fn send_message(input: &mut impl Write, message: &Value) -> io::Result<()> {
    let data = serde_json::to_vec(message)?;
    // A client masks what it sends; the mask needs no secrecy here.
    let mask = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1).to_be_bytes();
    let mut frame = vec![0x81];
    match data.len() {
        n if n < 126 => frame.push(0x80 | n as u8),
        n if n <= 0xFFFF => {
            frame.push(0x80 | 126);
            frame.extend((n as u16).to_be_bytes());
        }
        n => {
            frame.push(0x80 | 127);
            frame.extend((n as u64).to_be_bytes());
        }
    }
    frame.extend(mask);
    frame.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    input.write_all(&frame)?;
    input.flush()
}

/// The next JSON message, skipping pings and anything else that is not text.
fn read_message(output: &mut impl Read) -> io::Result<Value> {
    loop {
        let mut message = Vec::new();
        let mut opcode = 0;
        loop {
            let mut head = [0u8; 2];
            output.read_exact(&mut head)?;
            let len = match head[1] & 0x7F {
                126 => {
                    let mut b = [0u8; 2];
                    output.read_exact(&mut b)?;
                    u64::from(u16::from_be_bytes(b))
                }
                127 => {
                    let mut b = [0u8; 8];
                    output.read_exact(&mut b)?;
                    u64::from_be_bytes(b)
                }
                n => u64::from(n),
            };
            if len > 64 * 1024 * 1024 {
                return Err(io::Error::other("an answer too large to read"));
            }
            let mask = if head[1] & 0x80 != 0 {
                let mut m = [0u8; 4];
                output.read_exact(&mut m)?;
                Some(m)
            } else {
                None
            };
            let mut data = vec![0u8; len as usize];
            output.read_exact(&mut data)?;
            if let Some(m) = mask {
                data.iter_mut().enumerate().for_each(|(i, b)| *b ^= m[i % 4]);
            }
            if head[0] & 0x0F != 0 {
                opcode = head[0] & 0x0F;
            }
            message.extend(data);
            if head[0] & 0x80 != 0 {
                break;
            }
        }
        match opcode {
            1 => return serde_json::from_slice(&message).map_err(io::Error::other),
            8 => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the service closed the connection")),
            _ => continue,
        }
    }
}

// ----------------------------------------------------------------- signing in

/// The claims in the middle of a JSON web token (no checking: only the email and plan are read).
fn token_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let mut bits: u32 = 0;
    let mut count = 0;
    let mut bytes = Vec::new();
    for c in payload.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    serde_json::from_slice(&bytes).ok()
}

/// Who an account is signed in as. Read from its `auth.json` (the email and plan
/// in its identity token, nothing else), or asked of `codex login status` when
/// Codex keeps the login in the system's keychain instead.
pub fn auth_status(acc: &Account) -> AuthInfo {
    let mut info = AuthInfo::default();
    let auth = crate::store::read_json_or_null(&Path::new(&acc.dir).join("auth.json"));
    if auth.is_object() {
        if let Some(claims) = auth.pointer("/tokens/id_token").and_then(Value::as_str).and_then(token_claims) {
            info.logged_in = true;
            info.email = text(&claims, "email");
            let plan = claims.pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type").and_then(Value::as_str);
            info.plan = plan.map(|p| format!("ChatGPT {p}"));
        } else if auth.get("OPENAI_API_KEY").and_then(Value::as_str).is_some_and(|k| !k.is_empty()) {
            info.logged_in = true;
            info.plan = Some("API key".into());
        }
        return info;
    }
    match tools::run(acc, &["login", "status"], 20) {
        // It prints "Logged in using ChatGPT" or "... an API key - sk-...": only the first words are kept.
        Ok(out) => {
            let said = format!("{}\n{}", out.stdout, out.stderr);
            info.logged_in = said.contains("Logged in");
            if said.contains("ChatGPT") {
                info.plan = Some("ChatGPT".into());
            } else if said.contains("API key") {
                info.plan = Some("API key".into());
            }
        }
        Err(e) => info.error = Some(e.0),
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("paddock-codex-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rollout_names() {
        let id = "01a1ba8e-caeb-7c71-8d13-f9a0ff71750c";
        assert_eq!(rollout_id(Path::new(&format!("/x/rollout-2026-11-08T15-18-02-{id}.jsonl"))).as_deref(), Some(id));
        assert_eq!(rollout_id(Path::new(&format!("/x/rollout-2026-11-08T15-18-02-{id}.jsonl.zst"))).as_deref(), Some(id));
        assert_eq!(rollout_id(Path::new("/x/session_index.jsonl")), None);
        assert_eq!(rollout_id(Path::new(&format!("/x/rollout-2026-11-08T15-18-02-{id}.json"))), None);
    }

    #[test]
    fn prompts_skip_what_codex_adds() {
        let user = |text: &str| json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]});
        assert_eq!(prompt_text(&user("  fix   the bug ")), Some("fix the bug".into()));
        assert_eq!(prompt_text(&user("<environment_context>\n<cwd>/x</cwd></environment_context>")), None);
        assert_eq!(prompt_text(&user("# AGENTS.md instructions for /x\n\nbe brief")), None);
        let mut app = user("an app event");
        app["internal_chat_message_metadata_passthrough"] = json!({"content_item_kinds": ["additional_content.codex_apps"]});
        assert_eq!(prompt_text(&app), None);
        app["internal_chat_message_metadata_passthrough"] = json!({"content_item_kinds": ["user.text"]});
        assert_eq!(prompt_text(&app), Some("an app event".into()));
        assert_eq!(prompt_text(&json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hi"}]})), None);
    }

    #[test]
    fn patches_become_edits() {
        let patch = "*** Begin Patch\n*** Update File: src/app.rs\n@@ fn main\n-    old();\n+    new();\n context\n@@\n-a\n*** Add File: notes.md\n+one\n+two\n*** Delete File: gone.rs\n*** End Patch";
        let changes = changes(&patches(patch)[0]);
        assert_eq!(changes.len(), 2);
        match &changes[0] {
            Change::Update(file, pairs) => {
                assert_eq!(file, "src/app.rs");
                assert_eq!(pairs, &[("    old();\n".to_string(), "    new();\n".to_string()), ("a\n".to_string(), String::new())]);
            }
            _ => panic!("expected an update"),
        }
        assert!(matches!(&changes[1], Change::Add(file, content) if file == "notes.md" && content == "one\ntwo\n"));
        // Quoted inside a script or JSON, with \n for each line break.
        let quoted = r#"await tools.apply_patch("*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch")"#;
        assert_eq!(patches(quoted), ["*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch"]);
    }

    #[test]
    fn outputs_say_how_commands_ended() {
        let cli = json!({"output": "{\"output\":\"boom\\n\",\"metadata\":{\"exit_code\":1,\"duration_seconds\":0.1}}"});
        assert_eq!(output(&cli), ("boom\n".into(), Some(1)));
        let app = json!({"output": [{"type": "input_text", "text": "Script completed\n"}, {"type": "input_text", "text": "Process exited with code 2\nError: x"}]});
        assert_eq!(output(&app).1, Some(2));
        assert_eq!(output(&json!({"output": "fine"})).1, None);
        // Code mode, as the CLI writes it: the command's own output, without the run's timings.
        let run = |chunk: &str| json!({"output": [{"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
            {"type": "input_text", "text": format!("{{\"chunk_id\":\"{chunk}\",\"wall_time_seconds\":0.000009,\"exit_code\":1,\"output\":\"Traceback (most recent call last):\\n  File \\\"check.py\\\", line 3\\nAssertionError: add is wrong\\n\"}}")}]});
        assert_eq!(output(&run("b519ee")).1, Some(1));
        assert_eq!(output(&run("b519ee")).0, output(&run("e27bd7")).0);
        assert_eq!(crate::loops::code_error(&output(&run("b519ee")).0).as_deref(), Some("assertionerror: add is wrong"));
    }

    #[test]
    fn reviews_are_named_by_what_they_review() {
        let dir = temp("review");
        let path = dir.join("rollout-2026-11-17T23-27-51-01a1eaa8-785e-79d0-874d-7bc312ed0794.jsonl");
        let lines = [
            json!({"timestamp": "2026-11-17T17:57:51.000Z", "type": "session_meta", "payload": {"id": "01a1eaa8-785e-79d0-874d-7bc312ed0794", "cwd": "/work/app", "source": "exec"}}),
            json!({"timestamp": "2026-11-17T17:57:52.000Z", "type": "event_msg", "payload": {"type": "item_completed", "thread_id": "01a1eaa8-785e-79d0-874d-7bc312ed0794", "turn_id": "01a1eaa8-7881-7222-b692-f5b79f058d0a",
                "item": {"type": "EnteredReviewMode", "id": "01a1eaa8-7885-7831-a013-ca9ceef19dd6", "target": {"type": "uncommittedChanges"}, "user_facing_hint": "current changes"}}}),
        ];
        fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        assert_eq!(first_prompt(&path).as_deref(), Some("Review of current changes"));
        let entry = index_rollout(&path);
        assert_eq!((entry.title.as_deref(), entry.prompts), (Some("Review of current changes"), 0));
    }

    #[test]
    fn indexes_a_rollout() {
        let dir = temp("index");
        let id = "01a1ba8e-caeb-7c71-8d13-f9a0ff71750c";
        let day = dir.join("sessions/2026/11/08");
        fs::create_dir_all(&day).unwrap();
        let path = day.join(format!("rollout-2026-11-08T15-18-02-{id}.jsonl"));
        let usage = |input: u64, cached: u64, out: u64| {
            json!({"type": "event_msg", "timestamp": "2026-11-08T09:50:00.000Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": input, "cached_input_tokens": cached, "output_tokens": out}}}})
        };
        let lines = [
            json!({"timestamp": "2026-11-08T09:48:02.670Z", "type": "session_meta", "payload": {"id": id, "cwd": "/work/app", "timestamp": "2026-11-08T09:48:02.669Z"}}),
            json!({"type": "turn_context", "payload": {"cwd": "/work/app", "model": "gpt-6"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>x</environment_context>"}]}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "first question"}]}}),
            usage(1000, 600, 50),
            // Written twice: counts once.
            usage(1000, 600, 50),
            json!({"type": "response_item", "payload": {"type": "custom_tool_call", "name": "apply_patch", "call_id": "c1", "input": "*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch"}}),
            json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "call_id": "c2", "arguments": "{\"command\":[\"cargo\",\"test\"]}"}}),
            json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "c2", "output": "{\"output\":\"error[E0425]: cannot find value `x` in this scope\",\"metadata\":{\"exit_code\":101}}"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "second question"}]}}),
            usage(2500, 1600, 80),
        ];
        let text: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        fs::write(&path, text.join("\n")).unwrap();

        let entry = index_rollout(&path);
        assert_eq!(entry.session_id, id);
        assert!(!entry.hidden);
        assert_eq!(entry.last_at, Some(1794131400000));
        assert_eq!(entry.cwd.as_deref(), Some("/work/app"));
        assert_eq!(entry.started_at, Some(1794131282669));
        assert_eq!(entry.prompts, 2);
        assert_eq!(entry.first_prompt.as_deref(), Some("first question"));
        assert_eq!(entry.title.as_deref(), Some("first question"));
        assert_eq!(entry.tokens, Tokens { input: 900, output: 80, cache_write: 0, cache_read: 1600 });
        assert_eq!(entry.models.keys().collect::<Vec<_>>(), ["gpt-6"]);
        assert_eq!((entry.work.replies, entry.work.edits), (2, 1));
        assert_eq!(transcripts(&dir), std::slice::from_ref(&path));

        // Compressed once Codex is done with it: the same summary.
        let zst = path.with_extension("jsonl.zst");
        let packed = std::process::Command::new("zstd").arg("-q").arg(&path).arg("-o").arg(&zst).status();
        if packed.is_ok_and(|s| s.success()) {
            let again = index_rollout(&zst);
            assert_eq!((again.prompts, again.tokens), (entry.prompts, entry.tokens));
        }

        // A fork's first total includes its parent's: only its own reply counts.
        let fork = day.join("rollout-2026-11-08T15-20-02-01a1ba8e-1111-7c71-8d13-f9a0ff71750c.jsonl");
        let forked = json!({"type": "event_msg", "payload": {"type": "token_count", "info": {
            "total_token_usage": {"input_tokens": 9000, "cached_input_tokens": 8000, "output_tokens": 900},
            "last_token_usage": {"input_tokens": 1000, "cached_input_tokens": 600, "output_tokens": 50}}}});
        fs::write(&fork, forked.to_string()).unwrap();
        assert_eq!(index_rollout(&fork).tokens, Tokens { input: 400, output: 50, cache_write: 0, cache_read: 600 });
        fs::remove_file(&fork).unwrap();

        // A sub-agent's thread, and a side conversation left empty, are not conversations of their own.
        let other = day.join("rollout-2026-11-08T15-19-02-01a1ba8e-0000-7c71-8d13-f9a0ff71750c.jsonl");
        fs::write(&other, json!({"type": "session_meta", "payload": {"id": "x", "thread_source": "subagent"}}).to_string()).unwrap();
        assert!(index_rollout(&other).hidden);
        fs::write(&other, json!({"type": "session_meta", "payload": {"id": "x", "attached_thread": true}}).to_string()).unwrap();
        assert!(index_rollout(&other).hidden);
        fs::remove_file(&other).unwrap();

        fs::write(dir.join("session_index.jsonl"), format!("{{\"id\":\"{id}\",\"thread_name\":\"Old\"}}\n{{\"id\":\"{id}\",\"thread_name\":\"Fix the build\"}}\n")).unwrap();
        assert_eq!(thread_names(&dir).get(id).map(String::as_str), Some("Fix the build"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_status_from_the_tail() {
        let dir = temp("status");
        let path = dir.join("r.jsonl");
        fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n").unwrap();
        assert_eq!(status(&path), "working");
        fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"exec_approval_request\"}}\n").unwrap();
        assert_eq!(status(&path), "blocked");
        fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n").unwrap();
        assert_eq!(status(&path), "idle");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_processes() {
        assert!(is_session_process("/opt/homebrew/lib/node_modules/@openai/codex/vendor/aarch64-apple-darwin/codex/codex"));
        assert!(is_session_process("codex resume 019f"));
        assert!(is_session_process("/x/codex-aarch64-apple-darwin --model gpt-6"));
        assert!(!is_session_process("/Applications/Codex.app/Contents/Resources/codex app-server"));
        assert!(!is_session_process("node /usr/local/bin/codex"));
        assert!(!is_session_process("/usr/bin/codesign -v"));
        // What Codex 0.16x runs beside a session.
        let daemon = "/Users/me/.codex/packages/app-server-daemon/releases/0.169.0-aarch64-apple-darwin/bin";
        assert!(!is_session_process(&format!("{daemon}/codex app-server --listen unix:// --managed-daemon")));
        assert!(!is_session_process(&format!("{daemon}/codex-code-mode-host --listen grpc+stdio://")));
        assert!(!is_session_process("codex-exec-server"));
        assert!(!is_session_process("codex -c model=o3 agents"));
        // A prompt's words are not commands.
        assert!(is_session_process("codex please debug the login page"));
        assert!(is_session_process("codex -m gpt-6 fix the sandbox"));
        assert_eq!(
            invocation("codex -C /work/site --model gpt-6 resume 01a1e9c7-b4be-70c2-9c3b-f7893c63acc6 keep going"),
            Some(Invocation { resumes: Some("01a1e9c7-b4be-70c2-9c3b-f7893c63acc6 keep going".into()), cd: Some("/work/site".into()) })
        );
        assert_eq!(invocation("codex resume --last"), Some(Invocation::default()));
        assert_eq!(invocation("codex --cd=site fork 01a1"), Some(Invocation { resumes: None, cd: Some("site".into()) }));
    }

    #[test]
    fn turn_in_progress() {
        let dir = temp("turn");
        let path = dir.join("r.jsonl");
        let started = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t-2\"}}\n";
        let approval = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"exec_approval_request\"}}\n";
        fs::write(&path, format!("{started}{approval}")).unwrap();
        assert_eq!(running_turn(&path).as_deref(), Some("t-2"));
        fs::write(&path, format!("{started}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"turn_aborted\"}}}}\n")).unwrap();
        assert_eq!(running_turn(&path), None);
        // A command still running when the turn ended, as Codex records it; and one that finished.
        let output = |text: &str| format!("{{\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call_output\",\"call_id\":\"c\",\"output\":\"{text}\"}}}}\n");
        let done = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        fs::write(&path, format!("{started}{}{done}", output("Wall time: 10.0 seconds\\nProcess running with session ID 83947\\nOutput:\\n"))).unwrap();
        assert!(left_command(&path));
        fs::write(&path, format!("{}{started}{}{done}", output("Process running with session ID 1"), output("Process exited with code 0"))).unwrap();
        assert!(!left_command(&path), "only the last turn counts");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn websocket_messages() {
        let message = json!({"id": 3, "result": {"text": "x".repeat(300)}});
        let mut sent = Vec::new();
        send_message(&mut sent, &message).unwrap();
        assert_eq!(sent[0], 0x81);
        assert_eq!(sent[1], 0x80 | 126);
        // Unmask it into what a server would send: a ping, then the text in two pieces.
        let len = u16::from_be_bytes([sent[2], sent[3]]) as usize;
        let mask = [sent[4], sent[5], sent[6], sent[7]];
        let data: Vec<u8> = sent[8..].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
        assert_eq!(data.len(), len);
        let mut wire = vec![0x89, 0x00];
        wire.extend([0x01, 100]);
        wire.extend(&data[..100]);
        wire.extend([0x80, 126]);
        wire.extend(((len - 100) as u16).to_be_bytes());
        wire.extend(&data[100..]);
        assert_eq!(read_message(&mut Cursor::new(wire)).unwrap(), message);
        assert!(read_message(&mut Cursor::new(vec![0x88, 0x00])).is_err());
    }

    #[test]
    fn identity_claims() {
        // {"email":"me@example.com","https://api.openai.com/auth":{"chatgpt_plan_type":"plus"}}
        let token = "x.eyJlbWFpbCI6Im1lQGV4YW1wbGUuY29tIiwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfcGxhbl90eXBlIjoicGx1cyJ9fQ.sig";
        let claims = token_claims(token).unwrap();
        assert_eq!(claims["email"], "me@example.com");
        assert_eq!(claims.pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type").and_then(Value::as_str), Some("plus"));
    }
}
