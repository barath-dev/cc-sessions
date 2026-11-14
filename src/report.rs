//! Problem reports: what is worth knowing about this install, and a new GitHub issue
//! filled in with it. Nothing personal goes in: no paths, names, prompts or emails.

use std::time::Duration;

use serde_json::{json, Value};

use crate::accounts;
use crate::claude;
use crate::os;
use crate::proc;
use crate::server::VERSION;
use crate::update;

const NEW_ISSUE: &str = "https://github.com/barath-dev/cc-sessions/issues/new";
/// Browsers and GitHub cope with addresses well past this; it keeps a report readable.
const MAX_URL: usize = 6000;

/// Version, system and setup, for a report and for the page's report preview.
pub fn diagnostics() -> Value {
    let claude_version = claude::claude_bin()
        .ok()
        .and_then(|bin| proc::run(std::process::Command::new(bin).arg("--version"), Duration::from_secs(10)).ok())
        .map(|out| out.stdout.trim().to_string())
        .filter(|v| !v.is_empty());
    let accounts = accounts::load_accounts(false).map(|a| a.len()).unwrap_or(0);
    let config = accounts::load_config();
    json!({
        "version": VERSION,
        "latest": update::status().get("latest"),
        "system": format!("{} ({})", os::os_release(), std::env::consts::ARCH),
        "claudeCode": claude_version,
        "accounts": accounts,
        "autoUpdate": config.auto_update,
        "installedCopy": update::managed(),
    })
}

/// The diagnostics as the lines that go into the issue.
pub fn summary(diag: &Value) -> String {
    let text = |key: &str| match diag.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "unknown".to_string(),
        Some(other) => other.to_string(),
    };
    let on = |key: &str| if diag.get(key).and_then(Value::as_bool).unwrap_or(false) { "on" } else { "off" };
    format!(
        "cc-sessions: {}\nLatest release: {}\nSystem: {}\nClaude Code: {}\nAccounts: {}\nAuto-update: {}",
        text("version"),
        text("latest"),
        text("system"),
        text("claudeCode"),
        text("accounts"),
        on("autoUpdate")
    )
}

/// A new issue on the bug form, with these fields filled in. The person edits it on GitHub
/// before anything is sent.
pub fn issue_url(fields: &[(&str, &str)]) -> String {
    let mut url = format!("{NEW_ISSUE}?template=bug_report.yml");
    for (name, value) in fields {
        let room = MAX_URL.saturating_sub(url.len() + name.len() + 2);
        let mut encoded = encode(value);
        if encoded.len() > room {
            // An escape is three characters (%XX): step back rather than cut one in half.
            let mut cut = room;
            if let Some(at) = encoded[..cut].rfind('%') {
                if at + 3 > cut {
                    cut = at;
                }
            }
            encoded.truncate(cut);
        }
        url.push_str(&format!("&{name}={encoded}"));
    }
    url
}

/// Percent-encoding for a query value: everything but letters, digits and -._~ is escaped.
pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_escaped_for_a_query() {
        assert_eq!(encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(encode("v2.6.0_x~y-z"), "v2.6.0_x~y-z");
    }

    #[test]
    fn issue_address_is_filled_and_kept_short() {
        let url = issue_url(&[("version", "2.6.0"), ("diagnostics", "line one\nline two")]);
        assert!(url.starts_with("https://github.com/barath-dev/cc-sessions/issues/new?template=bug_report.yml&version=2.6.0&diagnostics=line%20one%0Aline%20two"), "{url}");
        let long = "é".repeat(5000);
        let url = issue_url(&[("diagnostics", &long)]);
        assert!(url.len() <= MAX_URL, "{}", url.len());
        assert!(!url.ends_with('%') && !url[url.len() - 2..].starts_with('%'), "never cut through an escape");
    }

    #[test]
    fn summary_reads_plainly() {
        let text = summary(&json!({ "version": "2.6.0", "latest": null, "system": "macOS 26.1 (aarch64)", "claudeCode": "2.1.0", "accounts": 3, "autoUpdate": true }));
        assert!(text.contains("cc-sessions: 2.6.0") && text.contains("Latest release: unknown") && text.contains("Accounts: 3") && text.contains("Auto-update: on"), "{text}");
    }
}
