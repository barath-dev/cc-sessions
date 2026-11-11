//! Reopening a session, and signing in, in a new terminal window.

use std::path::Path;

use crate::accounts::Account;
use crate::history::HistoryItem;
use crate::os;
use crate::sessions::Session;
use crate::shell::Launch;
use crate::util::{err, tilde, Result};

/// What a Resume would open.
#[derive(Debug, Clone)]
pub struct Target {
    pub account: String,
    pub background: bool,
    pub id: String,
    pub session_id: String,
    pub cwd: String,
    pub live: bool,
    pub pid: Option<u32>,
}

impl Target {
    pub fn from_session(sess: &Session) -> Target {
        Target {
            account: sess.account.clone(),
            background: sess.is_background(),
            id: sess.id.clone(),
            session_id: sess.session_id.clone(),
            cwd: sess.cwd.clone(),
            live: sess.live,
            pid: sess.pid,
        }
    }

    /// A saved conversation that belongs to a tracked session is opened as
    /// that session (attach for background, refused while open elsewhere);
    /// any other one is a plain `claude --resume`.
    pub fn from_history(item: &HistoryItem, sessions: &[Session]) -> Target {
        if let Some(sess) = sessions.iter().find(|s| s.account == item.account && s.session_id == item.session_id) {
            return Target::from_session(sess);
        }
        Target {
            account: item.account.clone(),
            background: false,
            id: item.id.clone(),
            session_id: item.session_id.clone(),
            cwd: item.cwd.clone(),
            live: false,
            pid: None,
        }
    }
}

fn launch(acc: &Account, target: &Target) -> Launch {
    let args = if target.background {
        vec!["attach".to_string(), target.id.clone()]
    } else {
        vec!["--resume".to_string(), target.session_id.clone()]
    };
    Launch { cwd: Some(target.cwd.clone()), config_dir: acc.config_dir(), alias: acc.alias.clone(), commands: vec![args] }
}

/// The command that reopens the session, as the person would type it.
pub fn display_command(acc: &Account, target: &Target) -> String {
    os::display_command(&launch(acc, target))
}

pub fn check_resumable(target: &Target) -> Result<()> {
    if !target.background && target.live {
        return err(format!("This session is already open in another terminal (pid {}).", target.pid.unwrap_or(0)));
    }
    if target.cwd.is_empty() || !Path::new(&target.cwd).is_dir() {
        let shown = if target.cwd.is_empty() { "?".to_string() } else { tilde(&target.cwd) };
        return err(format!("Its folder {shown} no longer exists."));
    }
    Ok(())
}

/// Open the session in a new terminal window. Returns the terminal's name.
pub fn open(acc: &Account, target: &Target) -> Result<String> {
    check_resumable(target)?;
    os::open_terminal(&launch(acc, target))
}

/// Open a terminal window that signs the account in (after signing out, for a re-login).
pub fn open_login(acc: &Account, relogin: bool) -> Result<String> {
    let mut commands = Vec::new();
    if relogin {
        commands.push(vec!["auth".to_string(), "logout".to_string()]);
    }
    commands.push(vec!["auth".to_string(), "login".to_string()]);
    os::open_terminal(&Launch { cwd: None, config_dir: acc.config_dir(), alias: acc.alias.clone(), commands })
}
