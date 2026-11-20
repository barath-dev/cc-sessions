//! What only Claude Code has: its own report of who is signed in.

use serde_json::Value;

use crate::accounts::Account;
use crate::tools::{self, AuthInfo};

/// Which login an account folder holds, from `claude auth status`.
pub fn auth_status(acc: &Account) -> AuthInfo {
    let mut info = AuthInfo::default();
    let data: Value = match tools::run(acc, &["auth", "status"], 20) {
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
