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
use crate::shell::{self, Launch};
use crate::store;
use crate::util::{err, home, path_str, squash, tilde, Error, Result};
use crate::{proc, store::state_dir};

pub const IS_WINDOWS: bool = false;
/// Closing an interactive session asks it to exit (SIGTERM) rather than forcing it.
pub const GRACEFUL_CLOSE: bool = true;
pub const OS_NAME: &str = if cfg!(target_os = "macos") { "macos" } else { "linux" };

pub fn executable_extensions() -> Vec<String> {
    vec![String::new()]
}

pub fn default_state_dir() -> PathBuf {
    home().join(".config").join("cc-sessions")
}

fn command_dir() -> PathBuf {
    home().join(".local").join("bin")
}

pub fn claude_fallbacks() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin/claude"),
        PathBuf::from("/usr/local/bin/claude"),
        home().join(".local/bin/claude"),
        home().join(".claude/local/claude"),
    ]
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
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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

/// Ask the process to exit. It can finish what it is writing first.
pub fn terminate(pid: u32) -> Result<()> {
    // SAFETY: plain kill(2) on a pid the caller has verified.
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(Error(format!("could not signal pid {pid}: {}", std::io::Error::last_os_error())))
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
    cmd.args(["-e", script, &shell::posix(launch, false)]);
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
    dir.is_dir().then(|| dir.join("conf.d").join("cc-sessions.fish"))
}

fn command_dir_on_path() -> bool {
    let dir = command_dir();
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d == dir)).unwrap_or(false)
}

/// Rewrite the generated shortcut files from the registry. `entries` is
/// (alias, config folder or None for the default account).
pub fn write_account_shortcuts(entries: &[(String, Option<String>)], manage: bool) -> Result<()> {
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
    fs::read_link(path).map(|target| target.file_name().is_some_and(|n| n == "cc-sessions")).unwrap_or(false)
}

/// Make `name` start the program: a link in `~/.local/bin`. Refuses to replace
/// anything that is not one of this tool's own links.
pub fn install_command(name: &str, exe: &Path, previous: Option<&str>) -> Result<PathBuf> {
    let dir = command_dir();
    fs::create_dir_all(&dir)?;
    let link = dir.join(name);
    if link.symlink_metadata().is_ok() {
        if !is_our_link(&link) {
            return err(format!("{} already exists and is not a cc-sessions link. Pick another command name.", tilde(&path_str(&link))));
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
    let app = home().join("Applications").join("Claude Sessions.app");
    let macos = app.join("Contents").join("MacOS");
    fs::create_dir_all(&macos)?;
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Claude Sessions</string>
  <key>CFBundleIdentifier</key><string>dev.cc-sessions.launcher</string>
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
    let entry = dir.join("cc-sessions.desktop");
    let icon = store::state_file("icon.png");
    fs::create_dir_all(state_dir())?;
    fs::write(&icon, include_bytes!("../../assets/icon.png"))?;
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Claude Sessions\nComment=See, close and resume Claude Code sessions\nExec=\"{}\" ui\nIcon={}\nTerminal=false\nCategories=Development;\n",
        path_str(exe),
        path_str(&icon)
    );
    fs::write(&entry, text)?;
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o755))?;
    Ok(entry)
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
