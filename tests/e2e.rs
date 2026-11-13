//! End-to-end tests: the real program against a fake `claude`, in a temporary
//! home folder. They run the same way on Mac, Linux and Windows.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const EXE: &str = env!("CARGO_BIN_EXE_cc-sessions");
const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

/// A process that plays a running Claude session; ended when dropped.
///
/// A background thread waits on it, the way a shell does for a real session,
/// so that once it exits it is really gone rather than left as a zombie.
struct StandIn {
    pid: u32,
    exited: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    reaper: Option<thread::JoinHandle<()>>,
}

impl StandIn {
    fn start() -> StandIn {
        let mut child: Child = Command::new(FAKE_CLAUDE).arg("sleep").stdin(Stdio::null()).stdout(Stdio::null()).spawn().unwrap();
        let pid = child.id();
        let (exited, stop) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (exited_flag, stop_flag) = (exited.clone(), stop.clone());
        let reaper = thread::spawn(move || loop {
            if child.try_wait().unwrap().is_some() {
                break exited_flag.store(true, Ordering::SeqCst);
            }
            if stop_flag.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            thread::sleep(Duration::from_millis(20));
        });
        StandIn { pid, exited, stop, reaper: Some(reaper) }
    }

    fn pid(&self) -> u32 {
        self.pid
    }

    fn exited_within(&mut self, seconds: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline {
            if self.exited.load(Ordering::SeqCst) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Start time in the form Claude Code records as `procStart`.
    fn proc_start(&self) -> String {
        if cfg!(windows) {
            return String::new();
        }
        let out = Command::new("ps")
            .args(["-o", "lstart=", "-p", &self.pid().to_string()])
            .env("LC_ALL", "C")
            .env("TZ", "UTC")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(reaper) = self.reaper.take() {
            let _ = reaper.join();
        }
    }
}

/// A temporary machine: home folder, state folder, and a PATH with the fake claude.
struct Machine {
    root: PathBuf,
    home: PathBuf,
    state: PathBuf,
    port: u16,
    before: BTreeMap<String, u64>,
}

impl Machine {
    fn new(name: &str, port: u16) -> Machine {
        let root = std::env::temp_dir().join(format!("cc-sessions-e2e-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(home.join(".claude").join("projects")).unwrap();
        fs::create_dir_all(home.join(".claude-work").join("projects")).unwrap();
        fs::create_dir_all(home.join(".claude-work").join("sessions")).unwrap();
        fs::copy(FAKE_CLAUDE, bin.join(if cfg!(windows) { "claude.exe" } else { "claude" })).unwrap();
        Machine { state: root.join("state"), root, home, port, before: BTreeMap::new() }
    }

    fn account(&self, name: &str) -> PathBuf {
        self.home.join(if name == "default" { ".claude".to_string() } else { format!(".claude-{name}") })
    }

    fn agents(&self, account: &str, sessions: Value) {
        fs::write(self.account(account).join("fake-agents.json"), sessions.to_string()).unwrap();
    }

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("claude-calls.log")).unwrap_or_default()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(EXE);
        cmd.args(args);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().to_uppercase().starts_with("CLAUDE") {
                cmd.env_remove(key);
            }
        }
        let mut path = vec![self.root.join("bin")];
        path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        cmd.env("PATH", std::env::join_paths(path).unwrap())
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("APPDATA", self.root.join("appdata"))
            .env("LOCALAPPDATA", self.root.join("localappdata"))
            .env("SHELL", "/bin/zsh")
            .env("CC_SESSIONS_HOME", &self.state)
            .env("CC_SESSIONS_PORT", self.port.to_string())
            .env("CC_SESSIONS_SKIP_PATH", "1")
            .env("FAKE_CLAUDE_LOG", self.root.join("claude-calls.log"))
            .current_dir(&self.home);
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    /// Run with `answers` typed on stdin.
    fn run_answering(&self, args: &[&str], answers: &str) -> Output {
        let mut child = self.command(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(answers.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?} failed: {}", text(&out));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?} did not print JSON ({e}): {}", text(&out)))
    }

    /// Every file under the account folders, with its size.
    fn account_files(&self) -> BTreeMap<String, u64> {
        fn walk(dir: &Path, into: &mut BTreeMap<String, u64>) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, into);
                } else {
                    into.insert(path.to_string_lossy().into_owned(), fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
                }
            }
        }
        let mut files = BTreeMap::new();
        for entry in fs::read_dir(&self.home).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(".claude") {
                walk(&entry.path(), &mut files);
            }
        }
        files
    }

    /// Call once the fixtures are in place; `Drop` then checks nothing under
    /// an account folder was removed or changed.
    fn seal(&mut self) {
        self.before = self.account_files();
    }

    fn http(&self, request: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        let _ = stream.read_to_string(&mut raw);
        let status = raw.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        (status, raw.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default())
    }

    fn get(&self, path: &str, token: Option<&str>, host: Option<&str>) -> (u16, String) {
        let host = host.map(String::from).unwrap_or_else(|| format!("127.0.0.1:{}", self.port));
        let token = token.map(|t| format!("X-CC-Token: {t}\r\n")).unwrap_or_default();
        self.http(&format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{token}Connection: close\r\n\r\n"))
    }

    fn post(&self, path: &str, token: &str, extra_headers: &str, body: &str) -> (u16, String) {
        self.http(&format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-CC-Token: {token}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        ))
    }

    fn token(&self) -> String {
        fs::read_to_string(self.state.join("ui-token")).unwrap().trim().to_string()
    }

    fn server_running(&self) -> bool {
        TcpStream::connect(("127.0.0.1", self.port)).is_ok()
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        if self.server_running() {
            let _ = self.run(&["stop"]);
        }
        if !thread::panicking() && !self.before.is_empty() {
            let after = self.account_files();
            for (path, size) in &self.before {
                assert_eq!(after.get(path), Some(size), "{path} was removed or changed under an account folder");
            }
            assert_eq!(after.len(), self.before.len(), "files were added under an account folder: {after:?}");
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn find<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array().unwrap().iter().find(|s| s["id"] == id).unwrap_or_else(|| panic!("no session {id} in {list}"))
}

fn background(id: &str, state: &str, pid: Option<u32>) -> Value {
    let mut value = json!({
        "id": id, "kind": "background", "state": state, "name": format!("job {id}"),
        "cwd": "/work/app", "sessionId": format!("{id}-0000-0000-0000-000000000000"), "startedAt": now_ms(),
    });
    if let Some(pid) = pid {
        value["pid"] = json!(pid);
    }
    value
}

/// An interactive session backed by a stand-in process, with its record on disk.
fn interactive(machine: &Machine, account: &str, session_id: &str, stand_in: &StandIn, proc_start: Option<&str>, started_at: i64) -> Value {
    let record = json!({
        "pid": stand_in.pid(), "sessionId": session_id, "cwd": "/work/app", "startedAt": started_at,
        "procStart": proc_start.map(String::from).unwrap_or_else(|| stand_in.proc_start()),
        "kind": "interactive", "name": "terminal one", "status": "idle", "updatedAt": started_at,
    });
    fs::write(machine.account(account).join("sessions").join(format!("{}.json", stand_in.pid())), record.to_string()).unwrap();
    json!({
        "pid": stand_in.pid(), "kind": "interactive", "status": "idle", "name": "terminal one",
        "cwd": "/work/app", "sessionId": session_id, "startedAt": started_at,
    })
}

#[test]
fn lists_sessions_across_accounts() {
    let mut m = Machine::new("list", 47801);
    let stand_in = StandIn::start();
    m.agents("default", json!([background("aaaa1111", "done", None)]));
    let live = interactive(&m, "work", "bbbb2222-1111-1111-1111-111111111111", &stand_in, None, now_ms());
    m.agents("work", json!([live, background("cccc3333", "blocked", None), background("dddd4444", "stopped", None)]));
    m.seal();

    let all = m.json(&["list", "--all", "--json"]);
    assert_eq!(all.as_array().unwrap().len(), 4);
    assert_eq!(find(&all, "aaaa1111")["account"], "default");
    assert_eq!(find(&all, "aaaa1111")["status"], "done");
    assert_eq!(find(&all, "bbbb2222")["status"], "idle");
    assert_eq!(find(&all, "bbbb2222")["live"], true);
    assert_eq!(find(&all, "bbbb2222")["key"], "work:bbbb2222");
    assert_eq!(find(&all, "cccc3333")["status"], "blocked");
    assert_eq!(find(&all, "dddd4444")["status"], "closed");
    assert_eq!(find(&all, "dddd4444")["open"], false);

    // Only open sessions by default; filters work.
    assert_eq!(m.json(&["list", "--json"]).as_array().unwrap().len(), 3);
    assert_eq!(m.json(&["list", "--json", "--account", "default"]).as_array().unwrap().len(), 1);
    assert_eq!(m.json(&["list", "--closed", "--json"]).as_array().unwrap().len(), 1);

    // The default account must run with the variable unset; the others with their folder.
    let log = m.log();
    assert!(log.contains("agents --json --all\tCLAUDE_CONFIG_DIR=<unset>"), "{log}");
    assert!(log.contains(&format!("agents --json --all\tCLAUDE_CONFIG_DIR={}", m.account("work").display())), "{log}");
}

#[test]
fn closes_a_background_session() {
    let mut m = Machine::new("close-bg", 47802);
    let mut stand_in = StandIn::start();
    m.agents("work", json!([background("eeee5555", "working", Some(stand_in.pid()))]));
    m.seal();

    let out = m.run(&["close", "work:eeee5555", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(stand_in.exited_within(5), "claude stop should have ended it: {}", text(&out));
    assert!(m.log().contains("stop eeee5555"));

    // Claude Code would now report it without a pid; the tool's ledger marks it closed.
    m.agents("work", json!([background("eeee5555", "done", None)]));
    m.seal();
    let closed = m.json(&["list", "--closed", "--json"]);
    assert_eq!(find(&closed, "eeee5555")["status"], "closed");
    assert!(m.state.join("closed.json").exists());
}

#[test]
fn closes_an_interactive_session_only_after_yes() {
    let mut m = Machine::new("close-int", 47803);
    let mut stand_in = StandIn::start();
    let session = interactive(&m, "work", "ffff6666-1111-1111-1111-111111111111", &stand_in, None, now_ms());
    m.agents("work", json!([session]));
    m.seal();

    let declined = m.run_answering(&["close", "ffff6666", "--yes"], "n\n");
    assert!(text(&declined).contains("Close terminal one"), "interactive sessions are always asked about: {}", text(&declined));
    assert!(!stand_in.exited_within(1), "answering no must leave it running");

    let accepted = m.run_answering(&["close", "ffff6666"], "y\n");
    assert!(accepted.status.success(), "{}", text(&accepted));
    assert!(stand_in.exited_within(5), "{}", text(&accepted));

    // Its record is gone from `claude agents`; the ledger keeps it listed as closed.
    m.agents("work", json!([]));
    m.seal();
    let closed = m.json(&["list", "--closed", "--json"]);
    assert_eq!(find(&closed, "ffff6666")["kind"], "interactive");
    assert_eq!(find(&closed, "ffff6666")["status"], "closed");
}

#[test]
fn refuses_a_reused_pid() {
    let mut m = Machine::new("reused", 47804);
    let mut stand_in = StandIn::start();
    // The record describes a process that started an hour before this one existed.
    let session = interactive(&m, "work", "abab7777-1111-1111-1111-111111111111", &stand_in, Some("Thu Jan 1 00:00:00 1970"), now_ms() - 3_600_000);
    m.agents("work", json!([session]));
    m.seal();

    let out = m.run_answering(&["close", "abab7777"], "y\n");
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("reused"), "{}", text(&out));
    assert!(!stand_in.exited_within(1), "a process that is not the session must be left alone");
    assert!(!m.state.join("closed.json").exists(), "nothing may be marked closed");
}

#[test]
fn listing_never_ends_a_process() {
    let mut m = Machine::new("alive", 47805);
    let mut stand_in = StandIn::start();
    let session = interactive(&m, "work", "cdcd8888-1111-1111-1111-111111111111", &stand_in, None, now_ms());
    m.agents("work", json!([session, background("efef9999", "working", Some(stand_in.pid()))]));
    m.seal();
    for _ in 0..3 {
        assert_eq!(find(&m.json(&["list", "--json"]), "cdcd8888")["live"], true);
    }
    assert!(!stand_in.exited_within(1), "checking whether a process is alive must not end it");
}

#[test]
fn account_lifecycle_keeps_folders() {
    let mut m = Machine::new("accounts", 47806);
    m.seal();

    let listed = text(&m.run(&["account", "list"]));
    assert!(listed.contains("default") && listed.contains("work"), "{listed}");

    let added = m.run(&["account", "add", "scratch", "--no-login"]);
    assert!(added.status.success(), "{}", text(&added));
    assert!(m.account("scratch").is_dir());
    m.seal();

    assert!(!m.run(&["account", "add", "work", "--no-login"]).status.success(), "duplicate names are refused");
    assert!(!m.run(&["account", "add", "Bad Name", "--no-login"]).status.success());
    assert!(!m.run(&["account", "add", "x", "--alias", "claude", "--no-login"]).status.success());

    let renamed = m.run(&["account", "rename", "scratch", "demo"]);
    assert!(renamed.status.success(), "{}", text(&renamed));
    assert!(m.account("scratch").is_dir() && !m.account("demo").exists(), "renaming must not move the folder");
    let registry: Value = serde_json::from_str(&fs::read_to_string(m.state.join("accounts.json")).unwrap()).unwrap();
    let demo = registry["accounts"].as_array().unwrap().iter().find(|a| a["name"] == "demo").unwrap();
    assert_eq!(demo["alias"], "claude-demo");
    if cfg!(windows) {
        let bin = m.root.join("localappdata").join("cc-sessions").join("bin");
        assert!(bin.join("claude-demo.cmd").exists() && !bin.join("claude-scratch.cmd").exists());
    } else {
        let aliases = fs::read_to_string(m.state.join("aliases.sh")).unwrap();
        assert!(aliases.contains("alias claude-demo=") && !aliases.contains("alias claude-scratch="), "{aliases}");
        assert!(aliases.contains("alias claude-work="));
    }

    assert!(m.run(&["account", "remove", "demo", "--yes"]).status.success());
    assert!(m.account("scratch").is_dir(), "removing an account must leave its folder");
    assert!(!text(&m.run(&["account", "list"])).contains("demo"));
    assert!(text(&m.run(&["account", "list", "--removed"])).contains("demo"));
    assert!(m.state.join("accounts.json.bak").exists(), "the previous registry is kept");

    assert!(m.run(&["account", "restore", "demo"]).status.success());
    assert!(text(&m.run(&["account", "list"])).contains("demo"));
}

#[test]
fn history_lists_saved_conversations() {
    let mut m = Machine::new("history", 47807);
    let project = m.account("work").join("projects").join("-work-app");
    fs::create_dir_all(project.join("subagents")).unwrap();
    let transcript = [
        r#"{"type":"user","cwd":"/work/app","timestamp":"2026-11-10T06:20:00.000Z","message":{"role":"user","content":"fix the login bug"}}"#,
        r#"{"type":"ai-title","aiTitle":"Login bug fix","sessionId":"x"}"#,
        r#"{"type":"user","message":{"role":"user","content":"now add a test"}}"#,
        r#"{"type":"frame-link","sessionId":"x","path":"/tmp/report.html","frameUrl":"https://claude.ai/artifact/AbCd1234","title":"Login report","artifactCount":1,"timestamp":"2026-11-10T06:25:00.000Z"}"#,
    ];
    fs::write(project.join("11112222-3333-4444-5555-666677778888.jsonl"), transcript.join("\n")).unwrap();
    fs::write(project.join("subagents").join("agent-1.jsonl"), transcript.join("\n")).unwrap();
    m.seal();

    let items = m.json(&["history", "--json"]);
    let items = items.as_array().unwrap();
    assert_eq!(items.len(), 1, "sub-agent transcripts are not sessions");
    assert_eq!(items[0]["title"], "Login bug fix");
    assert_eq!(items[0]["account"], "work");
    assert_eq!(items[0]["cwd"], "/work/app");
    assert_eq!(items[0]["prompts"], 2);
    assert_eq!(items[0]["firstPrompt"], "fix the login bug");
    assert_eq!(items[0]["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(items[0]["artifacts"][0]["url"], "https://claude.ai/artifact/AbCd1234");
    assert_eq!(items[0]["artifacts"][0]["title"], "Login report");
    // Second run comes from the cache and says the same.
    let again = m.json(&["history", "--json"]);
    assert_eq!(again[0]["title"], "Login bug fix");
    assert_eq!(again[0]["artifacts"][0]["title"], "Login report");

    let by_date = text(&m.run(&["history", "--by-date"]));
    assert!(by_date.contains("1 artifact") && by_date.contains("/work/app") && by_date.contains("Login bug fix"), "{by_date}");
}

#[test]
fn rename_is_kept_outside_account_folders() {
    let mut m = Machine::new("rename", 47811);
    let project = m.account("work").join("projects").join("-work-app");
    fs::create_dir_all(&project).unwrap();
    let session_id = "aaaa0002-3333-4444-5555-666677778888";
    let transcript = [
        r#"{"type":"user","cwd":"/work/app","timestamp":"2026-11-10T06:20:00.000Z","message":{"role":"user","content":"fix the login bug"}}"#,
        r#"{"type":"ai-title","aiTitle":"Login bug fix","sessionId":"x"}"#,
    ];
    fs::write(project.join(format!("{session_id}.jsonl")), transcript.join("\n")).unwrap();
    let mut job = background("aaaa0002", "done", None);
    job["sessionId"] = json!(session_id);
    m.agents("work", json!([job]));
    m.seal();

    let title = |m: &Machine| m.json(&["history", "--json"])[0]["title"].clone();
    let renamed = m.run(&["rename", "work:aaaa0002", "Auth", "  rework "]);
    assert!(renamed.status.success(), "{}", text(&renamed));
    assert_eq!(title(&m), "Auth rework");
    assert_eq!(m.json(&["history", "--json"])[0]["renamed"], true);
    // The session it belongs to goes by the same name.
    assert_eq!(find(&m.json(&["list", "--all", "--json"]), "aaaa0002")["name"], "Auth rework");

    assert!(!m.run(&["rename", "work:aaaa0002"]).status.success(), "a rename needs a name or --reset");
    assert!(!m.run(&["rename", "work:ffff9999", "x"]).status.success(), "no such session");
    assert!(m.run(&["rename", "work:aaaa0002", "--reset"]).status.success());
    assert_eq!(title(&m), "Login bug fix");
    assert_eq!(find(&m.json(&["list", "--all", "--json"]), "aaaa0002")["name"], "job aaaa0002");

    // The page renames through the server.
    assert!(m.run(&["--no-open"]).status.success());
    let token = m.token();
    let json = "Content-Type: application/json\r\n";
    let body = json!({ "account": "work", "sessionId": session_id, "title": "From the page" }).to_string();
    assert_eq!(m.post("/api/sessions/rename", &token, json, &body).0, 200);
    assert_eq!(title(&m), "From the page");
    let (_, state) = m.get("/api/state", Some(&token), None);
    assert!(state.contains("From the page"), "the session list shows the new name: {state}");
    let unknown = json!({ "account": "work", "sessionId": "nope", "title": "x" }).to_string();
    assert_eq!(m.post("/api/sessions/rename", &token, json, &unknown).0, 400);
    let reset = json!({ "account": "work", "sessionId": session_id, "title": "" }).to_string();
    assert_eq!(m.post("/api/sessions/rename", &token, json, &reset).0, 200);
    assert_eq!(title(&m), "Login bug fix");
}

#[test]
fn server_guards_and_disconnect() {
    let mut m = Machine::new("server", 47808);
    m.agents("work", json!([background("aaaa0001", "blocked", None)]));
    m.seal();

    let started = m.run(&["--no-open"]);
    assert!(started.status.success() && text(&started).contains("started"), "{}", text(&started));
    let token = m.token();

    assert_eq!(m.get("/api/state", None, None).0, 401, "no token");
    assert_eq!(m.get("/api/state", Some("wrong"), None).0, 401, "wrong token");
    assert_eq!(m.get("/api/state", Some(&token), Some("evil.example")).0, 403, "wrong host");
    assert_eq!(m.get("/", None, Some("evil.example")).0, 403, "wrong host for the page");
    assert_eq!(m.get("/", None, None).0, 200, "the page itself needs no token");
    assert_eq!(m.post("/api/refresh", &token, "Content-Type: application/json\r\nOrigin: http://evil.example\r\n", "{}").0, 403);
    assert_eq!(m.post("/api/refresh", &token, "Content-Type: text/plain\r\n", "{}").0, 415);
    assert_eq!(m.post("/api/refresh", &token, "Content-Type: application/json\r\n", "{}").0, 200);
    assert_eq!(m.post("/api/nothing", &token, "Content-Type: application/json\r\n", "{}").0, 404);

    let (status, body) = m.get("/api/state", Some(&token), None);
    assert_eq!(status, 200);
    let state: Value = serde_json::from_str(&body[body.find('{').unwrap()..=body.rfind('}').unwrap()]).unwrap();
    assert_eq!(state["sessions"][0]["key"], "work:aaaa0001");
    assert!(state["accounts"].as_array().unwrap().iter().any(|a| a["name"] == "work"));
    assert!(state["sessions"][0].get("accountDir").is_none(), "the page is not told account folders' full paths");

    assert!(text(&m.run(&["--no-open"])).contains("already running"), "a second start must not start a second server");

    let (status, _) = m.post("/api/shutdown", &token, "Content-Type: application/json\r\n", "{}");
    assert_eq!(status, 200);
    let deadline = Instant::now() + Duration::from_secs(10);
    while m.state.join("ui.pid").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    assert!(!m.state.join("ui.pid").exists(), "Disconnect must stop the server and remove its marker");
    assert!(text(&m.run(&["stop"])).contains("not running"));
}

#[test]
fn server_stops_itself_when_no_page_is_open() {
    let mut m = Machine::new("idle", 47809);
    m.seal();
    let started = m.command(&["--no-open"]).env("CC_SESSIONS_IDLE_SECONDS", "2").stdin(Stdio::null()).output().unwrap();
    assert!(started.status.success(), "{}", text(&started));
    assert!(m.state.join("ui.pid").exists());
    let deadline = Instant::now() + Duration::from_secs(15);
    while m.state.join("ui.pid").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(200));
    }
    assert!(!m.state.join("ui.pid").exists(), "the server should stop after the idle limit");
    assert!(fs::read_to_string(m.state.join("ui.log")).unwrap().contains("no page open"));
}

#[test]
fn setup_can_be_repeated() {
    let mut m = Machine::new("setup", 47810);
    fs::write(m.home.join(".zshrc"), "export KEEP=1\nalias kt=\"CLAUDE_CONFIG_DIR=~/.claude-work claude\"\n").unwrap();
    m.seal();

    let first = m.run(&["setup", "--yes", "--command", "cs-test", "--port", "4999", "--shortcut"]);
    assert!(first.status.success(), "{}", text(&first));
    let config: Value = serde_json::from_str(&fs::read_to_string(m.state.join("config.json")).unwrap()).unwrap();
    assert_eq!(config["command"], "cs-test");
    assert_eq!(config["port"], 4999);
    assert_eq!(config["shortcut"], true);

    let registry: Value = serde_json::from_str(&fs::read_to_string(m.state.join("accounts.json")).unwrap()).unwrap();
    let names: Vec<&str> = registry["accounts"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["default", "work"]);

    if cfg!(windows) {
        assert!(m.root.join("localappdata").join("cc-sessions").join("bin").join("cs-test.cmd").exists());
    } else {
        // An alias the person already had for the folder keeps its name.
        assert_eq!(registry["accounts"][1]["alias"], "kt");
        assert!(m.home.join(".local").join("bin").join("cs-test").exists());
        let rc = fs::read_to_string(m.home.join(".zshrc")).unwrap();
        assert!(rc.starts_with("export KEEP=1\n"), "{rc}");
        assert_eq!(rc.matches("aliases.sh").count(), 2, "one line, naming the file twice: {rc}");
        assert!(m.home.join(".zshrc.cc-sessions-backup").exists());

        // A second run changes nothing and renames the command cleanly.
        let second = m.run(&["setup", "--yes", "--command", "cs-two"]);
        assert!(second.status.success(), "{}", text(&second));
        assert_eq!(fs::read_to_string(m.home.join(".zshrc")).unwrap(), rc);
        assert!(m.home.join(".local").join("bin").join("cs-two").exists());
        assert!(!m.home.join(".local").join("bin").join("cs-test").exists(), "the old command link is its own launcher");
        let config: Value = serde_json::from_str(&fs::read_to_string(m.state.join("config.json")).unwrap()).unwrap();
        assert_eq!(config["port"], 4999, "answers that were not given again are kept");
    }
}
