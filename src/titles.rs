//! Names given to sessions through this tool. They are kept in the tool's own
//! records, never in a transcript, so a rename changes nothing under an
//! account folder. Claude Code itself does not see them.

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::store::{self, state_file};
use crate::util::{err, squash, Result};

const MAX_CHARS: usize = 120;

pub fn titles_file() -> PathBuf {
    state_file("titles.json")
}

fn key(account_dir: &str, session_id: &str) -> String {
    format!("{account_dir}|{session_id}")
}

pub struct Titles(Map<String, Value>);

impl Titles {
    pub fn load() -> Result<Titles> {
        Ok(Titles(
            store::read_json(&titles_file())?
                .and_then(|data| data.get("titles").and_then(Value::as_object).cloned())
                .unwrap_or_default(),
        ))
    }

    pub fn get(&self, account_dir: &str, session_id: &str) -> Option<&str> {
        self.0.get(&key(account_dir, session_id)).and_then(Value::as_str).filter(|t| !t.is_empty())
    }
}

/// Name a session, or with an empty `title` go back to Claude Code's own
/// title. Returns the name as stored.
pub fn set(account_dir: &str, session_id: &str, title: &str) -> Result<Option<String>> {
    if session_id.is_empty() {
        return err("This session has no conversation id, so it cannot be renamed.");
    }
    let title: String = squash(title).chars().take(MAX_CHARS).collect();
    let title = Some(title.trim_end().to_string()).filter(|t| !t.is_empty());
    store::locked(|| {
        let mut titles = Titles::load()?.0;
        match &title {
            Some(title) => titles.insert(key(account_dir, session_id), json!(title)),
            None => titles.remove(&key(account_dir, session_id)),
        };
        store::write_json(&titles_file(), &json!({ "version": 1, "titles": titles }), true)
    })?;
    Ok(title)
}
