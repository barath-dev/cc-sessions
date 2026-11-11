//! A stand-in for the real `claude` command, for the tests.
//!
//! It answers from sample files in the account folder it is pointed at:
//!   agents --json --all   prints <config>/fake-agents.json (or [])
//!   auth status           prints <config>/fake-auth.json (or logged out)
//!   stop <id>             ends the pid that fake-agents.json lists for <id>
//!   sleep                 stays alive, to play a running session
//! Every call is appended to $FAKE_CLAUDE_LOG with whether CLAUDE_CONFIG_DIR was set.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn config_dir() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            PathBuf::from(std::env::var_os(home).unwrap_or_default()).join(".claude")
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();

    if let Some(log) = std::env::var_os("FAKE_CLAUDE_LOG") {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(log) {
            let config = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| "<unset>".into());
            let _ = writeln!(file, "{}\tCLAUDE_CONFIG_DIR={config}", args.join(" "));
        }
    }

    match words.as_slice() {
        ["sleep"] => std::thread::sleep(std::time::Duration::from_secs(120)),
        ["--version"] => println!("0.0.0 (fake claude)"),
        ["agents", "--json", "--all"] => {
            let text = fs::read_to_string(config_dir().join("fake-agents.json")).unwrap_or_else(|_| "[]".into());
            println!("{text}");
        }
        ["auth", "status"] => {
            let text = fs::read_to_string(config_dir().join("fake-auth.json"))
                .unwrap_or_else(|_| r#"{"loggedIn": false, "authMethod": "none"}"#.into());
            println!("{text}");
        }
        ["stop", id] => {
            let text = fs::read_to_string(config_dir().join("fake-agents.json")).unwrap_or_default();
            // Find `"id": "<id>"` and the pid in the same object, without a JSON library.
            let pid = text
                .split('{')
                .find(|object| object.contains(&format!("\"{id}\"")))
                .and_then(|object| object.split("\"pid\":").nth(1))
                .and_then(|rest| rest.trim().split(|c: char| !c.is_ascii_digit()).next().map(String::from))
                .filter(|pid| !pid.is_empty());
            if let Some(pid) = pid {
                let _ = if cfg!(windows) {
                    Command::new("taskkill").args(["/PID", &pid, "/F"]).output()
                } else {
                    Command::new("kill").arg(&pid).output()
                };
            }
        }
        _ => {
            eprintln!("fake claude: unsupported arguments {args:?}");
            std::process::exit(2);
        }
    }
}
