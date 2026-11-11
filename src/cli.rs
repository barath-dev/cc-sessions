//! Command line: start the web app, or list, close and manage from a terminal.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::accounts::{self, Account};
use crate::claude;
use crate::closing;
use crate::history;
use crate::os;
use crate::server::{self, HOST, VERSION};
use crate::sessions::{self, Session};
use crate::setup;
use crate::util::{age, err, home, ms_to_date, path_str, tilde, Error, Result};

const HELP: &str = "\
cc-sessions: see, close and resume Claude Code sessions across every account.

  cc-sessions                      start the web app if needed and open it
  cc-sessions stop                 stop the web app (same as Disconnect in the page)
  cc-sessions setup                change the command name, port and shortcuts

  cc-sessions list [--folder PATH] [--account NAME] [--open | --closed | --all] [--json]
  cc-sessions history [--folder PATH] [--account NAME] [--json]
  cc-sessions close <id>... | --folder PATH | --account NAME  [--yes]

  cc-sessions account list [--removed]
  cc-sessions account add NAME [--alias A] [--dir PATH] [--no-login]
  cc-sessions account rename NAME NEW-NAME [--alias A]
  cc-sessions account login NAME
  cc-sessions account remove NAME [--yes]
  cc-sessions account restore NAME [--alias A]

Session ids are shown as account:id. Closing stops a session and keeps its
conversation. Nothing under an account folder is ever deleted.

Options: --no-open (start without opening the browser), --version, --help
";

/// Flags that take a value; every other `--flag` is a switch.
const VALUE_FLAGS: &[&str] = &["folder", "account", "alias", "dir", "command", "port"];
const SWITCHES: &[&str] = &[
    "json", "open", "closed", "all", "yes", "no-login", "removed", "no-open", "shortcut", "no-shortcut", "no-import",
    "no-aliases", "help", "version",
];

pub struct Args {
    pub words: Vec<String>,
    values: HashMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Args> {
        let mut args = Args { words: Vec::new(), values: HashMap::new(), switches: Vec::new() };
        let mut iter = raw.into_iter();
        while let Some(arg) = iter.next() {
            let Some(flag) = arg.strip_prefix("--") else {
                if arg == "-h" {
                    args.switches.push("help".into());
                } else {
                    args.words.push(arg);
                }
                continue;
            };
            let (name, inline) = match flag.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (flag, None),
            };
            if VALUE_FLAGS.contains(&name) {
                let value = inline.or_else(|| iter.next()).ok_or_else(|| Error(format!("--{name} needs a value.")))?;
                args.values.insert(name.to_string(), value);
            } else if SWITCHES.contains(&name) && inline.is_none() {
                args.switches.push(name.to_string());
            } else {
                return err(format!("Unknown option --{name}. Try: cc-sessions --help"));
            }
        }
        Ok(args)
    }

    pub fn has(&self, switch: &str) -> bool {
        self.switches.iter().any(|s| s == switch)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    fn word(&self, index: usize, what: &str) -> Result<&str> {
        self.words.get(index).map(String::as_str).ok_or_else(|| Error(format!("Missing {what}. Try: cc-sessions --help")))
    }
}

pub fn main() -> i32 {
    let args = match Args::parse(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(e) => return fail(&e),
    };
    match run(&args) {
        Ok(code) => code,
        Err(e) => fail(&e),
    }
}

fn fail(e: &Error) -> i32 {
    eprintln!("cc-sessions: {e}");
    1
}

fn run(args: &Args) -> Result<i32> {
    if args.has("version") {
        println!("cc-sessions {VERSION}");
        return Ok(0);
    }
    if args.has("help") {
        print!("{HELP}");
        return Ok(0);
    }
    match args.words.first().map(String::as_str) {
        None | Some("ui") => cmd_open(args),
        Some("stop") => cmd_stop(),
        Some("serve") => server::serve().map(|_| 0),
        Some("setup") => setup::run(args).map(|_| 0),
        Some("list") => cmd_list(args),
        Some("history") => cmd_history(args),
        Some("close") => cmd_close(args),
        Some("account") => match args.words.get(1).map(String::as_str) {
            Some("list") => account_list(args),
            Some("add") => account_add(args),
            Some("rename") => account_rename(args),
            Some("login") => account_login(args),
            Some("remove") => account_remove(args),
            Some("restore") => account_restore(args),
            _ => err("Say what to do with accounts: list, add, rename, login, remove or restore."),
        },
        Some(other) => err(format!("Unknown command '{other}'. Try: cc-sessions --help")),
    }
}

// ------------------------------------------------------------------- formatting

fn table(rows: &[Vec<String>], indent: &str) {
    let Some(first) = rows.first() else { return };
    let widths: Vec<usize> =
        (0..first.len() - 1).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    for row in rows {
        let mut line = String::from(indent);
        for (i, cell) in row.iter().enumerate() {
            match widths.get(i) {
                Some(width) => line.push_str(&format!("{cell:<width$}  ")),
                None => line.push_str(cell),
            }
        }
        println!("{}", line.trim_end());
    }
}

pub fn confirm(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(n) if n > 0 => matches!(line.trim().to_lowercase().as_str(), "y" | "yes"),
        _ => {
            println!();
            false
        }
    }
}

fn grouped<T>(items: Vec<T>, folder: impl Fn(&T) -> String) -> Vec<(String, Vec<T>)> {
    let mut groups: BTreeMap<(String, String), Vec<T>> = BTreeMap::new();
    for item in items {
        let name = Some(folder(&item)).filter(|f| !f.is_empty()).unwrap_or_else(|| "(unknown folder)".into());
        groups.entry((name.to_lowercase(), name)).or_default().push(item);
    }
    groups.into_iter().map(|((_, name), items)| (name, items)).collect()
}

fn warn(notices: &BTreeMap<String, String>) {
    for (name, notice) in notices {
        eprintln!("warning: account '{name}': {notice}");
    }
}

// ---------------------------------------------------------------------- web app

/// A tiny HTTP client for talking to our own server. Returns (status, body).
fn http(method: &str, path: &str, body: Option<&str>, timeout: Duration) -> Option<(u16, String)> {
    let port = server::port();
    let token = server::load_token().ok()?;
    let addr: SocketAddr = format!("{HOST}:{port}").parse().ok()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1)).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {HOST}:{port}\r\nX-CC-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let status = text.split_whitespace().nth(1)?.parse().ok()?;
    let (_, rest) = text.split_once("\r\n\r\n")?;
    // The reply is one JSON object; take it whether or not the body is chunked.
    let json = match (rest.find('{'), rest.rfind('}')) {
        (Some(start), Some(end)) if end > start => rest[start..=end].to_string(),
        _ => rest.to_string(),
    };
    Some((status, json))
}

/// The running server's ping reply, or None if ours is not answering.
fn ping() -> Option<Value> {
    let (status, body) = http("GET", "/api/ping", None, Duration::from_secs(2))?;
    let data: Value = serde_json::from_str(&body).ok()?;
    (status == 200 && data.get("app").and_then(Value::as_str) == Some("cc-sessions")).then_some(data)
}

fn port_busy() -> bool {
    format!("{HOST}:{}", server::port())
        .parse::<SocketAddr>()
        .is_ok_and(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok())
}

fn cmd_open(args: &Args) -> Result<i32> {
    let port = server::port();
    if ping().is_some() {
        println!("Claude Sessions is already running.");
    } else {
        if port_busy() {
            return err(format!("Port {port} is in use by another program. Stop it, or pick another port with: cc-sessions setup"));
        }
        fs::create_dir_all(crate::store::state_dir())?;
        let log = fs::OpenOptions::new().create(true).append(true).open(server::log_file())?;
        let exe = std::env::current_exe()?;
        let mut cmd = Command::new(exe);
        cmd.arg("serve").current_dir(home()).stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().to_uppercase().starts_with("CLAUDE") {
                cmd.env_remove(key);
            }
        }
        // On its own, so closing this terminal does not take the server down.
        os::detach(&mut cmd);
        cmd.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while ping().is_none() {
            if Instant::now() > deadline {
                return err(format!("The server did not start. See {}.", tilde(&path_str(&server::log_file()))));
            }
            thread::sleep(Duration::from_millis(100));
        }
        println!("Claude Sessions started.");
    }
    let base = format!("http://{HOST}:{port}");
    if !args.has("no-open") {
        if let Err(e) = os::open_browser(&format!("{base}/?t={}", server::load_token()?)) {
            eprintln!("warning: {e}");
        }
    }
    println!("  {base}");
    println!("  Stop it with Disconnect in the page, or: cc-sessions stop");
    Ok(0)
}

fn cmd_stop() -> Result<i32> {
    let Some(info) = ping() else {
        println!("Claude Sessions is not running.");
        return Ok(0);
    };
    let pid = info.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    let _ = http("POST", "/api/shutdown", Some("{}"), Duration::from_secs(5));
    // A close in progress is allowed to finish first.
    let deadline = Instant::now() + Duration::from_secs(60);
    while os::pid_alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if os::pid_alive(pid) {
        return err(format!("The server (pid {pid}) has not stopped yet."));
    }
    println!("Claude Sessions stopped. Your Claude sessions are untouched.");
    Ok(0)
}

// --------------------------------------------------------------------- sessions

fn check_account(args: &Args) -> Result<()> {
    match args.value("account") {
        Some(name) => accounts::get_account(name).map(|_| ()),
        None => Ok(()),
    }
}

fn selected(sess: &Session, args: &Args) -> bool {
    args.value("account").is_none_or(|a| a == sess.account) && args.value("folder").is_none_or(|f| sessions::in_folder(&sess.cwd, f))
}

fn cmd_list(args: &Args) -> Result<i32> {
    check_account(args)?;
    let (_, all, notices) = sessions::all_active()?;
    let shown: Vec<Session> = all
        .into_iter()
        .filter(|s| selected(s, args))
        .filter(|s| if args.has("closed") { !s.open } else { args.has("all") || s.open })
        .collect();
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&shown).map_err(|e| Error(e.to_string()))?);
        return Ok(0);
    }
    let none = shown.is_empty();
    for (folder, group) in grouped(shown, |s| s.cwd.clone()) {
        println!("{}", tilde(&folder));
        let rows: Vec<Vec<String>> = group
            .iter()
            .map(|s| vec![s.key.clone(), s.kind.clone(), s.status.clone(), age(s.updated_at), s.name.clone()])
            .collect();
        table(&rows, "  ");
    }
    if none {
        println!("No sessions match.");
    }
    warn(&notices);
    Ok(0)
}

fn cmd_history(args: &Args) -> Result<i32> {
    check_account(args)?;
    let items: Vec<_> = history::history_items()?
        .into_iter()
        .filter(|i| args.value("account").is_none_or(|a| a == i.account))
        .filter(|i| args.value("folder").is_none_or(|f| sessions::in_folder(&i.cwd, f)))
        .collect();
    if args.has("json") {
        println!("{}", serde_json::to_string_pretty(&items).map_err(|e| Error(e.to_string()))?);
        return Ok(0);
    }
    let none = items.is_empty();
    for (folder, group) in grouped(items, |i| i.folder_label.clone()) {
        println!("{}", tilde(&folder));
        let rows: Vec<Vec<String>> = group
            .iter()
            .map(|i| {
                vec![
                    ms_to_date(i.last_activity),
                    i.account.clone(),
                    i.id.clone(),
                    format!("{} prompts", i.prompts),
                    i.title.chars().take(80).collect(),
                ]
            })
            .collect();
        table(&rows, "  ");
    }
    if none {
        println!("No saved conversations match.");
    }
    Ok(0)
}

fn ancestors() -> Vec<u32> {
    let mut pids = Vec::new();
    let mut pid = std::process::id();
    while let Some(parent) = os::parent_pid(pid).filter(|p| *p > 1 && !pids.contains(p)) {
        pids.push(parent);
        pid = parent;
    }
    pids
}

fn describe(sess: &Session) -> String {
    format!("{}  {}  in {}  ({}, {})", sess.key, sess.name, tilde(&sess.cwd), sess.kind, sess.status)
}

fn cmd_close(args: &Args) -> Result<i32> {
    let ids = &args.words[1..];
    if ids.is_empty() && args.value("folder").is_none() && args.value("account").is_none() {
        return err("Say what to close: one or more ids, --folder PATH, or --account NAME.");
    }
    check_account(args)?;
    let (_, all, notices) = sessions::all_active()?;
    warn(&notices);

    let mut failed = false;
    let mut targets: Vec<&Session> = Vec::new();
    if ids.is_empty() {
        targets = all.iter().filter(|s| s.open && selected(s, args)).collect();
    } else {
        for token in ids {
            match sessions::find_sessions(&all, token).as_slice() {
                [] => {
                    eprintln!("No session matches '{token}'.");
                    failed = true;
                }
                [one] => targets.push(one),
                many => {
                    eprintln!("'{token}' matches more than one session; use account:id:");
                    for sess in many {
                        eprintln!("  {}", describe(sess));
                    }
                    failed = true;
                }
            }
        }
        targets.retain(|s| selected(s, args));
    }

    // Never close the session this command was run from.
    let own_id = std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default();
    let parents = ancestors();
    let mut kept = Vec::new();
    for sess in targets {
        if (!own_id.is_empty() && sess.session_id == own_id) || sess.pid.is_some_and(|pid| parents.contains(&pid)) {
            println!("Skipped {} ({}): this command is running inside it.", sess.key, sess.name);
        } else if !sess.open {
            println!("Skipped {} ({}): already closed.", sess.key, sess.name);
        } else {
            kept.push(sess);
        }
    }
    if kept.is_empty() {
        println!("Nothing to close.");
        return Ok(failed as i32);
    }

    let (background, interactive): (Vec<&Session>, Vec<&Session>) = kept.into_iter().partition(|s| s.is_background());
    let mut chosen = Vec::new();
    if !background.is_empty() {
        println!("Background sessions to close ({}):", background.len());
        for sess in &background {
            println!("  {}", describe(sess));
        }
        if args.has("yes") || confirm("Close these? Their conversations are kept.") {
            chosen.extend(background);
        }
    }
    // Interactive sessions are someone's open terminal: always ask, one by one.
    for sess in interactive {
        let how = if os::GRACEFUL_CLOSE { "open in a terminal" } else { "open in a terminal; it will be ended immediately" };
        if confirm(&format!("Close {} in {} ({}, {how})?", sess.name, tilde(&sess.cwd), sess.account)) {
            chosen.push(sess);
        }
    }
    for sess in chosen {
        let result = closing::close_session(sess);
        println!("{:6}  {}  {}: {}", if result.ok { "closed" } else { "FAILED" }, sess.key, sess.name, result.message);
        failed = failed || !result.ok;
    }
    Ok(failed as i32)
}

// --------------------------------------------------------------------- accounts

/// Run claude for an account in this terminal, so sign-in prompts work.
fn claude_here(acc: &Account, args: &[&str]) -> Result<()> {
    claude::command(acc)?.args(args).status()?;
    Ok(())
}

fn alias_note() {
    println!("Shortcuts changed. {}", os::alias_hint());
}

fn shown_alias(acc: &Account) -> String {
    acc.alias.clone().unwrap_or_else(|| if acc.is_default() { "claude".into() } else { "-".into() })
}

fn account_list(args: &Args) -> Result<i32> {
    let everyone = accounts::load_accounts(true)?;
    let removed = args.has("removed");
    let accounts: Vec<Account> = everyone.into_iter().filter(|a| a.removed == removed).collect();
    if accounts.is_empty() {
        println!("{}", if removed { "No removed accounts." } else { "No accounts. Add one with: cc-sessions account add NAME" });
        return Ok(0);
    }
    if removed {
        let mut rows = vec![vec!["NAME".to_string(), "ALIAS".into(), "FOLDER".into()]];
        rows.extend(accounts.iter().map(|a| vec![a.name.clone(), a.alias.clone().unwrap_or("-".into()), tilde(&a.dir)]));
        table(&rows, "");
        println!("Their folders and logins are untouched. Bring one back with: cc-sessions account restore NAME");
        return Ok(0);
    }
    let infos: Vec<claude::AuthInfo> = thread::scope(|scope| {
        let handles: Vec<_> = accounts.iter().map(|acc| scope.spawn(move || claude::auth_status(acc))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    let (sessions, _) = sessions::all_sessions(&accounts)?;
    let mut rows = vec![["NAME", "ALIAS", "LOGIN", "ORG", "PLAN", "OPEN", "FOLDER"].map(String::from).to_vec()];
    for (acc, info) in accounts.iter().zip(infos) {
        rows.push(vec![
            acc.name.clone(),
            shown_alias(acc),
            info.email.unwrap_or_else(|| "not logged in".into()),
            info.org.unwrap_or_else(|| "-".into()),
            info.plan.unwrap_or_else(|| "-".into()),
            sessions.iter().filter(|s| s.open && s.account == acc.name).count().to_string(),
            tilde(&acc.dir),
        ]);
    }
    table(&rows, "");
    let extra = accounts::unregistered_folders()?;
    if !extra.is_empty() {
        println!("\nFolders not registered as accounts:");
        for path in extra {
            println!("  {0}   (add with: cc-sessions account add NAME --dir {0})", tilde(&path));
        }
    }
    Ok(0)
}

fn account_add(args: &Args) -> Result<i32> {
    let (acc, created) = accounts::add_account(args.word(2, "the account name")?, args.value("alias"), args.value("dir"))?;
    let how = if created { "created" } else { "already existed and was adopted as is" };
    println!("Added account '{}': folder {} {how}, alias {}.", acc.name, tilde(&acc.dir), shown_alias(&acc));
    if !args.has("no-login") && !claude::auth_status(&acc).logged_in {
        println!("Signing in...");
        claude_here(&acc, &["auth", "login"])?;
    }
    println!("Login: {}", claude::auth_status(&acc).email.unwrap_or_else(|| "not logged in".into()));
    alias_note();
    Ok(0)
}

fn account_rename(args: &Args) -> Result<i32> {
    let acc = accounts::rename_account(args.word(2, "the account name")?, Some(args.word(3, "the new name")?), args.value("alias"))?;
    println!("Account is now '{}' with alias {}. Its folder {} was not touched.", acc.name, shown_alias(&acc), tilde(&acc.dir));
    alias_note();
    Ok(0)
}

fn account_login(args: &Args) -> Result<i32> {
    let acc = accounts::get_account(args.word(2, "the account name")?)?;
    let info = claude::auth_status(&acc);
    if info.logged_in {
        let (sessions, _) = sessions::all_sessions(std::slice::from_ref(&acc))?;
        println!("'{}' is logged in as {}.", acc.name, info.email.unwrap_or_default());
        let live: Vec<&Session> = sessions.iter().filter(|s| s.live).collect();
        if !live.is_empty() {
            println!("These running sessions will lose their login:");
            for sess in live {
                println!("  {}", describe(sess));
            }
        }
        if !confirm("Log out and sign in again? History and settings are kept.") {
            return Ok(1);
        }
        claude_here(&acc, &["auth", "logout"])?;
    }
    claude_here(&acc, &["auth", "login"])?;
    let info = claude::auth_status(&acc);
    println!("Login: {}", info.email.clone().unwrap_or_else(|| "not logged in".into()));
    Ok(!info.logged_in as i32)
}

fn account_remove(args: &Args) -> Result<i32> {
    let acc = accounts::get_account(args.word(2, "the account name")?)?;
    let (sessions, _) = sessions::all_sessions(std::slice::from_ref(&acc))?;
    let open: Vec<&Session> = sessions.iter().filter(|s| s.open).collect();
    if !open.is_empty() {
        println!("'{}' has {} open sessions. They keep running but will be hidden:", acc.name, open.len());
        for sess in open {
            println!("  {}", describe(sess));
        }
    }
    if !args.has("yes") && !confirm(&format!("Remove account '{}' from Claude Sessions? Nothing on disk is deleted.", acc.name)) {
        return Ok(1);
    }
    accounts::remove_account(&acc.name)?;
    println!("Removed '{}'. Its folder {}, login and history are untouched.", acc.name, tilde(&acc.dir));
    println!("Bring it back with: cc-sessions account restore {}", acc.name);
    alias_note();
    Ok(0)
}

fn account_restore(args: &Args) -> Result<i32> {
    let acc = accounts::restore_account(args.word(2, "the account name")?, args.value("alias"))?;
    println!("Restored '{}' with alias {}.", acc.name, shown_alias(&acc));
    alias_note();
    Ok(0)
}
