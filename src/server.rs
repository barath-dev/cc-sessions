//! Local web server and JSON API.
//!
//! The page can end processes, so the server only listens on 127.0.0.1,
//! requires a token on every API call, and rejects requests that do not name
//! it as Host.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::accounts::{self, Account};
use crate::claude::{self, AuthInfo};
use crate::closing;
use crate::history;
use crate::os;
use crate::resume::{self, Target};
use crate::sessions::{self, Session};
use crate::store::{state_dir, state_file};
use crate::util::{err, log_stamp, now_ms, tilde, Error, Result};

pub const HOST: &str = "127.0.0.1";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const SESSIONS_MAX_AGE: Duration = Duration::from_secs(30); // re-read even if nothing looks changed
const AUTH_MAX_AGE: Duration = Duration::from_secs(120);
const AUTH_MAX_AGE_WATCH: Duration = Duration::from_secs(4); // while a sign-in may be in progress
const AUTH_WATCH: Duration = Duration::from_secs(600);
const MAX_BODY: usize = 1 << 20;
const WORKERS: usize = 4;

/// The page, built into the program.
const UI: &[(&str, &str, &[u8])] = &[
    ("/", "text/html; charset=utf-8", include_bytes!("../ui/index.html")),
    ("/app.js", "text/javascript; charset=utf-8", include_bytes!("../ui/app.js")),
    ("/theme.js", "text/javascript; charset=utf-8", include_bytes!("../ui/theme.js")),
    ("/style.css", "text/css; charset=utf-8", include_bytes!("../ui/style.css")),
    ("/favicon.svg", "image/svg+xml", include_bytes!("../ui/favicon.svg")),
];

pub fn port() -> u16 {
    std::env::var("CC_SESSIONS_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or_else(|| accounts::load_config().port)
}

pub fn pid_file() -> PathBuf {
    state_file("ui.pid")
}

pub fn log_file() -> PathBuf {
    state_file("ui.log")
}

/// The access token, created on first use and readable only by its owner.
pub fn load_token() -> Result<String> {
    let path = state_file("ui-token");
    if let Ok(token) = fs::read_to_string(&path) {
        if !token.trim().is_empty() {
            return Ok(token.trim().to_string());
        }
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Error(format!("no random source: {e}")))?;
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut token = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| (n << 8) | *b as u32) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            token.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
        }
    }
    fs::create_dir_all(state_dir())?;
    fs::write(&path, "")?;
    os::restrict_to_owner(&path)?;
    fs::write(&path, format!("{token}\n"))?;
    Ok(token)
}

fn same(a: &str, b: &str) -> bool {
    // Constant-time, so the token cannot be guessed a byte at a time.
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

type Listing = Arc<(Vec<Session>, BTreeMap<String, String>)>;

struct SessionCache {
    data: Option<Listing>,
    at: Instant,
    fingerprint: Vec<(String, u128, u64)>,
}

struct AuthCache {
    by_dir: HashMap<String, AuthInfo>,
    at: Option<Instant>,
    watch_until: Option<Instant>,
}

/// Shared state behind the request handlers.
pub struct App {
    token: String,
    port: u16,
    stopping: AtomicBool,
    actions: Mutex<()>, // held while a close is in progress
    sessions: Mutex<SessionCache>,
    auth: Mutex<AuthCache>,
    last_seen: AtomicI64,
}

impl App {
    fn new(token: String, port: u16) -> App {
        App {
            token,
            port,
            stopping: AtomicBool::new(false),
            actions: Mutex::new(()),
            sessions: Mutex::new(SessionCache { data: None, at: Instant::now(), fingerprint: Vec::new() }),
            auth: Mutex::new(AuthCache { by_dir: HashMap::new(), at: None, watch_until: None }),
            last_seen: AtomicI64::new(now_ms()),
        }
    }

    // -- cached reads

    fn invalidate(&self, auth: bool) {
        self.sessions.lock().unwrap().data = None;
        if auth {
            self.auth.lock().unwrap().at = None;
        }
    }

    /// Current sessions. `claude agents` only runs when something changed on disk.
    fn sessions(&self, force: bool) -> Result<Listing> {
        let mut cache = self.sessions.lock().unwrap();
        let accounts = accounts::load_accounts(false)?;
        if let Some(data) = cache.data.as_ref().filter(|_| !force && cache.at.elapsed() < SESSIONS_MAX_AGE) {
            if sessions::fingerprint(&accounts, &data.0) == cache.fingerprint {
                return Ok(data.clone());
            }
        }
        let data: Listing = Arc::new(sessions::all_sessions(&accounts)?);
        cache.fingerprint = sessions::fingerprint(&accounts, &data.0);
        cache.at = Instant::now();
        cache.data = Some(data.clone());
        Ok(data)
    }

    fn auth(&self, accounts: &[Account]) -> HashMap<String, AuthInfo> {
        let mut cache = self.auth.lock().unwrap();
        let watching = cache.watch_until.is_some_and(|until| Instant::now() < until);
        let max_age = if watching { AUTH_MAX_AGE_WATCH } else { AUTH_MAX_AGE };
        let stale = cache.at.is_none_or(|at| at.elapsed() > max_age);
        if stale || accounts.iter().any(|a| !cache.by_dir.contains_key(&a.dir)) {
            let infos: Vec<AuthInfo> = thread::scope(|scope| {
                let handles: Vec<_> = accounts.iter().map(|acc| scope.spawn(move || claude::auth_status(acc))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
            });
            cache.by_dir = accounts.iter().map(|a| a.dir.clone()).zip(infos).collect();
            cache.at = Some(Instant::now());
        }
        cache.by_dir.clone()
    }

    // -- views

    fn state(&self) -> Result<Value> {
        let everyone = accounts::load_accounts(true)?;
        let active: Vec<Account> = everyone.iter().filter(|a| !a.removed).cloned().collect();
        let listing = self.sessions(false)?;
        let (sessions, notices) = (&listing.0, &listing.1);
        let auth = self.auth(&active);
        let accounts: Vec<Value> = everyone
            .iter()
            .map(|acc| {
                let info = if acc.removed { AuthInfo::default() } else { auth.get(&acc.dir).cloned().unwrap_or_default() };
                json!({
                    "name": acc.name,
                    "alias": acc.alias,
                    "dir": tilde(&acc.dir),
                    "isDefault": acc.is_default(),
                    "removed": acc.removed,
                    "loggedIn": info.logged_in,
                    "email": info.email,
                    "org": info.org,
                    "plan": info.plan,
                    "authError": info.error,
                    "openSessions": sessions.iter().filter(|s| s.open && s.account == acc.name).count(),
                    "retentionDays": accounts::retention_days(acc),
                    "error": notices.get(&acc.name),
                })
            })
            .collect();
        Ok(json!({
            "accounts": accounts,
            "sessions": sessions.iter().map(public).collect::<Vec<_>>(),
            "unregistered": accounts::unregistered_folders()?.iter().map(|p| tilde(p)).collect::<Vec<_>>(),
            "platform": {
                "os": os::OS_NAME,
                "command": accounts::load_config().command,
                "aliasHint": os::alias_hint(),
                "gracefulClose": os::GRACEFUL_CLOSE,
            },
            "serverTime": now_ms(),
        }))
    }

    fn history(&self) -> Result<Value> {
        let listing = self.sessions(false)?;
        let accounts = accounts::load_accounts(false)?;
        let items: Vec<Value> = history::history_items()?
            .iter()
            .filter_map(|item| {
                let acc = accounts.iter().find(|a| a.name == item.account)?;
                let target = Target::from_history(item, &listing.0);
                let live = listing.0.iter().any(|s| s.live && s.account == item.account && s.session_id == item.session_id);
                Some(json!({
                    "account": item.account,
                    "id": item.id,
                    "sessionId": item.session_id,
                    "title": item.title,
                    "cwd": tilde(&item.folder_label),
                    "startedAt": item.started_at,
                    "lastActivity": item.last_activity,
                    "prompts": item.prompts,
                    "firstPrompt": item.first_prompt,
                    "lastPrompts": item.last_prompts,
                    "live": live,
                    "resumeCommand": (!item.cwd.is_empty()).then(|| resume::display_command(acc, &target)),
                }))
            })
            .collect();
        Ok(json!({ "items": items }))
    }

    // -- actions

    fn close(&self, keys: &[String]) -> Result<Value> {
        if self.stopping.load(Ordering::SeqCst) {
            return err("The server is shutting down.");
        }
        let _busy = self.actions.lock().unwrap();
        let listing = self.sessions(true)?;
        let results: Vec<Value> = keys
            .iter()
            .map(|key| match listing.0.iter().find(|s| &s.key == key) {
                None => json!({ "key": key, "ok": false, "message": "Session not found. It may have ended already." }),
                Some(sess) if !sess.open => json!({ "key": key, "ok": true, "message": "Already closed." }),
                Some(sess) => {
                    let closed = closing::close_session(sess);
                    json!({ "key": key, "ok": closed.ok, "message": closed.message })
                }
            })
            .collect();
        self.invalidate(false);
        Ok(json!({ "results": results }))
    }

    fn open(&self, target: &Target) -> Result<Value> {
        let acc = accounts::get_account(&target.account)?;
        let terminal = resume::open(&acc, target)?;
        self.invalidate(false);
        Ok(json!({ "terminal": terminal }))
    }

    fn resume_session(&self, key: &str) -> Result<Value> {
        let listing = self.sessions(true)?;
        match listing.0.iter().find(|s| s.key == key) {
            Some(sess) => self.open(&Target::from_session(sess)),
            None => err("Session not found."),
        }
    }

    fn resume_history(&self, account: &str, session_id: &str) -> Result<Value> {
        // Take the folder from our own index, not from the request.
        let items = history::history_items()?;
        let Some(item) = items.iter().find(|i| i.account == account && i.session_id == session_id) else {
            return err("Conversation not found.");
        };
        let listing = self.sessions(true)?;
        self.open(&Target::from_history(item, &listing.0))
    }

    fn login(&self, name: &str, relogin: bool) -> Result<Value> {
        let acc = accounts::get_account(name)?;
        let terminal = resume::open_login(&acc, relogin)?;
        // Sign-in happens in the terminal; look logins up often until it has had time to finish.
        self.auth.lock().unwrap().watch_until = Some(Instant::now() + AUTH_WATCH);
        Ok(json!({ "terminal": terminal }))
    }

    fn changed(&self, acc: Account) -> Value {
        self.invalidate(true);
        json!({ "name": acc.name, "alias": acc.alias, "dir": tilde(&acc.dir) })
    }

    fn route_post(&self, path: &str, body: &Value) -> Result<Value> {
        let text = |field: &str| body.get(field).and_then(Value::as_str).filter(|s| !s.is_empty());
        let need = |field: &str| text(field).ok_or_else(|| Error(format!("Missing '{field}'.")));
        match path {
            "/api/refresh" => {
                self.invalidate(true);
                Ok(json!({ "ok": true }))
            }
            "/api/sessions/close" => {
                let keys: Vec<String> = body
                    .get("keys")
                    .and_then(Value::as_array)
                    .map(|keys| keys.iter().filter_map(|k| k.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                if keys.is_empty() {
                    return err("Missing 'keys'.");
                }
                self.close(&keys)
            }
            "/api/sessions/resume" => self.resume_session(need("key")?),
            "/api/history/resume" => self.resume_history(need("account")?, need("sessionId")?),
            "/api/accounts/add" => {
                let (acc, created) = accounts::add_account(need("name")?, text("alias"), text("folder"))?;
                let mut out = self.changed(acc);
                out["created"] = json!(created);
                Ok(out)
            }
            "/api/accounts/rename" => {
                // An empty alias is meaningful here: it clears the default account's alias.
                let alias = body.get("alias").and_then(Value::as_str);
                Ok(self.changed(accounts::rename_account(need("name")?, text("newName"), alias)?))
            }
            "/api/accounts/remove" => Ok(self.changed(accounts::remove_account(need("name")?)?)),
            "/api/accounts/restore" => Ok(self.changed(accounts::restore_account(need("name")?, text("alias"))?)),
            "/api/accounts/login" => self.login(need("name")?, body.get("relogin").and_then(Value::as_bool).unwrap_or(false)),
            "/api/shutdown" => {
                self.stopping.store(true, Ordering::SeqCst);
                Ok(json!({ "ok": true }))
            }
            _ => Err(Error("Not found.".into())),
        }
    }
}

/// A session as the page sees it.
fn public(sess: &Session) -> Value {
    let mut value = serde_json::to_value(sess).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.remove("accountDir");
        map.insert("cwd".into(), json!(tilde(&sess.cwd)));
    }
    value
}

// ------------------------------------------------------------------- transport

fn header(request: &Request, name: &str) -> String {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn respond(request: Request, status: u16, content_type: &str, body: Vec<u8>) {
    let method = request.method().clone();
    if method != Method::Get || !(200..300).contains(&status) {
        // The page polls every few seconds; only log what changes something or fails.
        eprintln!("{} {} {} {}", log_stamp(), method, request.url(), status);
    }
    let mut response = Response::from_data(body).with_status_code(status);
    for (name, value) in [
        ("Content-Type", content_type),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Content-Security-Policy", "default-src 'self'; frame-ancestors 'none'"),
    ] {
        if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(h);
        }
    }
    let _ = request.respond(response);
}

fn respond_json(request: Request, status: u16, value: &Value) {
    respond(request, status, "application/json", value.to_string().into_bytes());
}

fn static_file(path: &str) -> Option<(&'static str, Vec<u8>)> {
    let (_, content_type, bytes) = UI.iter().find(|(route, ..)| *route == path)?;
    // For working on the page without rebuilding.
    if let Some(dir) = std::env::var_os("CC_SESSIONS_UI_DIR") {
        let name = if path == "/" { "index.html" } else { &path[1..] };
        if let Ok(bytes) = fs::read(PathBuf::from(dir).join(name)) {
            return Some((content_type, bytes));
        }
    }
    Some((content_type, bytes.to_vec()))
}

fn handle(app: &App, mut request: Request) {
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let host = header(&request, "Host");
    // Refuse anything that reached us under another name (DNS rebinding).
    let host_ok = host == format!("127.0.0.1:{}", app.port) || host == format!("localhost:{}", app.port);
    let is_get = *request.method() == Method::Get;

    if is_get && !path.starts_with("/api/") {
        return match static_file(&path) {
            _ if !host_ok => respond(request, 403, "text/plain", b"Forbidden host.".to_vec()),
            Some((content_type, bytes)) => respond(request, 200, content_type, bytes),
            None => respond(request, 404, "text/plain", b"Not found.".to_vec()),
        };
    }
    if !host_ok {
        return respond_json(request, 403, &json!({ "error": "Forbidden host." }));
    }
    if !same(&header(&request, "X-CC-Token"), &app.token) {
        let error = format!("Missing or wrong token. Run {} to open the page.", accounts::load_config().command);
        return respond_json(request, 401, &json!({ "error": error }));
    }
    // Any authorised request means a page (or the command line) is using the server.
    app.last_seen.store(now_ms(), Ordering::SeqCst);

    let result = if is_get {
        match path.as_str() {
            "/api/ping" => Ok(json!({ "ok": true, "app": "cc-sessions", "version": VERSION, "pid": std::process::id() })),
            "/api/state" => app.state(),
            "/api/history" => app.history(),
            _ => return respond_json(request, 404, &json!({ "error": "Not found." })),
        }
    } else if *request.method() == Method::Post {
        let origin = header(&request, "Origin");
        if !origin.is_empty() && origin != format!("http://127.0.0.1:{}", app.port) && origin != format!("http://localhost:{}", app.port) {
            return respond_json(request, 403, &json!({ "error": "Forbidden origin." }));
        }
        if header(&request, "Content-Type").split(';').next().unwrap_or("").trim() != "application/json" {
            return respond_json(request, 415, &json!({ "error": "Send JSON." }));
        }
        if request.body_length().unwrap_or(0) > MAX_BODY {
            return respond_json(request, 413, &json!({ "error": "Request too large." }));
        }
        let mut text = String::new();
        let read = request.as_reader().take(MAX_BODY as u64 + 1).read_to_string(&mut text);
        let body = if text.trim().is_empty() { Ok(json!({})) } else { serde_json::from_str::<Value>(&text) };
        match (read, body) {
            (Ok(_), Ok(body)) if body.is_object() => match app.route_post(&path, &body) {
                Err(e) if e.0 == "Not found." => return respond_json(request, 404, &json!({ "error": e.0 })),
                other => other,
            },
            _ => return respond_json(request, 400, &json!({ "error": "Body must be a JSON object." })),
        }
    } else {
        return respond_json(request, 405, &json!({ "error": "Method not allowed." }));
    };
    match result {
        Ok(value) => respond_json(request, 200, &value),
        Err(e) => respond_json(request, 400, &json!({ "error": e.0 })),
    }
}

fn idle_limit() -> Option<Duration> {
    if let Some(seconds) = std::env::var("CC_SESSIONS_IDLE_SECONDS").ok().and_then(|s| s.parse::<u64>().ok()) {
        return (seconds > 0).then(|| Duration::from_secs(seconds));
    }
    let minutes = accounts::load_config().idle_minutes;
    (minutes > 0).then(|| Duration::from_secs(minutes as u64 * 60))
}

/// Run the server in the foreground until Disconnect, a stop signal, or the
/// idle limit with no page open.
pub fn serve() -> Result<()> {
    let port = port();
    let app = Arc::new(App::new(load_token()?, port));
    let server = Server::http((HOST, port)).map_err(|e| Error(format!("could not listen on {HOST}:{port}: {e}")))?;
    let server = Arc::new(server);

    fs::create_dir_all(state_dir())?;
    fs::write(pid_file(), format!("{}\n", std::process::id()))?;
    eprintln!("{} cc-sessions {VERSION} listening on http://{HOST}:{port} (pid {})", log_stamp(), std::process::id());

    let done = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..WORKERS)
        .map(|_| {
            let (app, server, done) = (app.clone(), server.clone(), done.clone());
            thread::spawn(move || {
                while !done.load(Ordering::SeqCst) {
                    if let Ok(Some(request)) = server.recv_timeout(Duration::from_millis(250)) {
                        handle(&app, request);
                    }
                }
            })
        })
        .collect();

    let signalled = os::stop_flag();
    let idle = idle_limit();
    let reason = loop {
        thread::sleep(Duration::from_millis(250));
        if app.stopping.load(Ordering::SeqCst) {
            break "disconnected";
        }
        if signalled.load(Ordering::SeqCst) {
            break "stop signal";
        }
        let quiet_for = Duration::from_millis((now_ms() - app.last_seen.load(Ordering::SeqCst)).max(0) as u64);
        // Never stop on idle while a close is in progress.
        if idle.is_some_and(|limit| quiet_for > limit) && app.actions.try_lock().is_ok() {
            break "no page open";
        }
    };
    app.stopping.store(true, Ordering::SeqCst);
    // Let a close that is in progress finish before the workers go away.
    drop(app.actions.lock());
    done.store(true, Ordering::SeqCst);
    for worker in workers {
        let _ = worker.join();
    }
    // The pid file is the tool's own marker that a server is running.
    if fs::read_to_string(pid_file()).is_ok_and(|pid| pid.trim() == std::process::id().to_string()) {
        let _ = fs::remove_file(pid_file());
    }
    eprintln!("{} cc-sessions stopped ({reason})", log_stamp());
    Ok(())
}
