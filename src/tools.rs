//! The coding tools whose sessions this program looks after, and what differs
//! between them: the command, the variable that points it at an account's
//! folder, and the folders a fresh install uses. A new tool starts here.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::accounts::Account;
use crate::os;
use crate::proc::{self, Output};
use crate::util::{home, Error, Result};
use crate::{claude, codex};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    /// Claude Code. Accounts registered before there were other tools are this.
    #[default]
    Claude,
    /// OpenAI's Codex CLI (its desktop app keeps conversations in the same place).
    Codex,
}

pub const ALL: [Tool; 2] = [Tool::Claude, Tool::Codex];

impl Tool {
    pub fn id(self) -> &'static str {
        match self {
            Tool::Claude => "claude",
            Tool::Codex => "codex",
        }
    }

    pub fn from_id(id: &str) -> Option<Tool> {
        ALL.into_iter().find(|t| t.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Claude => "Claude Code",
            Tool::Codex => "Codex",
        }
    }

    /// The command that starts it.
    pub fn program(self) -> &'static str {
        self.id()
    }

    /// The variable that points it at an account's folder.
    pub fn env_var(self) -> &'static str {
        match self {
            Tool::Claude => "CLAUDE_CONFIG_DIR",
            Tool::Codex => "CODEX_HOME",
        }
    }

    /// The folder it uses when the variable is not set.
    pub fn default_dir(self) -> PathBuf {
        home().join(format!(".{}", self.id()))
    }

    /// What more folders of it are called: `~/.claude-work`, `~/.codex-work`.
    pub fn folder_prefix(self) -> String {
        format!(".{}-", self.id())
    }

    /// The name the account in the default folder is registered under.
    pub fn default_name(self) -> &'static str {
        match self {
            Tool::Claude => "default",
            Tool::Codex => "codex",
        }
    }

    /// The shortcut a new account gets: `claude-work`, `codex-work`.
    pub fn alias_for(self, name: &str) -> String {
        match self {
            // Codex account names already start with "codex-" when they come from a folder.
            Tool::Codex if name == "codex" || name.starts_with("codex-") => name.to_string(),
            _ => format!("{}-{name}", self.id()),
        }
    }

    pub fn looks_like_config_dir(self, path: &Path) -> bool {
        let markers: &[&str] = match self {
            Tool::Claude => &["projects", "sessions", ".claude.json", "settings.json"],
            Tool::Codex => &["sessions", "config.toml", "auth.json", "session_index.jsonl"],
        };
        markers.iter().any(|child| path.join(child).exists())
    }

    /// The short id a session is shown by. Codex's ids begin with the time they
    /// were made, so conversations started together share their first characters;
    /// their last ones are random.
    pub fn short_id(self, session_id: &str) -> String {
        match self {
            Tool::Claude => session_id.chars().take(8).collect(),
            Tool::Codex => session_id.chars().skip(session_id.chars().count().saturating_sub(8)).collect(),
        }
    }

    /// Arguments that reopen a saved conversation.
    pub fn resume_args(self, session_id: &str) -> Vec<String> {
        match self {
            Tool::Claude => vec!["--resume".into(), session_id.into()],
            Tool::Codex => vec!["resume".into(), session_id.into()],
        }
    }

    pub fn login_args(self) -> Vec<String> {
        match self {
            Tool::Claude => vec!["auth".into(), "login".into()],
            Tool::Codex => vec!["login".into()],
        }
    }

    pub fn logout_args(self) -> Vec<String> {
        match self {
            Tool::Claude => vec!["auth".into(), "logout".into()],
            Tool::Codex => vec!["logout".into()],
        }
    }

    /// True for a variable a parent session of this tool sets that must not reach a child.
    fn inherited(self, key: &str) -> bool {
        let key = key.to_uppercase();
        match self {
            Tool::Claude => key.starts_with("CLAUDE"),
            Tool::Codex => key == "CODEX_HOME",
        }
    }
}

static BINS: [OnceLock<Option<PathBuf>>; ALL.len()] = [OnceLock::new(), OnceLock::new()];

pub fn bin(tool: Tool) -> Result<PathBuf> {
    let slot = &BINS[ALL.iter().position(|t| *t == tool).unwrap_or(0)];
    slot.get_or_init(|| os::which(tool.program()).or_else(|| os::program_fallbacks(tool.program()).into_iter().find(|p| p.is_file())))
        .clone()
        .ok_or_else(|| Error(format!("The {} command was not found on PATH.", tool.program())))
}

/// The tool's command for one account, free of any parent session's variables.
pub fn command(acc: &Account) -> Result<Command> {
    let mut cmd = Command::new(bin(acc.tool)?);
    for (key, _) in std::env::vars_os() {
        if acc.tool.inherited(&key.to_string_lossy()) {
            cmd.env_remove(key);
        }
    }
    if let Some(dir) = acc.config_dir() {
        cmd.env(acc.tool.env_var(), dir);
    }
    cmd.current_dir(home());
    Ok(cmd)
}

pub fn run(acc: &Account, args: &[&str], seconds: u64) -> Result<Output> {
    let mut cmd = command(acc)?;
    cmd.args(args);
    proc::run(&mut cmd, Duration::from_secs(seconds))
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthInfo {
    pub logged_in: bool,
    pub email: Option<String>,
    pub org: Option<String>,
    pub plan: Option<String>,
    pub error: Option<String>,
}

/// Which login an account folder holds.
pub fn auth_status(acc: &Account) -> AuthInfo {
    if !Path::new(&acc.dir).is_dir() {
        return AuthInfo { error: Some("folder is missing".into()), ..AuthInfo::default() };
    }
    match acc.tool {
        Tool::Claude => claude::auth_status(acc),
        Tool::Codex => codex::auth_status(acc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for tool in ALL {
            assert_eq!(Tool::from_id(tool.id()), Some(tool));
            assert_eq!(serde_json::to_value(tool).unwrap(), serde_json::json!(tool.id()));
        }
        assert_eq!(Tool::from_id("cursor"), None);
    }

    #[test]
    fn shortcuts_and_resume() {
        assert_eq!(Tool::Claude.alias_for("work"), "claude-work");
        assert_eq!(Tool::Codex.alias_for("work"), "codex-work");
        assert_eq!(Tool::Codex.alias_for("codex-work"), "codex-work");
        assert_eq!(Tool::Claude.resume_args("abc"), ["--resume", "abc"]);
        assert_eq!(Tool::Codex.resume_args("abc"), ["resume", "abc"]);
        assert_eq!(Tool::Claude.short_id("0f0f0f0f-aaaa-bbbb-cccc-121212121212"), "0f0f0f0f");
        assert_eq!(Tool::Codex.short_id("01a1a9b3-c74d-7cb3-866f-2c9b75cfc8af"), "75cfc8af");
        assert!(Tool::Claude.inherited("CLAUDECODE") && !Tool::Claude.inherited("CODEX_HOME"));
        assert!(Tool::Codex.inherited("CODEX_HOME") && !Tool::Codex.inherited("CODEX_API_KEY"));
    }
}
