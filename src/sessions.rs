//! Live and background sessions of every account, with the closed ledger merged in.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::accounts::{self, Account};
use crate::claude;
use crate::history::Artifact;
use crate::os;
use crate::resume;
use crate::store::{self, state_file};
use crate::pins::{self, Pins};
use crate::titles::{self, Titles};
use crate::util::{iso_to_ms, now_ms, path_str, Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct TimelineEntry {
    pub at: Option<i64>,
    pub state: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub key: String,
    pub account: String,
    pub account_dir: String,
    pub id: String,
    pub session_id: String,
    /// "interactive" or "background"
    pub kind: String,
    pub cwd: String,
    pub name: String,
    /// True when `name` was given through this tool.
    pub renamed: bool,
    /// Pinned to the top of the page through this tool.
    pub pinned: bool,
    /// The name Claude Code has for it, when `name` is not that.
    #[serde(skip)]
    pub claude_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
    pub started_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub pid: Option<u32>,
    pub live: bool,
    /// working | blocked | idle | done | closed, or whatever Claude Code reports
    pub status: String,
    pub open: bool,
    pub detail: Option<String>,
    pub timeline: Vec<TimelineEntry>,
    pub closed_at: Option<i64>,
    pub resume_command: String,
}

impl Session {
    pub fn is_background(&self) -> bool {
        self.kind == "background"
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn interactive_status(raw: &str) -> String {
    match raw {
        "busy" => "working",
        "idle" => "idle",
        "waiting" | "blocked" => "blocked",
        other => other,
    }
    .to_string()
}

fn background_status(raw: &str) -> String {
    match raw {
        "stopped" => "closed",
        other => other,
    }
    .to_string()
}

fn read_timeline(path: &Path, limit: usize) -> Vec<TimelineEntry> {
    let Ok(content) = fs::read_to_string(path) else { return Vec::new() };
    let lines: Vec<&str> = content.lines().collect();
    lines[lines.len().saturating_sub(limit)..]
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|rec| TimelineEntry {
            at: rec.get("at").and_then(Value::as_str).and_then(iso_to_ms),
            state: text(&rec, "state"),
            detail: text(&rec, "detail"),
        })
        .collect()
}

/// One entry of `claude agents --json` (or of a `sessions/<pid>.json` record,
/// which has the same fields for interactive sessions) as a Session.
fn normalise(acc: &Account, raw: &Value) -> Session {
    let interactive = raw.get("kind").and_then(Value::as_str) == Some("interactive");
    let session_id = text(raw, "sessionId").unwrap_or_default();
    let pid = raw.get("pid").and_then(Value::as_u64).map(|p| p as u32);
    let started_at = raw.get("startedAt").and_then(Value::as_i64);
    let config = Path::new(&acc.dir);
    let mut sess = Session {
        key: String::new(),
        account: acc.name.clone(),
        account_dir: acc.dir.clone(),
        id: text(raw, "id").unwrap_or_else(|| session_id.chars().take(8).collect()),
        session_id,
        kind: if interactive { "interactive" } else { "background" }.to_string(),
        cwd: text(raw, "cwd").unwrap_or_default(),
        name: text(raw, "name").unwrap_or_else(|| "(unnamed)".into()),
        renamed: false,
        pinned: false,
        claude_name: None,
        artifacts: Vec::new(),
        started_at,
        updated_at: started_at,
        pid,
        live: pid.is_some_and(os::pid_alive),
        status: String::new(),
        open: true,
        detail: None,
        timeline: Vec::new(),
        closed_at: None,
        resume_command: String::new(),
    };
    if interactive {
        sess.status = interactive_status(raw.get("status").and_then(Value::as_str).unwrap_or("idle"));
        if let Some(pid) = pid {
            let rec = store::read_json_or_null(&config.join("sessions").join(format!("{pid}.json")));
            let updated = rec.get("statusUpdatedAt").or_else(|| rec.get("updatedAt")).and_then(Value::as_i64);
            sess.updated_at = updated.or(sess.updated_at);
        }
    } else {
        sess.status = background_status(raw.get("state").and_then(Value::as_str).unwrap_or("done"));
        let job = config.join("jobs").join(&sess.id);
        let state = store::read_json_or_null(&job.join("state.json"));
        sess.detail = text(&state, "detail");
        sess.updated_at = state.get("updatedAt").and_then(Value::as_str).and_then(iso_to_ms).or(sess.updated_at);
        sess.timeline = read_timeline(&job.join("timeline.jsonl"), 5);
    }
    sess
}

/// Interactive sessions read straight from `<account>/sessions/*.json`, for a
/// system where `claude agents` is not available.
fn sessions_from_records(acc: &Account) -> Vec<Session> {
    let Ok(entries) = fs::read_dir(Path::new(&acc.dir).join("sessions")) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| store::read_json_or_null(&p))
        .filter(|rec| rec.get("pid").and_then(Value::as_u64).is_some_and(|pid| os::pid_alive(pid as u32)))
        .map(|mut rec| {
            if let Some(map) = rec.as_object_mut() {
                map.entry("kind").or_insert(json!("interactive"));
            }
            normalise(acc, &rec)
        })
        .collect()
}

/// Sessions of one account as `claude agents` reports them. The second value
/// is a notice when only part of the picture could be read.
pub fn fetch_account_sessions(acc: &Account) -> Result<(Vec<Session>, Option<String>)> {
    let out = claude::run(acc, &["agents", "--json", "--all"], 30)?;
    match serde_json::from_str::<Value>(&out.stdout) {
        Ok(Value::Array(items)) => Ok((items.iter().filter(|i| i.is_object()).map(|i| normalise(acc, i)).collect(), None)),
        _ => {
            let notice = format!(
                "claude agents is not available for '{}' ({}); showing interactive sessions only.",
                acc.name,
                out.last_line()
            );
            Ok((sessions_from_records(acc), Some(notice)))
        }
    }
}

// ----------------------------------------------------------------------- ledger

fn ledger_key(sess: &Session) -> String {
    // A background session keeps its short id even if its conversation id changes.
    let ident = if sess.is_background() { &sess.id } else { &sess.session_id };
    format!("{}|{}|{}", sess.account_dir, sess.kind, ident)
}

fn ledger_file() -> PathBuf {
    state_file("closed.json")
}

fn load_ledger() -> Result<Map<String, Value>> {
    Ok(store::read_json(&ledger_file())?
        .and_then(|data| data.get("closed").and_then(Value::as_object).cloned())
        .unwrap_or_default())
}

/// Remember that a session was closed through this tool. Claude Code has no
/// "closed" state of its own, so this is what keeps it listed as closed.
pub fn record_closed(sess: &Session) -> Result<()> {
    store::locked(|| {
        let mut closed = load_ledger()?;
        closed.insert(
            ledger_key(sess),
            json!({
                "dir": sess.account_dir,
                "kind": sess.kind,
                "id": sess.id,
                "sessionId": sess.session_id,
                "cwd": sess.cwd,
                // A name given through this tool is kept in its own record.
                "name": sess.claude_name.as_ref().unwrap_or(&sess.name),
                "startedAt": sess.started_at,
                "closedAt": now_ms(),
            }),
        );
        store::write_json(&ledger_file(), &json!({ "version": 1, "closed": closed }), true)
    })
}

fn session_from_ledger(acc: &Account, entry: &Value) -> Session {
    let session_id = text(entry, "sessionId").unwrap_or_default();
    let closed_at = entry.get("closedAt").and_then(Value::as_i64);
    Session {
        key: String::new(),
        account: acc.name.clone(),
        account_dir: acc.dir.clone(),
        id: text(entry, "id").unwrap_or_else(|| session_id.chars().take(8).collect()),
        session_id,
        kind: "interactive".into(),
        cwd: text(entry, "cwd").unwrap_or_default(),
        name: text(entry, "name").unwrap_or_else(|| "(unnamed)".into()),
        renamed: false,
        pinned: false,
        claude_name: None,
        artifacts: Vec::new(),
        started_at: entry.get("startedAt").and_then(Value::as_i64),
        updated_at: closed_at,
        pid: None,
        live: false,
        status: "closed".into(),
        open: false,
        detail: None,
        timeline: Vec::new(),
        closed_at,
        resume_command: String::new(),
    }
}

// ---------------------------------------------------------------------- listing

/// Account name to why it could not be read, in full or in part.
pub type Notices = BTreeMap<String, String>;

/// Sessions of every given account, with a notice for each account that could
/// not be read completely.
pub fn all_sessions(accounts: &[Account]) -> Result<(Vec<Session>, Notices)> {
    let fetched: Vec<Result<(Vec<Session>, Option<String>)>> = thread::scope(|scope| {
        let handles: Vec<_> = accounts.iter().map(|acc| scope.spawn(move || fetch_account_sessions(acc))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(Error("reader crashed".into())))).collect()
    });

    let ledger = load_ledger()?;
    let mut sessions = Vec::new();
    let mut notices = BTreeMap::new();
    for (acc, result) in accounts.iter().zip(fetched) {
        let (found, notice) = match result {
            Ok(ok) => ok,
            Err(e) => {
                notices.insert(acc.name.clone(), e.0);
                continue;
            }
        };
        if let Some(notice) = notice {
            notices.insert(acc.name.clone(), notice);
        }
        let seen: Vec<String> = found.iter().map(|s| s.session_id.clone()).collect();
        for mut sess in found {
            if let Some(entry) = ledger.get(&ledger_key(&sess)).filter(|_| !sess.live) {
                sess.status = "closed".into();
                sess.closed_at = entry.get("closedAt").and_then(Value::as_i64);
            }
            sessions.push(sess);
        }
        // Interactive sessions vanish from `claude agents` once their process
        // ends; the ledger is what keeps them listed as closed.
        for entry in ledger.values() {
            let mine = entry.get("dir").and_then(Value::as_str) == Some(acc.dir.as_str())
                && entry.get("kind").and_then(Value::as_str) == Some("interactive");
            let session_id = entry.get("sessionId").and_then(Value::as_str).unwrap_or("");
            if mine && !seen.iter().any(|s| s == session_id) {
                sessions.push(session_from_ledger(acc, entry));
            }
        }
    }

    let titles = Titles::load()?;
    let pins = Pins::load()?;
    for sess in &mut sessions {
        sess.key = format!("{}:{}", sess.account, sess.id);
        sess.open = sess.status != "closed";
        if let Some(given) = titles.get(&sess.account_dir, &sess.session_id) {
            sess.claude_name = Some(std::mem::replace(&mut sess.name, given.to_string()));
            sess.renamed = true;
        }
        sess.pinned = pins.has_session(&sess.account_dir, &sess.session_id);
        if let Some(acc) = accounts.iter().find(|a| a.name == sess.account) {
            sess.resume_command = resume::display_command(acc, &resume::Target::from_session(sess));
        }
    }
    sessions.sort_by(|a, b| {
        (a.cwd.to_lowercase(), !a.open, -a.updated_at.unwrap_or(0)).cmp(&(b.cwd.to_lowercase(), !b.open, -b.updated_at.unwrap_or(0)))
    });
    Ok((sessions, notices))
}

/// Cheap signature of everything a session listing depends on, so a caller
/// that polls often can skip `claude agents` until something actually changed.
pub fn fingerprint(accounts: &[Account], sessions: &[Session]) -> Vec<(String, u128, u64)> {
    let mut paths = vec![state_file("accounts.json"), ledger_file(), titles::titles_file(), pins::pins_file()];
    for acc in accounts {
        let base = Path::new(&acc.dir);
        if let Ok(entries) = fs::read_dir(base.join("sessions")) {
            paths.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")));
        }
        if let Ok(entries) = fs::read_dir(base.join("jobs")) {
            paths.extend(entries.flatten().map(|e| e.path().join("state.json")));
        }
        paths.push(base.join("daemon").join("roster.json"));
    }
    paths.sort();
    let mut parts: Vec<(String, u128, u64)> = paths
        .iter()
        .filter_map(|path| {
            let meta = fs::metadata(path).ok()?;
            let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            Some((path_str(path), modified, meta.len()))
        })
        .collect();
    // A process that died without touching its record still changes the picture.
    for sess in sessions.iter().filter(|s| s.live) {
        let alive = sess.pid.is_some_and(os::pid_alive);
        parts.push((format!("pid:{}", sess.pid.unwrap_or(0)), alive as u128, 0));
    }
    parts
}

/// Match `id`, a session-id prefix, or `account:id`.
pub fn find_sessions<'a>(sessions: &'a [Session], token: &str) -> Vec<&'a Session> {
    let (account, ident) = match token.rsplit_once(':') {
        Some((account, ident)) => (Some(account), ident),
        None => (None, token),
    };
    let ident = ident.to_lowercase();
    if ident.is_empty() {
        return Vec::new();
    }
    sessions
        .iter()
        .filter(|s| account.is_none_or(|a| a == s.account))
        .filter(|s| s.id.to_lowercase() == ident || s.session_id.to_lowercase().starts_with(&ident))
        .collect()
}

/// True if `path` is `folder` or anywhere under it.
pub fn in_folder(path: &str, folder: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let folder = crate::util::expand_tilde(folder);
    let folder = fs::canonicalize(&folder).unwrap_or(folder);
    let path = PathBuf::from(path);
    let path = fs::canonicalize(&path).unwrap_or(path);
    path.starts_with(&folder)
}

/// Sessions of every active account.
pub fn all_active() -> Result<(Vec<Account>, Vec<Session>, Notices)> {
    let accounts = accounts::load_accounts(false)?;
    let (sessions, notices) = all_sessions(&accounts)?;
    Ok((accounts, sessions, notices))
}
