//! Mac and Linux.

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::{which, ShellHook};
use crate::shell::{self, Launch, Shortcut};
use crate::store;
use crate::util::{err, home, path_str, squash, tilde, Error, Result, NAME, OLD_NAME};
use crate::{proc, store::state_dir};

pub const IS_WINDOWS: bool = false;
/// Closing an interactive session asks it to exit (SIGTERM) rather than forcing it.
pub const GRACEFUL_CLOSE: bool = true;
pub const OS_NAME: &str = if cfg!(target_os = "macos") { "macos" } else { "linux" };

pub fn executable_extensions() -> Vec<String> {
    vec![String::new()]
}

pub fn default_state_dir() -> PathBuf {
    home().join(".config").join(NAME)
}

/// The settings folder from before the rename.
pub fn old_state_dir() -> PathBuf {
    home().join(".config").join(OLD_NAME)
}

fn command_dir() -> PathBuf {
    home().join(".local").join("bin")
}

/// Where a tool's command may be when it is not on PATH (as for a server started at login).
pub fn program_fallbacks(program: &str) -> Vec<PathBuf> {
    let mut found = vec![
        PathBuf::from("/opt/homebrew/bin").join(program),
        PathBuf::from("/usr/local/bin").join(program),
        home().join(".local/bin").join(program),
    ];
    if program == "claude" {
        found.push(home().join(".claude/local/claude"));
    }
    found
}

pub fn lock_exclusive(file: &File) -> std::io::Result<()> {
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return !zombie(pid);
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// A process that has exited but not been collected by its parent, as happens in a
/// container whose first process never collects them. It still answers signal 0.
#[cfg(target_os = "linux")]
fn zombie(pid: u32) -> bool {
    // "pid (name) S ...": the state follows the name, which may itself hold spaces and brackets.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().chars().next()))
        == Some('Z')
}

#[cfg(not(target_os = "linux"))]
fn zombie(_pid: u32) -> bool {
    false
}

fn ps(pid: u32, field: &str) -> String {
    // LC_ALL and TZ pin the format: this is exactly how Claude Code records `procStart`.
    let mut cmd = Command::new("ps");
    cmd.args(["-o", field, "-p", &pid.to_string()]).env("LC_ALL", "C").env("TZ", "UTC");
    proc::run(&mut cmd, Duration::from_secs(5)).map(|o| squash(&o.stdout)).unwrap_or_default()
}

/// Make sure `pid` still is the session described by its `sessions/<pid>.json`
/// record before anything is sent to it.
pub fn verify_session_process(pid: u32, record: &Value) -> Result<()> {
    let expected = squash(record.get("procStart").and_then(Value::as_str).unwrap_or(""));
    if !expected.is_empty() {
        if expected != ps(pid, "lstart=") {
            return err("The process id has been reused by another program. Nothing was stopped.");
        }
    } else if !ps(pid, "command=").contains("claude") {
        return err("That process does not look like a Claude session. Nothing was stopped.");
    }
    Ok(())
}

pub fn parent_pid(pid: u32) -> Option<u32> {
    ps(pid, "ppid=").parse().ok()
}

/// "[[dd-]hh:]mm:ss", as `ps -o etime` writes how long a process has run, in seconds.
fn elapsed_seconds(text: &str) -> Option<i64> {
    let (days, clock) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<i64>().ok()?, rest),
        None => (0, text),
    };
    let parts: Vec<i64> = clock.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let seconds = parts.iter().fold(0, |total, part| total * 60 + part);
    (!parts.is_empty() && parts.len() <= 3).then_some(days * 86_400 + seconds)
}

/// Every process of this user, with its command line.
pub fn processes() -> Vec<super::Process> {
    let mut cmd = Command::new("ps");
    // -x includes processes without a terminal; ww keeps long command lines whole.
    cmd.args(["-x", "-ww", "-o", "pid=,etime=,args=", "-U"]).arg(unsafe { libc::getuid() }.to_string()).env("LC_ALL", "C");
    let Ok(out) = proc::run(&mut cmd, Duration::from_secs(5)) else { return Vec::new() };
    let now = crate::util::now_ms();
    out.stdout
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let ran = words.next().and_then(elapsed_seconds);
            let args = words.collect::<Vec<_>>().join(" ");
            Some(super::Process { pid, args, started_at: ran.map(|s| now - s * 1000) })
        })
        .collect()
}

/// The folder each process runs in and the files it has open.
#[cfg(target_os = "macos")]
pub fn open_files(pids: &[u32]) -> std::collections::HashMap<u32, super::OpenFiles> {
    let mut found = std::collections::HashMap::new();
    if pids.is_empty() {
        return found;
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let mut cmd = Command::new("lsof");
    // -F pfn: one field to a line, p for the process, f for the descriptor, n for the path.
    cmd.args(["-n", "-P", "-w", "-F", "pfn", "-p", &list]);
    let Ok(out) = proc::run(&mut cmd, Duration::from_secs(10)) else { return found };
    let (mut pid, mut cwd_next) = (0, false);
    for line in out.stdout.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => pid = value.parse().unwrap_or(0),
            "f" => cwd_next = value == "cwd",
            "n" if pid != 0 => {
                let entry: &mut super::OpenFiles = found.entry(pid).or_default();
                if cwd_next {
                    entry.cwd = Some(PathBuf::from(value));
                } else if value.starts_with('/') {
                    entry.files.push(PathBuf::from(value));
                }
            }
            _ => {}
        }
    }
    found
}

#[cfg(not(target_os = "macos"))]
pub fn open_files(pids: &[u32]) -> std::collections::HashMap<u32, super::OpenFiles> {
    pids.iter()
        .map(|pid| {
            let base = PathBuf::from(format!("/proc/{pid}"));
            let files = fs::read_dir(base.join("fd"))
                .map(|fds| fds.flatten().filter_map(|fd| fs::read_link(fd.path()).ok()).filter(|p| p.is_absolute()).collect())
                .unwrap_or_default();
            (*pid, super::OpenFiles { cwd: fs::read_link(base.join("cwd")).ok(), files })
        })
        .collect()
}

/// Ask the process to exit. It can finish what it is writing first.
pub fn terminate(pid: u32) -> Result<()> {
    // SAFETY: plain kill(2) on a pid the caller has verified.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(Error(format!("could not signal pid {pid}: {}", std::io::Error::last_os_error())))
    }
}

/// The process groups of `pid`'s children that each lead a group of their own. A program
/// that starts commands that way (Codex) leaves them running when it is stopped.
pub fn child_groups(pid: u32) -> Vec<u32> {
    let mut cmd = Command::new("ps");
    cmd.args(["-x", "-o", "pid=,ppid=,pgid=", "-U"]).arg(unsafe { libc::getuid() }.to_string()).env("LC_ALL", "C");
    let Ok(out) = proc::run(&mut cmd, Duration::from_secs(5)) else { return Vec::new() };
    out.stdout
        .lines()
        .filter_map(|line| {
            let ids: Vec<u32> = line.split_whitespace().filter_map(|w| w.parse().ok()).collect();
            matches!(ids[..], [child, parent, group] if parent == pid && group == child).then_some(ids[0])
        })
        .collect()
}

/// Ask every process in a group to exit.
pub fn terminate_group(group: u32) -> Result<()> {
    // SAFETY: plain kill(2) on a process group the caller found under a verified pid.
    if unsafe { libc::kill(-(group as libc::pid_t), libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(Error(format!("could not signal group {group}: {}", std::io::Error::last_os_error())))
    }
}

/// Start in its own session, so closing the terminal does not take it down.
pub fn detach(cmd: &mut Command) {
    // SAFETY: setsid is async-signal-safe and is the only thing done before exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

/// Helper programs need no special flags here (see the Windows version).
pub fn quiet(_cmd: &mut Command) {}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// A flag that turns true on SIGTERM or Ctrl-C, so the server can stop cleanly.
pub fn stop_flag() -> &'static AtomicBool {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    &STOP
}

fn spawn_quietly(cmd: &mut Command) -> Result<()> {
    detach(cmd);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ()).map_err(Error::from)
}

fn is_wsl() -> bool {
    fs::read_to_string("/proc/version").map(|v| v.to_lowercase().contains("microsoft")).unwrap_or(false)
}

pub fn open_browser(url: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        return spawn_quietly(Command::new("open").arg(url));
    }
    if is_wsl() {
        if let Some(wslview) = which("wslview") {
            return spawn_quietly(Command::new(wslview).arg(url));
        }
        if let Some(cmd) = which("cmd.exe") {
            return spawn_quietly(Command::new(cmd).args(["/c", "start", "", url]));
        }
    }
    for opener in ["xdg-open", "gio", "sensible-browser"] {
        if let Some(path) = which(opener) {
            let mut cmd = Command::new(path);
            if opener == "gio" {
                cmd.arg("open");
            }
            return spawn_quietly(cmd.arg(url));
        }
    }
    err("No way to open a browser was found. Open the address by hand.")
}

/// The command as the person would type it, for showing and copying.
pub fn display_command(launch: &Launch) -> String {
    shell::posix(launch, true)
}

/// Where the installer puts the program. An install from before the rename keeps
/// its place, `~/.local/share/cc-sessions/cc-sessions`, which the installer updates.
pub fn installed_program() -> PathBuf {
    let share = home().join(".local").join("share");
    let old = share.join(OLD_NAME).join(OLD_NAME);
    let new = share.join(NAME).join(NAME);
    if !new.exists() && old.exists() {
        old
    } else {
        new
    }
}

/// Fetch a URL into a file with the system's curl, or wget.
pub fn download(url: &str, to: &Path, timeout: Duration) -> Result<()> {
    let agent = format!("{NAME}/{}", env!("CARGO_PKG_VERSION"));
    let seconds = timeout.as_secs().to_string();
    let mut cmd = if let Some(curl) = which("curl") {
        let mut cmd = Command::new(curl);
        cmd.args(["-fsSL", "--max-time", &seconds, "-A", &agent, "-o"]).arg(to).arg(url);
        cmd
    } else if let Some(wget) = which("wget") {
        let mut cmd = Command::new(wget);
        cmd.args(["-q", "-T", &seconds, "-U", &agent, "-O"]).arg(to).arg(url);
        cmd
    } else {
        return err("Neither curl nor wget was found, so it cannot download.");
    };
    let out = proc::run(&mut cmd, timeout + Duration::from_secs(5))?;
    if !out.ok {
        return err(format!("Download failed: {}", out.last_line()));
    }
    Ok(())
}

/// Put a new program in place of the installed one. The rename is atomic, and a copy that
/// is running keeps the file it started from.
pub fn replace_program(new: &Path, target: &Path) -> Result<()> {
    let staged = target.with_extension("new");
    fs::copy(new, &staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    fs::rename(&staged, target)?;
    Ok(())
}

/// The system's release, for problem reports: "macOS 26.1" or "Linux 6.8.0".
pub fn os_release() -> String {
    let (program, args, name): (&str, &[&str], &str) =
        if cfg!(target_os = "macos") { ("sw_vers", &["-productVersion"], "macOS") } else { ("uname", &["-r"], "Linux") };
    let version = proc::run(Command::new(program).args(args), Duration::from_secs(5)).map(|o| o.stdout.trim().to_string()).unwrap_or_default();
    format!("{name} {version}").trim().to_string()
}

/// An app by any of these names in /Applications or ~/Applications. Only Macs have them.
pub fn find_app(names: &[&str]) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    names.iter().flat_map(|name| [Path::new("/Applications").join(name), home().join("Applications").join(name)]).find(|path| path.is_dir())
}

/// Open a folder in a program: an app through `open -a`, a command by giving it the folder.
pub fn open_folder_in(program: &Path, folder: &Path) -> Result<()> {
    if program.extension().is_some_and(|ext| ext == "app") {
        return spawn_quietly(Command::new("open").arg("-a").arg(program).arg(folder));
    }
    spawn_quietly(Command::new(program).arg(folder).current_dir(folder))
}

const ITERM_SCRIPT: &str = r#"
on run argv
  tell application "iTerm"
    activate
    set newWindow to (create window with default profile)
    tell current session of newWindow to write text (item 1 of argv)
  end tell
end run
"#;

const TERMINAL_SCRIPT: &str = r#"
on run argv
  tell application "Terminal"
    activate
    do script (item 1 of argv)
  end tell
end run
"#;

#[cfg(target_os = "macos")]
fn has_iterm() -> bool {
    // iTerm may live in /Applications or ~/Applications; fall back to asking Spotlight.
    if Path::new("/Applications/iTerm.app").is_dir() || home().join("Applications/iTerm.app").is_dir() {
        return true;
    }
    let mut cmd = Command::new("mdfind");
    cmd.arg("kMDItemCFBundleIdentifier == 'com.googlecode.iterm2'");
    proc::run(&mut cmd, Duration::from_secs(5)).map(|o| !o.stdout.trim().is_empty()).unwrap_or(false)
}

/// Run the launch in a new terminal window. Returns the terminal's name.
#[cfg(target_os = "macos")]
pub fn open_terminal(launch: &Launch) -> Result<String> {
    let (app, script) = if has_iterm() { ("iTerm", ITERM_SCRIPT) } else { ("Terminal", TERMINAL_SCRIPT) };
    let mut cmd = Command::new("osascript");
    // The command is typed into the new window's shell. Ctrl-U first clears what
    // reached it before, such as keys pressed as the window came to the front.
    cmd.args(["-e", script, &format!("\u{15}{}", shell::posix(launch, false))]);
    let out = proc::run(&mut cmd, Duration::from_secs(30)).map_err(|_| {
        Error(format!("{app} did not respond. If macOS asked for permission to control it, allow it and try again."))
    })?;
    if !out.ok {
        return err(format!("Could not open {app}: {}", out.last_line()));
    }
    Ok(app.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn open_terminal(launch: &Launch) -> Result<String> {
    let _ = (ITERM_SCRIPT, TERMINAL_SCRIPT);
    // Keep the window open on a shell once claude exits.
    let text = format!("{}; exec \"${{SHELL:-sh}}\"", shell::posix(launch, false));
    let run = ["sh", "-c", text.as_str()];

    if is_wsl() {
        if let Some(wt) = which("wt.exe") {
            spawn_quietly(Command::new(wt).args(["wsl.exe", "--"]).args(run))?;
            return Ok("Windows Terminal".into());
        }
    }
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return err("This machine has no desktop to open a terminal window on. Use Copy and run the command yourself.");
    }
    // (program, arguments that come before the command to run)
    let preferred = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty());
    let mut candidates: Vec<(String, Vec<&str>)> = Vec::new();
    if let Some(term) = &preferred {
        candidates.push((term.clone(), vec!["-e"]));
    }
    for (name, lead) in [
        ("x-terminal-emulator", vec!["-e"]),
        ("gnome-terminal", vec!["--"]),
        ("konsole", vec!["-e"]),
        ("xfce4-terminal", vec!["-x"]),
        ("kitty", vec![]),
        ("alacritty", vec!["-e"]),
        ("wezterm", vec!["start", "--"]),
        ("xterm", vec!["-e"]),
    ] {
        candidates.push((name.to_string(), lead));
    }
    for (name, lead) in candidates {
        if let Some(path) = which(&name) {
            spawn_quietly(Command::new(path).args(lead).args(run))?;
            return Ok(name);
        }
    }
    err("No terminal program was found. Use Copy and run the command yourself.")
}

// ------------------------------------------------------------ account shortcuts

fn login_shell() -> String {
    std::env::var("SHELL").ok().and_then(|s| s.rsplit('/').next().map(String::from)).unwrap_or_default()
}

fn rc_file() -> Option<PathBuf> {
    match login_shell().as_str() {
        "zsh" => Some(home().join(".zshrc")),
        "bash" if cfg!(target_os = "macos") => Some(home().join(".bash_profile")),
        "bash" => Some(home().join(".bashrc")),
        _ => None,
    }
}

fn fish_conf() -> Option<PathBuf> {
    let dir = home().join(".config").join("fish");
    dir.is_dir().then(|| dir.join("conf.d").join(format!("{NAME}.fish")))
}

/// The fish file written before the rename, if there is one of ours.
pub fn old_fish_conf() -> Option<PathBuf> {
    let file = home().join(".config").join("fish").join("conf.d").join(format!("{OLD_NAME}.fish"));
    fs::read_to_string(&file).is_ok_and(|text| text.contains("Generated by cc-sessions")).then_some(file)
}

fn command_dir_on_path() -> bool {
    let dir = command_dir();
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d == dir)).unwrap_or(false)
}

/// Rewrite the generated shortcut files from the registry.
pub fn write_account_shortcuts(entries: &[Shortcut], manage: bool) -> Result<()> {
    store::write_text(&state_dir().join("aliases.sh"), &shell::aliases_sh(entries), false)?;
    if manage {
        if let Some(fish) = fish_conf() {
            let mut text = shell::aliases_fish(entries);
            if !command_dir_on_path() {
                text.push_str("fish_add_path -g ~/.local/bin\n");
            }
            store::write_text(&fish, &text, false)?;
        }
    }
    Ok(())
}

pub fn alias_hint() -> String {
    match rc_file() {
        Some(rc) => format!("Open a new terminal, or run: source {}", tilde(&path_str(&rc))),
        None => "Open a new terminal to use them.".to_string(),
    }
}

/// The line `setup` offers to add to the shell's startup file.
pub fn shell_hook() -> Option<ShellHook> {
    let file = rc_file()?;
    let aliases = tilde(&path_str(&state_dir().join("aliases.sh")));
    let mut lines = vec![format!("[ -f {aliases} ] && . {aliases}")];
    if !command_dir_on_path() {
        lines.push("export PATH=\"$HOME/.local/bin:$PATH\"".to_string());
    }
    Some(ShellHook { file, lines })
}

// ------------------------------------------------------------------- installing

fn is_our_link(path: &Path) -> bool {
    fs::read_link(path).map(|target| target.file_name().is_some_and(|n| n == NAME || n == OLD_NAME)).unwrap_or(false)
}

/// Whether `name` is a command link of this tool's, and to which program.
pub fn command_target(name: &str) -> Option<PathBuf> {
    let link = command_dir().join(name);
    is_our_link(&link).then(|| fs::read_link(&link).ok()).flatten()
}

/// Make `name` start the program: a link in `~/.local/bin`. Refuses to replace
/// anything that is not one of this tool's own links.
pub fn install_command(name: &str, exe: &Path, previous: Option<&str>) -> Result<PathBuf> {
    let dir = command_dir();
    fs::create_dir_all(&dir)?;
    let link = dir.join(name);
    if link.symlink_metadata().is_ok() {
        if !is_our_link(&link) {
            return err(format!("{} already exists and is not a Paddock link. Pick another command name.", tilde(&path_str(&link))));
        }
        fs::remove_file(&link)?;
    }
    std::os::unix::fs::symlink(exe, &link)?;
    if let Some(old) = previous.filter(|old| *old != name) {
        let old_link = dir.join(old);
        if is_our_link(&old_link) {
            fs::remove_file(&old_link)?;
        }
    }
    Ok(link)
}

/// A clickable shortcut that starts the server and opens the page.
#[cfg(target_os = "macos")]
pub fn install_shortcut(exe: &Path) -> Result<PathBuf> {
    let app = home().join("Applications").join("Paddock.app");
    let macos = app.join("Contents").join("MacOS");
    fs::create_dir_all(&macos)?;
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Paddock</string>
  <key>CFBundleIdentifier</key><string>dev.paddock.launcher</string>
  <key>CFBundleExecutable</key><string>launch</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
"#;
    fs::write(app.join("Contents").join("Info.plist"), plist)?;
    let resources = app.join("Contents").join("Resources");
    fs::create_dir_all(&resources)?;
    fs::write(resources.join("icon.icns"), include_bytes!("../../assets/icon.icns"))?;
    let script = macos.join("launch");
    fs::write(&script, format!("#!/bin/sh\nexec {} ui\n", shell::sh_quote(&path_str(exe))))?;
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    Ok(app)
}

#[cfg(not(target_os = "macos"))]
pub fn install_shortcut(exe: &Path) -> Result<PathBuf> {
    let dir = home().join(".local").join("share").join("applications");
    fs::create_dir_all(&dir)?;
    let entry = dir.join(format!("{NAME}.desktop"));
    let icon = store::state_file("icon.png");
    fs::create_dir_all(state_dir())?;
    fs::write(&icon, include_bytes!("../../assets/icon.png"))?;
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Paddock\nComment=Every Claude Code and Codex session on your machine, in one place\nExec=\"{}\" ui\nIcon={}\nTerminal=false\nCategories=Development;\n",
        path_str(exe),
        path_str(&icon)
    );
    fs::write(&entry, text)?;
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o755))?;
    Ok(entry)
}

/// The app shortcut made before the rename, if there is one.
#[cfg(target_os = "macos")]
pub fn old_shortcut() -> Option<PathBuf> {
    let app = home().join("Applications").join("Claude Sessions.app");
    fs::read_to_string(app.join("Contents").join("Info.plist")).is_ok_and(|plist| plist.contains("dev.cc-sessions.launcher")).then_some(app)
}

#[cfg(not(target_os = "macos"))]
pub fn old_shortcut() -> Option<PathBuf> {
    let entry = home().join(".local").join("share").join("applications").join(format!("{OLD_NAME}.desktop"));
    entry.is_file().then_some(entry)
}

/// Keep the token readable by its owner only.
pub fn restrict_to_owner(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(Error::from)
}

/// Create an account folder that only its owner can enter.
pub fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(Error::from)
}

/// Where the installer's PATH change, if any, has to be made by hand.
pub fn path_note() -> Option<String> {
    (!command_dir_on_path() && rc_file().is_none() && fish_conf().is_none())
        .then(|| "Add ~/.local/bin to your PATH to use the command.".to_string())
}
