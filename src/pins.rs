//! Sessions and folders pinned through this tool, so the page keeps them at
//! the top. Like names, they are kept in the tool's own records and change
//! nothing under an account folder.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::store::{self, state_file};
use crate::util::{err, squash, Result};

const MAX_FOLDER_CHARS: usize = 1024;

pub fn pins_file() -> PathBuf {
    state_file("pins.json")
}

fn key(account_dir: &str, session_id: &str) -> String {
    format!("{account_dir}|{session_id}")
}

#[derive(Default)]
pub struct Pins {
    sessions: Vec<String>,
    folders: Vec<String>,
}

impl Pins {
    pub fn load() -> Result<Pins> {
        let data = store::read_json(&pins_file())?;
        let list = |name: &str| -> Vec<String> {
            data.as_ref()
                .and_then(|data| data.get(name))
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(|item| item.as_str().map(String::from)).collect())
                .unwrap_or_default()
        };
        Ok(Pins { sessions: list("sessions"), folders: list("folders") })
    }

    pub fn has_session(&self, account_dir: &str, session_id: &str) -> bool {
        !session_id.is_empty() && self.sessions.contains(&key(account_dir, session_id))
    }

    /// Pinned folders as the page names them, in the order they were pinned.
    pub fn folders(&self) -> &[String] {
        &self.folders
    }
}

fn change(update: impl FnOnce(&mut Pins)) -> Result<()> {
    store::locked(|| {
        let mut pins = Pins::load()?;
        update(&mut pins);
        store::write_json(&pins_file(), &json!({ "version": 1, "sessions": pins.sessions, "folders": pins.folders }), true)
    })
}

fn put(list: &mut Vec<String>, item: String, pinned: bool) {
    list.retain(|known| *known != item);
    if pinned {
        list.push(item);
    }
}

pub fn set_session(account_dir: &str, session_id: &str, pinned: bool) -> Result<()> {
    if session_id.is_empty() {
        return err("This session has no conversation id, so it cannot be pinned.");
    }
    let item = key(account_dir, session_id);
    change(|pins| put(&mut pins.sessions, item, pinned))
}

pub fn set_folder(folder: &str, pinned: bool) -> Result<()> {
    let folder = squash(folder);
    if folder.is_empty() || folder.chars().count() > MAX_FOLDER_CHARS {
        return err("That is not a folder.");
    }
    change(|pins| put(&mut pins.folders, folder, pinned))
}
