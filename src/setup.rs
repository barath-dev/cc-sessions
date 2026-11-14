//! `cc-sessions setup`: a few questions, each with a default, that can be
//! answered again at any time. The installers run this once after download.

use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::accounts::{self, Account, Config};
use crate::cli::Args;
use crate::os::{self, ShellHook};
use crate::store::{self, state_file};
use crate::util::{err, expand_tilde, home, now_ms, path_str, tilde, Error, Result};

/// Read one answer; Enter (or no terminal at all) keeps the default.
fn ask(question: &str, default: &str, unattended: bool) -> String {
    if unattended {
        return default.to_string();
    }
    print!("{question} [{default}]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(n) if n > 0 && !line.trim().is_empty() => line.trim().to_string(),
        Ok(n) if n > 0 => default.to_string(),
        _ => {
            println!();
            default.to_string()
        }
    }
}

fn ask_yes(question: &str, default: bool, unattended: bool) -> bool {
    let shown = if default { "Y/n" } else { "y/N" };
    match ask(question, shown, unattended).to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default,
    }
}

/// `alias NAME="CLAUDE_CONFIG_DIR=PATH claude"` lines the person already has,
/// as (folder, alias), so a first import keeps the names they are used to.
pub fn known_aliases(rc_text: &str) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    for line in rc_text.lines() {
        let Some(rest) = line.trim().strip_prefix("alias ") else { continue };
        let Some((name, value)) = rest.split_once('=') else { continue };
        let Some((_, after)) = value.split_once("CLAUDE_CONFIG_DIR=") else { continue };
        if !value.contains("claude") || !accounts::valid_alias(name.trim()) {
            continue;
        }
        let raw: String = after.trim_start_matches(['"', '\'']).chars().take_while(|c| !c.is_whitespace() && !"\"'".contains(*c)).collect();
        let raw = raw.replace("$HOME", "~").replace("${HOME}", "~");
        if !raw.is_empty() {
            found.push((expand_tilde(&raw), name.trim().to_string()));
        }
    }
    found
}

/// Add the hook lines to the shell's startup file, once, keeping a backup.
/// Returns what was done, for the summary.
pub fn apply_hook(hook: &ShellHook) -> Result<String> {
    let shown = tilde(&path_str(&hook.file));
    let before = fs::read_to_string(&hook.file).unwrap_or_default();
    let mut lines: Vec<String> = before.lines().map(String::from).collect();

    // Version 1 loaded a file called aliases.zsh; point that line at the new file.
    let mut changed = false;
    for line in &mut lines {
        if line.contains("cc-sessions/aliases.zsh") {
            *line = hook.lines[0].clone();
            changed = true;
        }
    }
    let missing: Vec<&String> = hook.lines.iter().filter(|wanted| !lines.iter().any(|l| l.trim() == wanted.as_str())).collect();
    if !missing.is_empty() {
        lines.push(String::new());
        lines.push("# cc-sessions: Claude Code account shortcuts".to_string());
        lines.extend(missing.into_iter().cloned());
        changed = true;
    }
    if !changed {
        return Ok(format!("{shown} already loads the shortcuts."));
    }
    if hook.file.exists() {
        let mut backup = PathBuf::from(format!("{}.cc-sessions-backup", path_str(&hook.file)));
        if backup.exists() {
            backup = PathBuf::from(format!("{}.cc-sessions-backup-{}", path_str(&hook.file), now_ms()));
        }
        fs::copy(&hook.file, &backup)?;
    }
    // Written in place rather than renamed over, so a symlinked startup file stays a symlink.
    fs::write(&hook.file, lines.join("\n") + "\n")?;
    Ok(format!("Added the shortcut loader to {shown} (previous version kept beside it)."))
}

fn rc_text(hook: Option<&ShellHook>) -> String {
    hook.and_then(|h| fs::read_to_string(&h.file).ok()).unwrap_or_default()
}

fn valid_command(name: &str) -> Result<()> {
    if !accounts::valid_alias(name) || name == "claude" {
        return err("The command name must be letters, digits, dashes or underscores, and not 'claude'.");
    }
    Ok(())
}

fn switch(args: &Args, on: &str, off: &str) -> Option<bool> {
    if args.has(on) {
        Some(true)
    } else if args.has(off) {
        Some(false)
    } else {
        None
    }
}

pub fn run(args: &Args) -> Result<()> {
    // `--yes`, or no terminal to ask on, means: take the defaults and the flags.
    let unattended = args.has("yes") || !std::io::stdin().is_terminal();
    let config_exists = state_file("config.json").exists();
    let old = accounts::load_config();
    let hook = os::shell_hook();

    println!("cc-sessions setup. Press Enter to keep the value in [brackets].\n");

    // 1. Command name
    let command = match args.value("command") {
        Some(name) => name.to_string(),
        None => ask("Command that starts it", &old.command, unattended),
    };
    valid_command(&command)?;

    // 2. Port
    let port_text = match args.value("port") {
        Some(port) => port.to_string(),
        None => ask("Port for the page", &old.port.to_string(), unattended),
    };
    let port: u16 = port_text.parse().ok().filter(|p| *p >= 1024).ok_or_else(|| Error("Port must be a number from 1024 to 65535.".into()))?;

    // 3. Clickable shortcut
    let shortcut = switch(args, "shortcut", "no-shortcut")
        .unwrap_or_else(|| ask_yes("Add a clickable shortcut (Applications / Start menu)?", old.shortcut, unattended));

    // 4. Register the Claude Code folders found (first run only)
    let mut import: Option<Vec<Account>> = None;
    if accounts::registry_exists() {
        println!("Accounts: {} already registered; left as they are.", accounts::load_accounts(false)?.len());
    } else {
        let found = accounts::discover(&known_aliases(&rc_text(hook.as_ref())));
        if found.is_empty() {
            println!("No Claude Code folders found yet. Add accounts later with: {command} account add NAME");
            import = Some(Vec::new());
        } else {
            println!("Claude Code folders found:");
            for acc in &found {
                let alias = acc.alias.clone().unwrap_or_else(|| "claude".into());
                println!("  {:<12} {:<18} {}", acc.name, alias, tilde(&acc.dir));
            }
            let yes = if args.has("no-import") { false } else { ask_yes("Register these as accounts?", true, unattended) };
            import = Some(if yes { found } else { Vec::new() });
        }
    }

    // 5. Manage the claude-* shortcuts
    let manage_default = if config_exists { old.manage_aliases } else { true };
    let manage = if args.has("no-aliases") {
        false
    } else {
        match &hook {
            Some(h) => {
                println!("To make the claude-* shortcuts work, this line is added to {}:", tilde(&path_str(&h.file)));
                for line in &h.lines {
                    println!("    {line}");
                }
                ask_yes("Let cc-sessions manage the claude-* shortcuts?", manage_default, unattended)
            }
            None => ask_yes("Let cc-sessions manage the claude-* shortcuts?", manage_default, unattended),
        }
    };

    // 6. Updates
    let auto_update = if args.has("no-auto-update") {
        false
    } else {
        ask_yes("Keep cc-sessions up to date automatically?", old.auto_update, unattended)
    };

    // Apply.
    println!();
    // Resolve links so the command points at the real program. Not on Windows,
    // where the resolved form is a \\?\ path that cmd.exe cannot run.
    let exe = if os::IS_WINDOWS { std::env::current_exe()? } else { fs::canonicalize(std::env::current_exe()?)? };
    let config = Config { command: command.clone(), port, shortcut, manage_aliases: manage, idle_minutes: old.idle_minutes, auto_update };
    accounts::save_config(&config)?;

    let link = os::install_command(&command, &exe, Some(&old.command))?;
    println!("Command:   {command}  ({})", tilde(&path_str(&link)));
    println!("Port:      {port}");
    println!("Updates:   {}", if auto_update { "automatic" } else { "by hand (cc-sessions update)" });

    if shortcut {
        println!("Shortcut:  {}", tilde(&path_str(&os::install_shortcut(&exe)?)));
    } else if old.shortcut {
        println!("Shortcut:  no longer wanted; the existing one was left in place for you to delete.");
    }

    if let Some(found) = import {
        store::locked(|| accounts::save_all(&found))?;
        println!("Accounts:  {} registered.", found.len());
    } else {
        accounts::write_shortcuts(&accounts::load_accounts(true)?)?;
    }

    if manage {
        match &hook {
            Some(h) => println!("Shortcuts: {}", apply_hook(h)?),
            None if os::IS_WINDOWS => println!("Shortcuts: claude-* launchers are in the cc-sessions bin folder on your PATH."),
            None => println!(
                "Shortcuts: written to {}; load that file from your shell's startup file to use them.",
                tilde(&path_str(&state_file("aliases.sh")))
            ),
        }
    } else {
        println!("Shortcuts: not managed; your shell files were not touched.");
    }
    if let Some(note) = os::path_note() {
        println!("Note:      {note}");
    }
    println!("\nStart it with: {command}");
    Ok(())
}

/// What `setup` wrote, for tests and troubleshooting.
#[allow(dead_code)]
pub fn summary() -> serde_json::Value {
    let config = accounts::load_config();
    json!({ "command": config.command, "port": config.port, "home": path_str(&home()), "state": path_str(Path::new(&store::state_dir())) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_existing_account_aliases() {
        let rc = "export X=1\nalias claude-work=\"CLAUDE_CONFIG_DIR=~/.claude-work claude\"\n  alias kt='CLAUDE_CONFIG_DIR=$HOME/.claude-kt claude'\nalias ll='ls -l'\nalias bad name=\"CLAUDE_CONFIG_DIR=~/.x claude\"\n";
        let found = known_aliases(rc);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0], (home().join(".claude-work"), "claude-work".to_string()));
        assert_eq!(found[1], (home().join(".claude-kt"), "kt".to_string()));
    }

    #[test]
    fn hook_is_added_once_and_migrates_the_old_line() {
        let dir = std::env::temp_dir().join(format!("cc-sessions-hook-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rc = dir.join("zshrc");
        fs::write(&rc, "export A=1\n[ -f ~/.config/cc-sessions/aliases.zsh ] && source ~/.config/cc-sessions/aliases.zsh\nexport B=2\n").unwrap();
        let hook = ShellHook {
            file: rc.clone(),
            lines: vec!["[ -f ~/.config/cc-sessions/aliases.sh ] && . ~/.config/cc-sessions/aliases.sh".into(), "export PATH=\"$HOME/.local/bin:$PATH\"".into()],
        };
        apply_hook(&hook).unwrap();
        let once = fs::read_to_string(&rc).unwrap();
        assert!(!once.contains("aliases.zsh"));
        assert_eq!(once.matches("aliases.sh ] && .").count(), 1);
        assert_eq!(once.matches("export PATH=").count(), 1);
        assert!(once.starts_with("export A=1\n") && once.contains("export B=2\n"));
        assert!(dir.join("zshrc.cc-sessions-backup").exists());

        apply_hook(&hook).unwrap();
        assert_eq!(fs::read_to_string(&rc).unwrap(), once, "a second run must change nothing");
        let _ = fs::remove_dir_all(&dir);
    }
}
