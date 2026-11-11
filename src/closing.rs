//! Closing a session: stop it and keep its conversation.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::accounts::{self, Account};
use crate::claude;
use crate::os;
use crate::sessions::{self, Session};
use crate::store;
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
        if sess.is_background() {
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
    let out = claude::run(acc, &["stop", &sess.id], 45)?;
    if let Some(pid) = sess.pid.filter(|_| sess.live) {
        if !wait_gone(pid, CLOSE_WAIT_SECONDS) {
            return err(format!("claude stop did not stop it: {}", out.last_line()));
        }
        return Ok("Stopped.".into());
    }
    Ok(if sess.live { "Stopped." } else { "Already finished; marked closed." }.into())
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

    os::terminate(pid)?;
    if !wait_gone(pid, CLOSE_WAIT_SECONDS) {
        return err(format!("It did not exit within {CLOSE_WAIT_SECONDS} seconds and was left running."));
    }
    Ok("Stopped.".into())
}
