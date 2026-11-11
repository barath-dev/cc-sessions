//! Running the `claude` command as one account.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::accounts::Account;
use crate::os;
use crate::proc::{self, Output};
use crate::util::{home, Error, Result};

static CLAUDE: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn claude_bin() -> Result<PathBuf> {
    CLAUDE
        .get_or_init(|| os::which("claude").or_else(|| os::claude_fallbacks().into_iter().find(|p| p.is_file())))
        .clone()
        .ok_or_else(|| Error("The claude command was not found on PATH.".into()))
}

/// A `claude` command for one account, free of any parent session's variables.
pub fn command(acc: &Account) -> Result<Command> {
    let mut cmd = Command::new(claude_bin()?);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().to_uppercase().starts_with("CLAUDE") {
            cmd.env_remove(key);
        }
    }
    if let Some(dir) = acc.config_dir() {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
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

/// Which login an account folder holds, from `claude auth status`.
pub fn auth_status(acc: &Account) -> AuthInfo {
    let mut info = AuthInfo::default();
    if !Path::new(&acc.dir).is_dir() {
        info.error = Some("folder is missing".into());
        return info;
    }
    let data: Value = match run(acc, &["auth", "status"], 20) {
        Ok(out) => match serde_json::from_str(&out.stdout) {
            Ok(data) => data,
            Err(_) => {
                info.error = Some(out.last_line());
                return info;
            }
        },
        Err(e) => {
            info.error = Some(e.0);
            return info;
        }
    };
    let text = |key: &str| data.get(key).and_then(Value::as_str).map(String::from);
    info.logged_in = data.get("loggedIn").and_then(Value::as_bool).unwrap_or(false);
    info.email = text("email");
    info.org = text("orgName");
    info.plan = text("subscriptionType");
    info
}
