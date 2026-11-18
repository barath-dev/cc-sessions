//! Closing a session: stop it and keep its conversation.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::accounts::{self, Account};
use crate::codex;
use crate::os;
use crate::sessions::{self, Session};
use crate::store;
use crate::tools::{self, Tool};
use crate::util::{err, Result};

pub const CLOSE_WAIT_SECONDS: u64 = 5;

#[derive(Debug, Clone, Serialize)]
pub struct Closed {
    pub ok: bool,
    pub message: String,
}

fn wait_gone(pid: u32, seconds: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if !os::pid_alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    !os::pid_alive(pid)
}

/// Stop one session, keeping its conversation. Nothing is marked closed unless
/// the session really is stopped afterwards.
pub fn close_session(sess: &Session) -> Closed {
    let result = accounts::get_account(&sess.account).and_then(|acc| {
        if acc.tool == Tool::Codex {
            close_codex(&acc, sess)
        } else if sess.is_background() {
            close_background(&acc, sess)
        } else {
            close_interactive(&acc, sess)
        }
    });
    match result.and_then(|message| sessions::record_closed(sess).map(|_| message)) {
        Ok(message) => Closed { ok: true, message },
        Err(e) => Closed { ok: false, message: e.0 },
    }
}

fn close_background(acc: &Account, sess: &Session) -> Result<String> {
    let out = tools::run(acc, &["stop", &sess.id], 45)?;
    if let Some(pid) = sess.pid.filter(|_| sess.live) {
        if !wait_gone(pid, CLOSE_WAIT_SECONDS) {
            return err(format!("claude stop did not stop it: {}", out.last_line()));
        }
        return Ok("Stopped.".into());
    }
    Ok(if sess.live { "Stopped." } else { "Already finished; marked closed." }.into())
}

/// A Codex session runs its turns in Codex's background service, which carries
/// on with a turn after the window is gone: the turn is stopped there first,
/// then the window. Run with `--no-daemon`, the window holds the turn itself, and
/// runs each command in a process group of its own that outlives the window.
fn close_codex(acc: &Account, sess: &Session) -> Result<String> {
    let Some(rollout) = &sess.rollout else { return err("Refresh and try again.") };
    let pid = sess.pid.filter(|pid| os::pid_alive(*pid));
    if let Some(pid) = pid {
        if codex::verify(pid, &sess.session_id, rollout, &sess.cwd)? {
            let commands = os::child_groups(pid);
            let stopped = stop(pid)?;
            for group in commands {
                let _ = os::terminate_group(group);
            }
            return Ok(stopped);
        }
    }
    let turn = codex::interrupt(acc, &sess.session_id, rollout);
    match (pid, turn) {
        (Some(pid), Ok(_)) => stop(pid),
        (Some(pid), Err(e)) => stop(pid).map(|_| format!("Closed its window. {e} Its turn may still finish in the background.")),
        (None, Ok(true)) => Ok("Stopped.".into()),
        (None, Ok(false)) => Ok(if sess.pid.is_some() { "Its process had already ended; marked closed." } else { "Already finished; marked closed." }.into()),
        (None, Err(e)) => Err(e),
    }
}

fn close_interactive(acc: &Account, sess: &Session) -> Result<String> {
    let Some(pid) = sess.pid.filter(|pid| os::pid_alive(*pid)) else {
        return Ok("Its process had already ended; marked closed.".into());
    };

    // Make sure the pid still is this session before ending it.
    let record = store::read_json_or_null(&Path::new(&acc.dir).join("sessions").join(format!("{pid}.json")));
    if let Some(recorded) = record.get("sessionId").and_then(Value::as_str) {
        if recorded != sess.session_id {
            return err("That process now belongs to a different session. Refresh and try again.");
        }
    }
    os::verify_session_process(pid, &record)?;
    stop(pid)
}

fn stop(pid: u32) -> Result<String> {
    os::terminate(pid)?;
    if !wait_gone(pid, CLOSE_WAIT_SECONDS) {
        return err(format!("It did not exit within {CLOSE_WAIT_SECONDS} seconds and was left running."));
    }
    Ok("Stopped.".into())
}
